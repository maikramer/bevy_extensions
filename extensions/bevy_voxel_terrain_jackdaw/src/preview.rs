//! Camera-local voxel terrain preview for the Jackdaw viewport.
//!
//! The game runtime owns chunk streaming, collisions and the full material;
//! this preview only answers "does the authored shape look right?" while the
//! scene is open. It meshes a single LOD with `bevy_voxel_terrain`'s
//! Transvoxel builder around the active camera and rebuilds when the terrain
//! or one of its feature components changes.

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy_ext_terrain::{
    AhgtHeightField, VoxelArch, VoxelBridge, VoxelCave, VoxelFeature, VoxelTerrain,
};
use bevy_voxel_terrain::{
    HeightField, TintParams, VoxelChunkParams, VoxelField, VoxelMod, build_voxel_mesh,
};
use jackdaw_extension::document::SceneFilePath;
use jackdaw_extension::scene::EditorHidden;

/// A generated preview chunk. The terrain entity is kept so a later rebuild
/// can remove every chunk belonging to it.
#[derive(Component, Clone, Copy, Debug)]
pub struct VoxelPreviewChunk {
    pub terrain: Entity,
}

/// Preview cache/dirty state. The material handle is shared by every chunk.
#[derive(Resource, Default)]
pub struct VoxelPreviewState {
    pub dirty: bool,
    material: Option<Handle<StandardMaterial>>,
}

impl VoxelPreviewState {
    /// Marks the preview for a rebuild on the next update.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }
}

/// How far around the camera the preview reaches, in meters.
const PREVIEW_RADIUS: f32 = 128.0;
/// Extra vertical room above/below the sampled height range, for caves and
/// arches whose mods extend past the heightfield.
const PREVIEW_Y_PADDING: f32 = 40.0;

pub(super) fn mark_dirty(
    changed: Query<
        (),
        Or<(
            Changed<VoxelTerrain>,
            Changed<VoxelCave>,
            Changed<VoxelArch>,
            Changed<VoxelBridge>,
        )>,
    >,
    mut removed_caves: RemovedComponents<VoxelCave>,
    mut removed_arches: RemovedComponents<VoxelArch>,
    mut removed_bridges: RemovedComponents<VoxelBridge>,
    mut state: ResMut<VoxelPreviewState>,
) {
    // Do NOT watch `Changed<Children>` on the terrain: the preview chunks are
    // its children, so a rebuild would dirty itself forever.
    let removed = removed_caves.read().next().is_some()
        || removed_arches.read().next().is_some()
        || removed_bridges.read().next().is_some();
    if !changed.is_empty() || removed {
        state.dirty = true;
    }
}

pub(super) fn rebuild_preview(world: &mut World) {
    if !world.resource::<VoxelPreviewState>().dirty {
        return;
    }

    let terrains: Vec<(Entity, VoxelTerrain)> = world
        .query_filtered::<Entity, With<VoxelTerrain>>()
        .iter(world)
        .filter_map(|entity| {
            world
                .get::<VoxelTerrain>(entity)
                .cloned()
                .map(|terrain| (entity, terrain))
        })
        .collect();

    // Remove the previous patch before building the new one.
    let old_chunks: Vec<Entity> = world
        .query_filtered::<Entity, With<VoxelPreviewChunk>>()
        .iter(world)
        .collect();
    for entity in old_chunks {
        world.despawn(entity);
    }

    let camera = world
        .query_filtered::<&GlobalTransform, With<Camera>>()
        .iter(world)
        .next()
        .map(|transform| transform.translation());

    let project_root = project_root(world);

    for (entity, terrain) in terrains {
        let base = heightfield(&terrain, project_root.as_deref());
        let mut mods: Vec<Box<dyn VoxelMod>> = Vec::new();
        collect_feature_mods(world, entity, base.as_ref(), &mut mods);

        let world_size = terrain.world_size.max(1.0);
        let cell_size = terrain.voxel_size.max(0.25);
        let field = VoxelField::new(mods, world_size, cell_size.max(0.01));

        let terrain_pos = world
            .get::<GlobalTransform>(entity)
            .map(|transform| transform.translation())
            .unwrap_or(Vec3::ZERO);
        let center = camera.unwrap_or(terrain_pos);
        let radius = PREVIEW_RADIUS.min(world_size * 0.5);
        let step = cell_size * bevy_voxel_terrain::VOXEL_CHUNK_CELLS as f32;
        let x0 = ((center.x - radius) / step).floor() * step;
        let x1 = center.x + radius;
        let z0 = ((center.z - radius) / step).floor() * step;
        let z1 = center.z + radius;

        let mut x = x0;
        while x <= x1 {
            let mut z = z0;
            while z <= z1 {
                let (min_y, max_y) = base
                    .range_over(x, z, x + step, z + step)
                    .unwrap_or((0.0, terrain.max_height.max(1.0)));
                let y_start = ((min_y - PREVIEW_Y_PADDING) / step).floor() * step;
                let y_end = max_y + PREVIEW_Y_PADDING;
                let mut y = y_start;
                while y <= y_end {
                    let params = VoxelChunkParams::new(
                        Vec3::new(x, y, z),
                        cell_size,
                        4.0,
                        TintParams::default(),
                        terrain.max_height.max(1.0),
                        false,
                    );
                    if let Some(data) =
                        build_voxel_mesh(&|point| field.density(base.as_ref(), point), &params)
                    {
                        spawn_chunk(world, entity, terrain_pos, Vec3::new(x, y, z), data);
                    }
                    y += step;
                }
                z += step;
            }
            x += step;
        }
    }

    world.resource_mut::<VoxelPreviewState>().dirty = false;
}

fn heightfield(
    terrain: &VoxelTerrain,
    project_root: Option<&std::path::Path>,
) -> Box<dyn HeightField> {
    if !terrain.heightmap.is_empty() {
        let path = resolve_path(&terrain.heightmap, project_root);
        let world_size = (terrain.world_size > 0.0).then_some(terrain.world_size);
        let max_height = (terrain.max_height > 0.0).then_some(terrain.max_height);
        match AhgtHeightField::from_path_with(&path, world_size, max_height) {
            Ok(field) => return Box::new(field),
            Err(err) => {
                warn!(
                    "voxel preview: {}: {err}; using a flat base",
                    path.display()
                );
            }
        }
    }
    Box::new(FlatHeightField {
        height: 0.0,
        max_height: terrain.max_height.max(1.0),
    })
}

fn project_root(world: &World) -> Option<PathBuf> {
    // The open scene tells us where the project is even when the editor was
    // launched from another directory: walk up to the world/`jackdaw.toml`.
    if let Some(scene) = world
        .get_resource::<SceneFilePath>()
        .and_then(|scene| scene.path.as_ref())
    {
        let mut dir = Path::new(scene).parent();
        while let Some(candidate) = dir {
            if candidate.join("world.bsn").is_file() || candidate.join("jackdaw.toml").is_file() {
                return Some(candidate.to_path_buf());
            }
            dir = candidate.parent();
        }
    }
    std::env::current_dir().ok()
}

fn resolve_path(path: &str, project_root: Option<&std::path::Path>) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else if let Some(root) = project_root {
        root.join(path)
    } else {
        path
    }
}

fn collect_feature_mods(
    world: &World,
    entity: Entity,
    base: &dyn HeightField,
    out: &mut Vec<Box<dyn VoxelMod>>,
) {
    let children: Vec<Entity> = world
        .get::<Children>(entity)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    for child in children {
        if let Some(cave) = world.get::<VoxelCave>(child) {
            out.extend(cave.build(base));
        }
        if let Some(arch) = world.get::<VoxelArch>(child) {
            out.extend(arch.build(base));
        }
        if let Some(bridge) = world.get::<VoxelBridge>(child) {
            out.extend(bridge.build(base));
        }
        collect_feature_mods(world, child, base, out);
    }
}

fn spawn_chunk(
    world: &mut World,
    terrain: Entity,
    terrain_pos: Vec3,
    origin: Vec3,
    data: bevy_voxel_terrain::ChunkMeshData,
) {
    if data.indices.is_empty() {
        return;
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, data.colors);
    mesh.insert_indices(Indices::U32(data.indices));
    let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);

    let material = match world.resource::<VoxelPreviewState>().material.clone() {
        Some(material) => material,
        None => {
            let material = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    // The editor viewport may have no environment light while
                    // the project's GLBs are still loading; an unlit preview
                    // still shows the authored shape.
                    base_color: Color::srgb(0.62, 0.70, 0.52),
                    unlit: true,
                    perceptual_roughness: 0.9,
                    metallic: 0.0,
                    ..default()
                });
            world.resource_mut::<VoxelPreviewState>().material = Some(material.clone());
            material
        }
    };

    world.spawn((
        VoxelPreviewChunk { terrain },
        EditorHidden,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::from_translation(origin - terrain_pos),
        Visibility::default(),
        ChildOf(terrain),
    ));
}

/// A constant-height base for previewing authored mods before a heightmap is
/// assigned.
struct FlatHeightField {
    height: f32,
    max_height: f32,
}

impl HeightField for FlatHeightField {
    fn sample(&self, _world_x: f32, _world_z: f32) -> f32 {
        self.height
    }

    fn sample_normal(&self, _world_x: f32, _world_z: f32, _epsilon: f32) -> Vec3 {
        Vec3::Y
    }

    fn max_height(&self) -> f32 {
        self.max_height
    }
}
