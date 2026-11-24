//! Geometry backend: the world as rerecast sees it.
//!
//! `bevy_rerecast` does not know what a world is made of — it asks a *backend*
//! system for one [`TriMesh`] and voxelises that. The builtin `Mesh3dBackend`
//! is no use here for two reasons: the render meshes are LOD ladders (millions
//! of triangles of foliage that are not obstacles), and the terrain colliders
//! only exist within `physics::PHYSICS_CHUNK_RADIUS` of the player, so half the
//! tile would simply have no ground.
//!
//! So this backend builds the input itself, from the two authorities that are
//! always complete:
//!
//! * **the ground** — sampled straight from the [`NavGround`] contract on a
//!   regular XZ lattice over the tile. Cells over water or on a cliff face are
//!   left out entirely: a hole in the input is a hole in the navmesh, which is
//!   exactly what "you cannot walk here" means.
//! * **the obstacles** — the static Avian colliders inside the tile. A
//!   `trimesh` collider (the city walls, the houses, the bridges — the pipeline
//!   bakes a `*_collision.glb` for each) goes in as its own triangles, so a
//!   gateway stays a gateway; anything else goes in as the box of its local
//!   AABB, which is the right approximation for a barrel or a crate.
//!
//! Roads are NOT marked here. `generate_navmesh` calls
//! `TriMesh::mark_walkable_triangles`, which overwrites every area type with
//! `DEFAULT_WALKABLE` before the volumes are applied — the road areas are
//! carried by [`NavmeshSettings::area_volumes`] instead (see
//! [`crate::tile::road_area_volumes`]).

use avian3d::parry::math::Pose;
use avian3d::prelude::{Collider, Sensor};
use bevy::prelude::*;
use bevy_ext_core::{AiLocomotion, Player, RagdollBodyOf, VoxelCollider};
use bevy_rerecast::prelude::NavmeshSettings;
use bevy_rerecast::rerecast::{AreaType, BuildContoursFlags, TriMesh};

use crate::NavGround;

/// Static colliders only: a kinematic body is a creature, and creatures are
/// agents that avoid each other at runtime, not holes in the navmesh.
///
/// Os corpos do ragdoll (`RagdollBodyOf`) são cápsulas dinâmicas que vivem
/// 2,5 s: sem a exclusão cada morte furava a navmesh com as 7 cápsulas e
/// mudava a contagem de obstáculos, forçando um re-bake do tile inteiro.
pub type ObstacleFilter = (
    Without<VoxelCollider>,
    Without<AiLocomotion>,
    Without<Player>,
    // Triggers (loja/porta/água) não são paredes: sem isto viravam
    // obstáculos NOT_WALKABLE no navmesh.
    Without<Sensor>,
    Without<RagdollBodyOf>,
);

/// A collider whose local AABB is taller than this is a wall, a cliff body or a
/// building; below it, it is clutter a character steps over rather than around.
///
/// Rerecast would work this out on its own from `walkable_climb`, but feeding
/// it every pebble in the valley costs voxelisation time for nothing.
pub const MIN_OBSTACLE_HEIGHT: f32 = 0.35;

/// Builds the navmesh input for the tile described by `settings.aabb`.
pub fn nav_backend(
    settings: In<NavmeshSettings>,
    config: Res<crate::NavConfig>,
    ground: Option<Res<crate::NavGroundHandle>>,
    obstacles: Query<(&GlobalTransform, &Collider), ObstacleFilter>,
) -> TriMesh {
    let Some(aabb) = settings.aabb else {
        // No tile: an empty input yields an empty navmesh, and the agents fall
        // back to the beeline. Never a panic — navigation is an upgrade over
        // the old behaviour, not a precondition for the world to run.
        return TriMesh::default();
    };
    let min = Vec3::new(aabb.min.x, aabb.min.y, aabb.min.z);
    let max = Vec3::new(aabb.max.x, aabb.max.y, aabb.max.z);

    let mut trimesh = TriMesh::default();
    if let Some(ground) = ground.as_ref() {
        trimesh.extend(ground_lattice(
            ground.0.as_ref(),
            min.xz(),
            max.xz(),
            config.ground_cell,
        ));
    }
    let ground_tris = trimesh.indices.len();
    let mut taken = 0usize;
    for (global, collider) in &obstacles {
        if let Some(mesh) = obstacle_trimesh(global, collider, min, max) {
            trimesh.extend(mesh);
            taken += 1;
        }
    }
    dump_input(&settings, &trimesh);
    if config.debug {
        info!(
            "nav: backend — chão {} triângulos, obstáculos {}/{} ({} triângulos)",
            ground_tris,
            taken,
            obstacles.iter().count(),
            trimesh.indices.len() - ground_tris
        );
    }
    trimesh
}

/// Writes a bake's whole input to `VAEL_NAV_DUMP=<pasta>` when the variable
/// is set: the triangle mesh and the settings it is voxelised with.
///
/// The navmesh is a function of exactly these two, so one file is the whole
/// of a bake — re-running it offline (`rerecast` alone, no ECS) is how a tile
/// that came out wrong gets examined and turned into a test. One file per
/// bake, in order, named after the tile's corner. Nothing is written without
/// the variable.
fn dump_input(settings: &NavmeshSettings, trimesh: &TriMesh) {
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let Some(dir) = std::env::var_os("VAEL_NAV_DUMP") else {
        return;
    };

    let mut out = Vec::new();
    out.extend_from_slice(b"VAELNAV1");
    let floats = [
        settings.agent_height,
        settings.agent_radius,
        settings.walkable_climb,
        settings.walkable_slope_angle,
        settings.cell_size_fraction,
        settings.cell_height_fraction,
        settings.max_simplification_error,
        settings.detail_sample_dist,
        settings.detail_sample_max_error,
    ];
    let shorts = [
        settings.min_region_size,
        settings.merge_region_size,
        settings.edge_max_len_factor,
        settings.max_vertices_per_polygon,
        settings.tile_size,
    ];
    out.extend(floats.iter().flat_map(|v| v.to_le_bytes()));
    out.extend(shorts.iter().flat_map(|v| v.to_le_bytes()));
    let flags = settings.contour_flags;
    let tessellate = |flag| u8::from(flags.intersects(flag));
    out.push(tessellate(BuildContoursFlags::TESSELLATE_SOLID_WALL_EDGES));
    out.push(tessellate(BuildContoursFlags::TESSELLATE_AREA_EDGES));
    out.push(u8::from(settings.tiling));
    out.push(u8::from(settings.aabb.is_some()));
    if let Some(aabb) = settings.aabb {
        out.extend(
            [
                aabb.min.x, aabb.min.y, aabb.min.z, aabb.max.x, aabb.max.y, aabb.max.z,
            ]
            .iter()
            .flat_map(|v| v.to_le_bytes()),
        );
    }
    out.extend_from_slice(&(settings.area_volumes.len() as u32).to_le_bytes());
    for volume in &settings.area_volumes {
        out.extend_from_slice(&(volume.vertices.len() as u32).to_le_bytes());
        out.extend(
            volume
                .vertices
                .iter()
                .flat_map(|v| [v.x, v.y])
                .flat_map(|f| f.to_le_bytes()),
        );
        out.extend_from_slice(&volume.min_y.to_le_bytes());
        out.extend_from_slice(&volume.max_y.to_le_bytes());
        out.push(volume.area.0);
    }
    out.extend_from_slice(&(trimesh.vertices.len() as u32).to_le_bytes());
    out.extend(
        trimesh
            .vertices
            .iter()
            .flat_map(|v| [v.x, v.y, v.z])
            .flat_map(|f| f.to_le_bytes()),
    );
    out.extend_from_slice(&(trimesh.indices.len() as u32).to_le_bytes());
    out.extend(
        trimesh
            .indices
            .iter()
            .flat_map(|i| [i.x, i.y, i.z])
            .flat_map(|k| k.to_le_bytes()),
    );
    out.extend_from_slice(&(trimesh.area_types.len() as u32).to_le_bytes());
    out.extend(trimesh.area_types.iter().map(|area| area.0));

    let corner = settings.aabb.map_or(Vec3A::ZERO, |aabb| aabb.min);
    let path = Path::new(&dir).join(format!(
        "nav-{:05}-{:.1}_{:.1}.vnav",
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
        corner.x,
        corner.z
    ));
    let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, out));
    if let Err(err) = written {
        warn!("nav: VAEL_NAV_DUMP não gravou {}: {err}", path.display());
    }
}

/// The walkable ground of the tile as a regular lattice of quads.
///
/// Holes (water, cliff faces, outside the heightfield) are simply not emitted.
/// A quad is only emitted when all four of its corners are walkable, so the
/// navmesh edge lands on the last fully solid cell rather than halfway into the
/// lake.
pub fn ground_lattice(ground: &dyn NavGround, min: Vec2, max: Vec2, cell: f32) -> TriMesh {
    let cell = cell.max(0.1);
    let cols = (((max.x - min.x) / cell).ceil() as usize).max(1) + 1;
    let rows = (((max.y - min.y) / cell).ceil() as usize).max(1) + 1;
    // A tile this big is a bug in the caller, not a request: refuse it rather
    // than allocating gigabytes.
    if cols.saturating_mul(rows) > 4_000_000 {
        warn!("nav: lattice {cols}×{rows} pedida — tile grande demais, ignorada");
        return TriMesh::default();
    }

    // Pré-filtra lagos e segmentos de rio que cruzam o tile `[min, max]`:
    // num mundo de 4 km o `hull` de um rio cobre o mapa inteiro, e testar
    // `in_water` por ponto varria ~1 500 estações × 183 000 nós na main
    // thread a cada pedido de tile.
    let tile_max = Vec2::new(
        min.x + (cols.saturating_sub(1)) as f32 * cell,
        min.y + (rows.saturating_sub(1)) as f32 * cell,
    );
    let mut lakes: Vec<(Vec2, f32)> = Vec::new();
    let mut river_segs: Vec<(Vec2, Vec2, Vec2, Vec2, f32)> = Vec::new();
    let mut water_min = Vec2::splat(f32::INFINITY);
    let mut water_max = Vec2::splat(f32::NEG_INFINITY);
    for w in ground.water() {
        let r = w.carve_radius;
        match w.kind {
            crate::NavWaterKind::Lake => {
                if w.at.x + r >= min.x
                    && w.at.x - r <= tile_max.x
                    && w.at.y + r >= min.y
                    && w.at.y - r <= tile_max.y
                {
                    lakes.push((w.at, r));
                    water_min = water_min.min(w.at - Vec2::splat(r));
                    water_max = water_max.max(w.at + Vec2::splat(r));
                }
            }
            crate::NavWaterKind::River => {
                if w.stations.len() == 1 {
                    let s = w.stations[0];
                    if s.x + r >= min.x
                        && s.x - r <= tile_max.x
                        && s.y + r >= min.y
                        && s.y - r <= tile_max.y
                    {
                        lakes.push((s, r));
                        water_min = water_min.min(s - Vec2::splat(r));
                        water_max = water_max.max(s + Vec2::splat(r));
                    }
                } else {
                    for pair in w.stations.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        let smin = a.min(b) - Vec2::splat(r);
                        let smax = a.max(b) + Vec2::splat(r);
                        if smax.x >= min.x
                            && smin.x <= tile_max.x
                            && smax.y >= min.y
                            && smin.y <= tile_max.y
                        {
                            river_segs.push((a, b, smin, smax, r));
                            water_min = water_min.min(smin);
                            water_max = water_max.max(smax);
                        }
                    }
                }
            }
        }
    }
    let has_water = !lakes.is_empty() || !river_segs.is_empty();
    let in_tile_water = |p: Vec2| -> bool {
        if !has_water
            || p.x < water_min.x
            || p.x > water_max.x
            || p.y < water_min.y
            || p.y > water_max.y
        {
            return false;
        }
        for &(center, r) in &lakes {
            if p.distance(center) <= r {
                return true;
            }
        }
        for &(a, b, smin, smax, r) in &river_segs {
            if p.x < smin.x || p.x > smax.x || p.y < smin.y || p.y > smax.y {
                continue;
            }
            let ab = b - a;
            let len_sq = ab.length_squared();
            let closest = if len_sq <= 1e-12 {
                a
            } else {
                a + ab * ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
            };
            if p.distance(closest) <= r {
                return true;
            }
        }
        false
    };

    let mut heights = vec![f32::NAN; cols * rows];
    for row in 0..rows {
        for col in 0..cols {
            let x = min.x + col as f32 * cell;
            let z = min.y + row as f32 * cell;
            let p = Vec2::new(x, z);
            if !ground.in_field(x, z) || in_tile_water(p) || ground.ocean_at(p) {
                continue;
            }
            if ground.is_cliff(p) {
                continue;
            }
            heights[row * cols + col] = ground.surface_top(x, z);
        }
    }

    let mut mesh = TriMesh::default();
    // One vertex per lattice node, emitted lazily so the vertex buffer only
    // holds nodes an emitted quad actually uses.
    let mut vertex_of = vec![u32::MAX; cols * rows];
    let vertex = |mesh: &mut TriMesh, vertex_of: &mut Vec<u32>, index: usize| -> u32 {
        if vertex_of[index] != u32::MAX {
            return vertex_of[index];
        }
        let col = index % cols;
        let row = index / cols;
        let id = mesh.vertices.len() as u32;
        mesh.vertices.push(
            Vec3::new(
                min.x + col as f32 * cell,
                heights[index],
                min.y + row as f32 * cell,
            )
            .into(),
        );
        vertex_of[index] = id;
        id
    };

    for row in 0..rows.saturating_sub(1) {
        for col in 0..cols.saturating_sub(1) {
            let corners = [
                row * cols + col,
                row * cols + col + 1,
                (row + 1) * cols + col + 1,
                (row + 1) * cols + col,
            ];
            if corners.iter().any(|i| heights[*i].is_nan()) {
                continue;
            }
            let ids = corners.map(|i| vertex(&mut mesh, &mut vertex_of, i));
            // Counter-clockwise seen from above (+Y normal) — rerecast keeps a
            // triangle when its normal points up.
            mesh.indices.push(UVec3::new(ids[0], ids[3], ids[1]));
            mesh.indices.push(UVec3::new(ids[1], ids[3], ids[2]));
            mesh.area_types
                .extend([AreaType::DEFAULT_WALKABLE, AreaType::DEFAULT_WALKABLE]);
        }
    }
    mesh
}

/// One static collider as navmesh input, or `None` when it is outside the tile
/// or too low to matter.
fn obstacle_trimesh(
    global: &GlobalTransform,
    collider: &Collider,
    min: Vec3,
    max: Vec3,
) -> Option<TriMesh> {
    // `shape_scaled` já leva a escala do collider dobrada, por isso só
    // translação e rotação vêm do transform.
    let placement = global.compute_transform();
    let iso = Transform {
        translation: placement.translation,
        rotation: placement.rotation,
        scale: Vec3::ONE,
    };
    let local = collider.shape_scaled().compute_aabb(&Pose::identity());
    let local_min = Vec3::new(local.mins.x, local.mins.y, local.mins.z);
    let local_max = Vec3::new(local.maxs.x, local.maxs.y, local.maxs.z);
    if local_max.y - local_min.y < MIN_OBSTACLE_HEIGHT {
        return None;
    }
    // Reject on the world AABB of the (rotated) local AABB: cheap, and the
    // false positives it lets through are clipped by the voxelisation anyway.
    let (world_min, world_max) = rotated_aabb(&iso, local_min, local_max);
    if world_max.x < min.x || world_min.x > max.x || world_max.z < min.z || world_min.z > max.z {
        return None;
    }

    let mut mesh = TriMesh::default();
    if let Some(trimesh) = collider.shape_scaled().as_trimesh() {
        // The baked collision mesh: gateways, arches and bridge decks stay
        // open because their real geometry is what gets voxelised.
        for tri in trimesh.triangles() {
            let base = mesh.vertices.len() as u32;
            for point in [tri.a, tri.b, tri.c] {
                mesh.vertices.push(iso.transform_point(point).into());
            }
            mesh.indices.push(UVec3::new(base, base + 1, base + 2));
            mesh.area_types.push(AreaType::NOT_WALKABLE);
        }
        return Some(mesh);
    }
    if let Some(compound) = collider.shape_scaled().as_compound() {
        for (sub_pose, sub_shape) in compound.shapes() {
            let sub = sub_shape.compute_aabb(sub_pose);
            let sub_min = Vec3::new(sub.mins.x, sub.mins.y, sub.mins.z);
            let sub_max = Vec3::new(sub.maxs.x, sub.maxs.y, sub.maxs.z);
            if sub_max.y - sub_min.y >= MIN_OBSTACLE_HEIGHT {
                mesh.extend(box_trimesh(&iso, sub_min, sub_max));
            }
        }
        if !mesh.indices.is_empty() {
            return Some(mesh);
        }
    }
    Some(box_trimesh(&iso, local_min, local_max))
}

/// World-space AABB of a local AABB placed by `iso`.
fn rotated_aabb(iso: &Transform, local_min: Vec3, local_max: Vec3) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for corner in aabb_corners(local_min, local_max) {
        let p = iso.transform_point(corner);
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

fn aabb_corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ]
}

/// The 12 triangles of a box, as an obstacle.
fn box_trimesh(iso: &Transform, local_min: Vec3, local_max: Vec3) -> TriMesh {
    let mut mesh = TriMesh::default();
    for corner in aabb_corners(local_min, local_max) {
        mesh.vertices.push(iso.transform_point(corner).into());
    }
    // `generate_navmesh` corre `TriMesh::mark_walkable_triangles`, que promove
    // a `DEFAULT_WALKABLE` todo o triângulo cuja normal aponte para +Y. A tampa
    // +Y vai enrolada com normal -Y ([4, 5, 6, 4, 6, 7]): o voxelizador de
    // spans continua a fechar o topo da caixa, mas `mark_walkable_triangles`
    // preserva `AreaType::NOT_WALKABLE` para que pedras baixas nunca virem chão.
    const FACES: [[u32; 6]; 6] = [
        [0, 1, 2, 0, 2, 3], // -Y
        [4, 5, 6, 4, 6, 7], // +Y (normal para baixo: topo nunca caminhável)
        [0, 4, 5, 0, 5, 1], // -Z
        [3, 2, 6, 3, 6, 7], // +Z
        [0, 3, 7, 0, 7, 4], // -X
        [1, 5, 6, 1, 6, 2], // +X
    ];
    for face in FACES {
        mesh.indices.push(UVec3::new(face[0], face[1], face[2]));
        mesh.indices.push(UVec3::new(face[3], face[4], face[5]));
        mesh.area_types
            .extend([AreaType::NOT_WALKABLE, AreaType::NOT_WALKABLE]);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_and_compound_obstacles_stay_not_walkable_after_mark_walkable_triangles() {
        let compound = Collider::compound(vec![
            (Vec3::ZERO, Quat::IDENTITY, Collider::cuboid(0.8, 0.7, 0.8)),
            (
                Vec3::new(0.6, 0.0, 0.3),
                Quat::IDENTITY,
                Collider::cuboid(0.5, 0.5, 0.5),
            ),
        ]);
        let mut mesh = obstacle_trimesh(
            &GlobalTransform::IDENTITY,
            &compound,
            Vec3::splat(-10.0),
            Vec3::splat(10.0),
        )
        .expect("compound rock produces obstacle trimesh");
        assert_eq!(mesh.indices.len(), 24, "two segments -> two 12-tri boxes");
        mesh.mark_walkable_triangles(50.0_f32.to_radians());
        assert!(
            mesh.area_types.iter().all(|&a| a == AreaType::NOT_WALKABLE),
            "no face of a rock obstacle may be promoted to walkable by mark_walkable_triangles"
        );
    }
}
