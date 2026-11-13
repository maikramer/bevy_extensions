//! Native previews for the Vael `.bsn` while the official editor's asset
//! loader cannot read the meshopt/KTX2 pool.
//!
//! Three pieces make `world.bsn` legible without trying to draw the whole
//! world at full detail:
//!
//! - the `WorldNode` `Terrain` gets a real heightfield mesh from its `.ahgt`
//!   (with the `.paint` layers tinting the vertices);
//! - models near the viewport get an AABB box sized by the GLB's own bounds
//!   (`GlbBounds`, no loader needed) and are swapped for the real glTF scene
//!   only while they are big enough on screen;
//! - every other model is folded into a single merged box mesh, so the whole
//!   world stays visible at overview for one draw call.
//!
//! The pipeline is progressive on purpose: opening a 9k-node world used to
//! read 3.7k GLBs and spawn 3.7k previews in one frame. The scan now uses
//! change ticks, the spawn/upgrade/despawn work has per-frame budgets, and the
//! detail tier follows the apparent pixel size (so an orbit camera zoomed out
//! keeps the world readable without spawning everything).
//!
//! The previews carry `EditorHidden` and `SkipSerialization` and are not part
//! of the scene document, so they never enter the saved `.bsn`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::gltf::GltfAssetLabel;
use bevy::light::NotShadowCaster;
use bevy::pbr::StandardMaterial;
use bevy::picking::Pickable;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy::world_serialization::WorldAssetRoot;
use bevy_ext_gltf::glb_bounds::{self, GlbBounds};
use bevy_ext_terrain::{AhgtHeightField, PaintGrid};
use bevy_ext_world::bsn::WorldNode;
use bevy_voxel_terrain::HeightField;
use jackdaw_extension::EditorEntity;
use jackdaw_extension::document::SceneFilePath;
use jackdaw_extension::prelude::*;
use jackdaw_extension::scene::{EditorHidden, SkipSerialization};
use jackdaw_extension::viewport::MainViewportCamera;

/// One generated preview child.
#[derive(Component)]
pub struct WorldPreview;

/// The terrain preview: its heights are world-space (the runtime ignores the
/// `<Terrain>` pose), so it is spawned unparented at the origin.
#[derive(Component)]
pub struct TerrainPreview;

/// The merged mesh with every model that is not individually previewed.
#[derive(Component)]
pub struct FarPreview;

/// Detail tier of an individual preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// No individual entity: the node is in the merged far mesh.
    Far,
    /// AABB box.
    Box,
    /// Decoded glTF scene.
    Real,
}

/// Real models instead of boxes (toggled by `vael.preview.toggle_models`).
#[derive(Resource)]
pub struct ModelPreviewMode {
    pub real: bool,
}

impl Default for ModelPreviewMode {
    fn default() -> Self {
        Self { real: true }
    }
}

/// Progressive-preview knobs. The defaults keep the editor responsive on the
/// 9k-node Vael world; the env vars are for QA without a rebuild.
#[derive(Resource, Clone, Debug)]
pub struct PreviewPolicy {
    pub enabled: bool,
    /// Apparent radius (pixels) at which a box becomes a real model.
    pub real_px: f32,
    /// Apparent radius (pixels) below which a node goes to the far mesh.
    pub box_px: f32,
    /// Maximum individual previews (boxes + real models).
    pub max_live: usize,
    /// Maximum real glTF scenes.
    pub max_real: usize,
    /// Box spawns per frame.
    pub spawn_budget: usize,
    /// Box → real upgrades per frame.
    pub upgrade_budget: usize,
    /// Preview despawns per frame.
    pub despawn_budget: usize,
    /// Re-evaluate when the camera moves more than this (meters).
    pub camera_move: f32,
    /// Minimum seconds between tier evaluations.
    pub refresh_secs: f32,
}

impl Default for PreviewPolicy {
    fn default() -> Self {
        let env = |key: &str| std::env::var(key).ok();
        let parse = |key: &str, default: f32| {
            env(key)
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(default)
        };
        Self {
            enabled: env("VAEL_PREVIEW").map_or(true, |value| value != "0"),
            real_px: parse("VAEL_PREVIEW_REAL_PX", 10.0),
            box_px: parse("VAEL_PREVIEW_BOX_PX", 0.6),
            max_live: parse("VAEL_PREVIEW_MAX_LIVE", 700.0).max(1.0) as usize,
            max_real: parse("VAEL_PREVIEW_MAX_REAL", 400.0).max(0.0) as usize,
            spawn_budget: 48,
            upgrade_budget: 10,
            despawn_budget: 64,
            camera_move: 2.0,
            refresh_secs: 0.25,
        }
    }
}

impl PreviewPolicy {
    /// Tier a node should have, with hysteresis so it does not flip at a
    /// threshold. Pure so the unit tests can pin the policy.
    pub fn desired_tier(&self, current: Tier, apparent_px: f32) -> Tier {
        match current {
            Tier::Real => {
                if apparent_px >= self.real_px * 0.8 {
                    Tier::Real
                } else if apparent_px >= self.box_px * 0.8 {
                    Tier::Box
                } else {
                    Tier::Far
                }
            }
            Tier::Box => {
                if apparent_px >= self.real_px {
                    Tier::Real
                } else if apparent_px >= self.box_px * 0.8 {
                    Tier::Box
                } else {
                    Tier::Far
                }
            }
            Tier::Far => {
                if apparent_px >= self.real_px {
                    Tier::Real
                } else if apparent_px >= self.box_px {
                    Tier::Box
                } else {
                    Tier::Far
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct TerrainSpec {
    heightmap: String,
    world_size: f32,
    max_height: f32,
}

impl TerrainSpec {
    fn digest(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.heightmap.hash(&mut hasher);
        self.world_size.to_bits().hash(&mut hasher);
        self.max_height.to_bits().hash(&mut hasher);
        hasher.finish()
    }
}

/// Rough kind of a model, from its asset path: colours the boxes and the far
/// mesh so the overview reads (trees vs buildings vs rocks) before the real
/// models arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Category {
    Plant,
    Building,
    Rock,
    Creature,
    #[default]
    Prop,
}

impl Category {
    pub fn of(url: &str) -> Self {
        let url = url.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|needle| url.contains(needle));
        if has(&["/characters/", "/creatures/", "/npc"]) {
            Self::Creature
        } else if has(&[
            "/forest/", "/vegetation/", "tree", "pine", "birch", "oak", "bush", "cactus", "fern",
            "reed", "flower", "grass", "shrub",
        ]) {
            Self::Plant
        } else if has(&["rock", "stone", "cliff", "boulder", "/terrain/", "cairn", "crystal"]) {
            Self::Rock
        } else if has(&[
            "/village/", "/infra/", "/interiors/", "/farm/", "house", "wall", "gate", "tower",
            "bridge", "shack", "hut", "chapel", "barn", "fence",
        ]) {
            Self::Building
        } else {
            Self::Prop
        }
    }

    pub fn color(self) -> Vec3 {
        match self {
            Self::Plant => Vec3::new(0.27, 0.45, 0.22),
            Self::Building => Vec3::new(0.66, 0.52, 0.36),
            Self::Rock => Vec3::new(0.56, 0.56, 0.53),
            Self::Creature => Vec3::new(0.78, 0.30, 0.24),
            Self::Prop => Vec3::new(0.70, 0.62, 0.44),
        }
    }
}

/// Fake light per cube face (unlit material): tops bright, sides darker, so
/// the boxes read as volumes.
const FACE_SHADE: [f32; 6] = [0.72, 0.82, 0.64, 0.90, 0.45, 1.0];

/// Unit cube centred at the origin, one quad per face (24 vertices) in the
/// order of [`FACE_SHADE`]: -Z, +Z, -X, +X, -Y, +Y.
fn shaded_cube(
    matrix: Mat4,
    color: Vec3,
    positions: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
) {
    const FACES: [[[f32; 3]; 4]; 6] = [
        [[-0.5, -0.5, -0.5], [-0.5, 0.5, -0.5], [0.5, 0.5, -0.5], [0.5, -0.5, -0.5]],
        [[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]],
        [[-0.5, -0.5, -0.5], [-0.5, -0.5, 0.5], [-0.5, 0.5, 0.5], [-0.5, 0.5, -0.5]],
        [[0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5], [0.5, -0.5, 0.5]],
        [[-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, -0.5, 0.5], [-0.5, -0.5, 0.5]],
        [[-0.5, 0.5, -0.5], [-0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]],
    ];
    for (face, shade) in FACES.iter().zip(FACE_SHADE) {
        let base = positions.len() as u32;
        let c = color * shade;
        for corner in face {
            positions.push(matrix.transform_point3(Vec3::from(*corner)).to_array());
            colors.push([c.x, c.y, c.z, 1.0]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

fn shaded_mesh(positions: Vec<[f32; 3]>, colors: Vec<[f32; 4]>, indices: Vec<u32>) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// What the preview pipeline needs to know about one `WorldNode`.
#[derive(Clone, Debug)]
struct TrackedNode {
    is_terrain: bool,
    is_scatter: bool,
    /// Asset path (normalized: no leading `/`, no fragment).
    url: Option<String>,
    /// Lighter LOD for the box/model preview, when authored.
    lod1: Option<String>,
    terrain: Option<TerrainSpec>,
    category: Category,
}

impl TrackedNode {
    fn from_node(node: &WorldNode) -> Self {
        let is_terrain = node.tag.eq_ignore_ascii_case("terrain");
        let is_scatter = node.tag.eq_ignore_ascii_case("scatter");
        let normalize = |value: &str| {
            value
                .split('#')
                .next()
                .unwrap_or(value)
                .trim_start_matches('/')
                .to_string()
        };
        let terrain = is_terrain.then(|| TerrainSpec {
            heightmap: node.attr("heightmap").unwrap_or_default().to_string(),
            world_size: node
                .attr("world-size")
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(4000.0),
            max_height: node
                .attr("max-height")
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(200.0),
        });
        let url = node.attr("url").map(normalize);
        Self {
            is_terrain,
            is_scatter,
            category: url.as_deref().map(Category::of).unwrap_or_default(),
            url,
            lod1: node.attr("lod1-url").map(normalize),
            terrain,
        }
    }

    fn previewable(&self) -> bool {
        self.url.is_some() && !self.is_terrain && !self.is_scatter
    }
}

/// Scene `WorldNode`s that changed since the last scan.
///
/// The scene's `WorldNode` is read by **reflection** (type path), not by
/// type: the game registers the same type path in the project schema, and
/// depending on the start-up order the component the editor inserts is not
/// this dylib's copy of the type. `FromReflect` rebuilds our own value.
///
/// Change detection uses the component's own `ComponentTicks` (read by
/// `ComponentId`), so the scan only reflects nodes the editor actually
/// touched. The first pass (`last_tick == None`) sees everything.
#[derive(Resource, Default)]
pub struct NodeScan {
    last_tick: Option<bevy::ecs::change_detection::Tick>,
    tracked: HashMap<Entity, TrackedNode>,
    changed: Vec<Entity>,
    removed: Vec<Entity>,
    /// Descendants of a `<Scatter>` recipe: identity-transform templates that
    /// must not be previewed as if they were placed props.
    templates: HashSet<Entity>,
    timer: f32,
}

impl NodeScan {
    /// Forget everything: the next scan re-emits every node.
    pub fn reset(&mut self) {
        self.last_tick = None;
        self.timer = 0.0;
        self.changed.clear();
        self.removed.clear();
        self.templates.clear();
    }
}

const WORLD_NODE_PATH: &str = "bevy_world_xml::bsn::WorldNode";
const MODEL_LOD_PATH: &str = "bevy_ext_scene::ModelLod";

/// `ModelLod.lod1` por reflexão (o tipo chega pelo schema do jogo).
fn reflected_lod1(value: &dyn bevy::reflect::PartialReflect) -> Option<String> {
    let bevy::reflect::ReflectRef::Struct(value) = value.reflect_ref() else {
        return None;
    };
    value
        .field("lod1")?
        .try_downcast_ref::<Option<String>>()?
        .clone()
}

fn normalize_url(value: &str) -> String {
    value
        .split('#')
        .next()
        .unwrap_or(value)
        .trim_start_matches('/')
        .to_string()
}
const SCAN_INTERVAL: f32 = 0.5;

pub(super) fn scan_world_nodes(world: &mut World) {
    use bevy::ecs::reflect::ReflectComponent;
    use bevy::reflect::FromReflect;

    let delta = world.resource::<Time>().delta_secs();
    {
        let mut scan = world.resource_mut::<NodeScan>();
        scan.timer -= delta;
        if scan.timer > 0.0 {
            return;
        }
        scan.timer = SCAN_INTERVAL;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(registration) = registry.get_with_type_path(WORLD_NODE_PATH) else {
        return;
    };
    let Some(reflect) = registration.data::<ReflectComponent>() else {
        return;
    };
    let Some(component) = world.components().get_id(registration.type_id()) else {
        return;
    };
    // `bevy_ext_scene::ModelLod` tipado (fonte); `lod1-url` só em cenas por
    // migrar.
    let lod_reflect = registry
        .get_with_type_path(MODEL_LOD_PATH)
        .and_then(|registration| registration.data::<ReflectComponent>());
    let last_tick = world.resource::<NodeScan>().last_tick;
    let now = world.change_tick();

    let mut found = Vec::new();
    let mut alive = HashSet::new();
    let mut query = world.query::<EntityRef>();
    for entity in query.iter(world) {
        if !entity.contains_id(component) {
            continue;
        }
        let id = entity.id();
        alive.insert(id);
        let is_new_or_changed = match last_tick {
            None => true,
            Some(last) => entity
                .get_change_ticks_by_id(component)
                .map(|ticks| ticks.is_added(last, now) || ticks.is_changed(last, now))
                .unwrap_or(true),
        };
        if !is_new_or_changed {
            continue;
        }
        let Some(value) = reflect.reflect(entity) else {
            continue;
        };
        let Some(node) = WorldNode::from_reflect(value.as_partial_reflect()) else {
            continue;
        };
        let mut tracked = TrackedNode::from_node(&node);
        if let Some(lod1) = lod_reflect
            .and_then(|lod| lod.reflect(entity))
            .and_then(|lod| reflected_lod1(lod.as_partial_reflect()))
        {
            tracked.lod1 = Some(normalize_url(&lod1));
        }
        found.push((id, tracked));
    }
    drop(registry);

    let mut scan = world.resource_mut::<NodeScan>();
    scan.last_tick = Some(now);
    let removed: Vec<Entity> = scan
        .tracked
        .keys()
        .filter(|entity| !alive.contains(*entity))
        .copied()
        .collect();
    scan.removed = removed;
    scan.tracked.retain(|entity, _| alive.contains(entity));
    scan.changed.clear();
    for (entity, node) in found {
        scan.tracked.insert(entity, node);
        scan.changed.push(entity);
    }
    drop(scan);

    // Recompute the scatter-template set from the live hierarchy. Templates
    // are the recipe's identity-transform children; the generated instances
    // are siblings under `spawn/grp-*` and therefore stay previewable.
    let scatter_roots: Vec<Entity> = world
        .resource::<NodeScan>()
        .tracked
        .iter()
        .filter(|(_, node)| node.is_scatter)
        .map(|(entity, _)| *entity)
        .collect();
    let mut templates = HashSet::new();
    for root in scatter_roots {
        let mut stack = vec![root];
        let mut visited = HashSet::new();
        while let Some(entity) = stack.pop() {
            if !visited.insert(entity) {
                continue;
            }
            if let Some(children) = world.get::<Children>(entity) {
                for child in children.iter() {
                    templates.insert(child);
                    stack.push(child);
                }
            }
        }
    }
    world.resource_mut::<NodeScan>().templates = templates;
}

#[derive(Resource, Default)]
pub struct BoundsCache {
    /// Source → bounds (`None`: no GLB/geometry; do not ask again).
    by_source: HashMap<String, Option<GlbBounds>>,
    resolved: HashMap<String, Option<PathBuf>>,
    /// URL → chosen source (`lod1` when it is on disk, otherwise the base).
    chosen: HashMap<String, Option<String>>,
    pending: VecDeque<(String, PathBuf)>,
    queued: HashSet<String>,
    failed: HashSet<String>,
}

impl BoundsCache {
    fn get(&self, source: &str) -> Option<Option<GlbBounds>> {
        self.by_source.get(source).copied()
    }

    /// Pick the model to preview: LOD1 when the file exists (lighter), else
    /// the base GLB. LOD1/LOD2 are derived and missing in a fresh clone, so
    /// the existence check is not optional.
    fn source_for(&mut self, root: &Path, node: &TrackedNode) -> Option<String> {
        let url = node.url.as_deref()?;
        if let Some(chosen) = self.chosen.get(url) {
            return chosen.clone();
        }
        let chosen = [node.lod1.as_deref(), Some(url)]
            .into_iter()
            .flatten()
            .find(|candidate| resolve_asset(root, candidate).is_some())
            .map(str::to_string);
        self.chosen.insert(url.to_string(), chosen.clone());
        chosen
    }

    /// Chosen source without touching the cache (far mesh path).
    fn chosen_source<'a>(&'a self, node: &'a TrackedNode) -> Option<&'a str> {
        let url = node.url.as_deref()?;
        match self.chosen.get(url) {
            Some(Some(source)) => Some(source),
            Some(None) => None,
            None => Some(url),
        }
    }

    /// Forget every bounds/source decision (after a rebuild).
    pub fn invalidate(&mut self) {
        self.by_source.clear();
        self.resolved.clear();
        self.chosen.clear();
        self.pending.clear();
        self.queued.clear();
        self.failed.clear();
    }

    /// Ask for the GLB's AABB once. The read only touches the JSON chunk, so
    /// it is cheap enough to budget a few per frame instead of blocking the
    /// first sync on 3.7k full-file reads.
    fn request(&mut self, root: &Path, source: &str) {
        if self.by_source.contains_key(source)
            || self.queued.contains(source)
            || self.failed.contains(source)
        {
            return;
        }
        let path = self
            .resolved
            .entry(source.to_string())
            .or_insert_with(|| resolve_asset(root, source))
            .clone();
        match path {
            Some(path) => {
                self.queued.insert(source.to_string());
                self.pending.push_back((source.to_string(), path));
            }
            None => {
                self.failed.insert(source.to_string());
            }
        }
    }

    /// Bounds known for `source`, or the small fallback while the read is in
    /// flight (or when the GLB has no geometry).
    fn bounds_or_fallback(&self, source: &str) -> (Vec3, Vec3) {
        self.get(source)
            .flatten()
            .map(|bounds| (bounds.center(), bounds.half_extents()))
            .unwrap_or((Vec3::ZERO, Vec3::splat(0.5)))
    }
}

const BOUNDS_READS_PER_FRAME: usize = 8;

pub(super) fn poll_bounds(mut bounds: ResMut<BoundsCache>, mut book: ResMut<PreviewBook>) {
    for _ in 0..BOUNDS_READS_PER_FRAME {
        let Some((source, path)) = bounds.pending.pop_front() else {
            break;
        };
        bounds.queued.remove(&source);
        let result = glb_bounds::read_json_chunk(&path);
        if result.is_none() && !path.is_file() {
            // Remember the failure; retrying an absent file every frame is
            // worse than showing the fallback box.
            bounds.failed.insert(source.clone());
        }
        bounds.by_source.insert(source, result);
        book.force = true;
    }
}

/// Preview entity bookkeeping.
#[derive(Resource, Default)]
pub struct PreviewBook {
    entries: HashMap<Entity, PreviewEntry>,
    /// Owners whose target tier is Real and whose box is waiting for its
    /// decoded GLB.
    real_wanted: HashSet<Entity>,
    last_camera: Vec3,
    last_refresh: f32,
    pub force: bool,
}

#[derive(Clone, Debug)]
struct PreviewEntry {
    preview: Entity,
    tier: Tier,
    score: f32,
    source: Option<String>,
}

impl PreviewBook {
    /// Ask the next update to re-evaluate every node.
    pub fn invalidate(&mut self) {
        self.force = true;
    }
}

#[derive(Resource, Clone, Default)]
pub struct PreviewStatus {
    pub tracked: usize,
    pub previewable: usize,
    pub boxes: usize,
    pub real: usize,
    pub far: usize,
    pub queued: usize,
    pub terrain: Option<String>,
}

#[derive(Resource, Default)]
pub struct PreviewAssets {
    cubes: HashMap<Category, Handle<Mesh>>,
    cube_material: Option<Handle<StandardMaterial>>,
}

impl PreviewAssets {
    fn cube(&mut self, meshes: &mut Assets<Mesh>, category: Category) -> Handle<Mesh> {
        self.cubes
            .entry(category)
            .or_insert_with(|| {
                let (mut positions, mut colors, mut indices) = (Vec::new(), Vec::new(), Vec::new());
                shaded_cube(
                    Mat4::IDENTITY,
                    category.color(),
                    &mut positions,
                    &mut colors,
                    &mut indices,
                );
                meshes.add(shaded_mesh(positions, colors, indices))
            })
            .clone()
    }

    fn cube_material(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.cube_material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    // The colour is in the vertices (category × face shade).
                    base_color: Color::WHITE,
                    unlit: true,
                    perceptual_roughness: 0.8,
                    ..default()
                })
            })
            .clone()
    }
}

/// Decode jobs and their results, by source path.
#[derive(Resource, Default)]
pub struct GlbCache {
    jobs: HashMap<String, Task<Option<String>>>,
    /// Source → cached asset path (`None`: could not be decoded; keep the box).
    ready: HashMap<String, Option<String>>,
    /// Every model of the scene, decoded in the background (one at a time)
    /// so the next zoom-in finds the cache warm.
    prewarm: VecDeque<String>,
    prewarm_seeded: bool,
}

/// A box kept under its freshly spawned real model until the glTF scene has
/// instanced, so the swap never leaves a hole while the model loads.
#[derive(Component)]
pub struct RetiringBox;

/// A click on a real model hits the glTF's own entities (they are not
/// editor entities), so the editor would select a mesh inside the preview.
/// Hand the selection to the `WorldNode` that owns the preview instead.
pub(super) fn redirect_preview_selection(
    added: On<Add<jackdaw_extension::selection::Selected>>,
    previews: Query<(), With<WorldPreview>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let mut entity = added.event_target();
    let owner = loop {
        let Ok(child_of) = parents.get(entity) else {
            return;
        };
        if previews.contains(entity) {
            break child_of.parent();
        }
        entity = child_of.parent();
    };
    commands.queue(move |world: &mut World| {
        if let Err(err) = world.operator("selection.select").param("entity", owner).call() {
            warn!("preview: não foi possível selecionar o nó do modelo: {err}");
        }
    });
}

/// Despawn the boxes whose real model has appeared.
pub(super) fn retire_boxes(
    mut commands: Commands,
    boxes: Query<(Entity, &ChildOf), With<RetiringBox>>,
    children: Query<&Children>,
) {
    for (entity, parent) in &boxes {
        // The box itself is one child; the scene adds the others.
        if children
            .get(parent.parent())
            .is_ok_and(|children| children.len() >= 2)
        {
            commands.entity(entity).despawn();
        }
    }
}

/// Folder (under the assets folder) where decoded GLBs live.
const CACHE_DIR: &str = ".jackdaw-cache/glb";
const MAX_DECODE_JOBS: usize = 4;
/// Background decodes (cache warm-up) while nothing near the camera waits.
const PREWARM_JOBS: usize = 1;

/// Decode `source` (assets-relative) into the cache, unless a fresh copy is
/// there. Returns the assets-relative path to load.
fn decode_into_cache(assets_root: &Path, source: &str) -> Option<String> {
    let src = assets_root.join(source);
    let cached_rel = format!("{CACHE_DIR}/{source}");
    let dst = assets_root.join(&cached_rel);
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let (Some(dst_time), Some(src_time)) = (modified(&dst), modified(&src))
        && dst_time >= src_time
    {
        return Some(cached_rel);
    }
    let bytes = std::fs::read(&src).ok()?;
    if !bevy_ext_gltf::needs_rewrite(&bytes) {
        // Plain glTF: the editor reads it as is.
        return Some(source.to_string());
    }
    let decoded = match bevy_ext_gltf::decode_glb(&bytes) {
        Ok(decoded) => decoded,
        Err(err) => {
            warn!("preview: {source}: {err:#}");
            return None;
        }
    };
    std::fs::create_dir_all(dst.parent()?).ok()?;
    let tmp = dst.with_extension("glb.tmp");
    std::fs::write(&tmp, decoded).ok()?;
    std::fs::rename(&tmp, &dst).ok()?;
    Some(cached_rel)
}

/// The built terrain preview: the mesh and the line shown in the panel.
struct TerrainBuilt {
    mesh: Mesh,
    status: String,
    /// Albedo do mundo (splat real) quando as texturas das camadas existem.
    albedo: Option<Image>,
}

/// Terrain jobs, live previews and the shared unlit material.
#[derive(Resource, Default)]
pub struct TerrainPreviews {
    by_owner: HashMap<Entity, Entity>,
    specs: HashMap<Entity, u64>,
    jobs: HashMap<Entity, Task<Result<TerrainBuilt, String>>>,
    material: Option<Handle<StandardMaterial>>,
    status: Option<String>,
}

impl TerrainPreviews {
    /// Force every terrain to be reloaded (e.g. after re-baking an `.ahgt`).
    pub fn invalidate(&mut self) {
        self.specs.clear();
    }

    fn has_preview(&self) -> bool {
        !self.by_owner.is_empty()
    }
}

/// Frame the world once, as soon as the terrain preview exists.
///
/// The editor's camera starts at the world origin; the Vael terrain is a
/// world-space heightfield (the city sits around y≈40), so the default view
/// is *under* the ground and reads as "no terrain". Framing the scene gives
/// the overview the editor is for; the user can zoom in from there.
#[derive(Resource, Default)]
pub struct AutoFrame {
    done: bool,
    waited: f32,
}

pub(super) fn frame_world_once(world: &mut World) {
    if world.resource::<AutoFrame>().done {
        return;
    }
    // The scene is loaded once the scan has nodes. Wait for the terrain
    // preview (the normal path), but fall back to framing the props if the
    // heightmap failed or the scene has no terrain.
    let scene_ready = !world.resource::<NodeScan>().tracked.is_empty();
    if !scene_ready {
        return;
    }
    let delta = world.resource::<Time>().delta_secs();
    let has_terrain = world.resource::<TerrainPreviews>().has_preview();
    {
        let mut frame = world.resource_mut::<AutoFrame>();
        frame.waited += delta;
        if !has_terrain && frame.waited < 3.0 {
            return;
        }
        frame.done = true;
    }
    if let Err(err) = world.operator("view.frame_all").call() {
        warn!("preview: não foi possível enquadrar o mundo: {err}");
    }
}

/// The single merged mesh with the models that are far away.
#[derive(Resource, Default)]
pub struct FarMesh {
    entity: Option<Entity>,
    fingerprint: Option<u64>,
}

const TERRAIN_MIN_GRID: usize = 129;
const TERRAIN_MAX_GRID: usize = 513;
const TERRAIN_METERS_PER_VERTEX: f32 = 8.0;

/// Adaptive grid: ~8 m per vertex, odd size, capped so the full-world mesh
/// stays cheap (501² for the 4000 m Vael world).
fn terrain_grid(world_size: f32) -> usize {
    let target = (world_size / TERRAIN_METERS_PER_VERTEX).ceil() as usize;
    let mut grid = target.clamp(TERRAIN_MIN_GRID, TERRAIN_MAX_GRID);
    if grid % 2 == 0 {
        grid += 1;
    }
    grid.min(TERRAIN_MAX_GRID)
}

/// Camera parameters used to turn a node's world radius into apparent pixels.
#[derive(Clone, Copy)]
enum ScreenScale {
    Perspective { viewport_h: f32, tan_half_fov: f32 },
    Orthographic { viewport_h: f32, scale: f32 },
}

impl ScreenScale {
    fn apparent_px(self, radius: f32, distance: f32) -> f32 {
        match self {
            Self::Perspective {
                viewport_h,
                tan_half_fov,
            } => radius * viewport_h * 0.5 / (distance.max(0.1) * tan_half_fov.max(1e-4)),
            Self::Orthographic { viewport_h, scale } => {
                radius * viewport_h / (2.0 * scale.max(1e-4))
            }
        }
    }
}

fn camera_scale(camera: Option<(&GlobalTransform, &Projection, &Camera)>) -> (Vec3, ScreenScale) {
    let Some((transform, projection, camera)) = camera else {
        return (
            Vec3::ZERO,
            ScreenScale::Perspective {
                viewport_h: 1080.0,
                tan_half_fov: 0.4142,
            },
        );
    };
    let viewport_h = camera.logical_viewport_size().map_or(1080.0, |size| size.y);
    let scale = match projection {
        Projection::Perspective(perspective) => ScreenScale::Perspective {
            viewport_h,
            tan_half_fov: (perspective.fov * 0.5).tan(),
        },
        Projection::Orthographic(orthographic) => ScreenScale::Orthographic {
            viewport_h,
            scale: orthographic.scale,
        },
        Projection::Custom(_) => ScreenScale::Perspective {
            viewport_h,
            tan_half_fov: 0.4142,
        },
    };
    (transform.translation(), scale)
}

/// A candidate for an individual preview, before the global caps decide the
/// final tier.
struct Candidate {
    owner: Entity,
    score: f32,
    center: Vec3,
    half: Vec3,
    source: Option<String>,
    category: Category,
}

pub(super) fn refresh_previews(
    mut commands: Commands,
    time: Res<Time>,
    policy: Res<PreviewPolicy>,
    mode: Res<ModelPreviewMode>,
    mut scan: ResMut<NodeScan>,
    mut book: ResMut<PreviewBook>,
    mut bounds: ResMut<BoundsCache>,
    mut terrain: ResMut<TerrainPreviews>,
    mut assets: ResMut<PreviewAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    scene: Option<Res<SceneFilePath>>,
    cameras: Query<(&GlobalTransform, &Projection, &Camera), With<MainViewportCamera>>,
    globals: Query<&GlobalTransform>,
) {
    let Some(root) = project_root(scene.as_deref()) else {
        return;
    };

    // Removals are cheap and should be honoured before the cadence check.
    if !scan.removed.is_empty() {
        for owner in scan.removed.drain(..) {
            if let Some(entry) = book.entries.remove(&owner)
                && commands.get_entity(entry.preview).is_ok()
            {
                commands.entity(entry.preview).despawn();
            }
            book.real_wanted.remove(&owner);
            if let Some(preview) = terrain.by_owner.remove(&owner)
                && commands.get_entity(preview).is_ok()
            {
                commands.entity(preview).despawn();
            }
            terrain.specs.remove(&owner);
        }
        book.force = true;
    }

    // Terrain: start one background job per changed `.ahgt` spec. The decode
    // is a 29 MB inflate plus a 33 MB paint sidecar — too heavy to run inside
    // the editor's Update. This runs even when the model previews are off:
    // the terrain is the world reference the props sit on.
    let terrain_nodes: Vec<(Entity, TerrainSpec)> = scan
        .tracked
        .iter()
        .filter_map(|(entity, node)| node.terrain.as_ref().map(|spec| (*entity, spec.clone())))
        .collect();
    for (owner, spec) in terrain_nodes {
        let digest = spec.digest();
        if terrain.specs.get(&owner) == Some(&digest) || terrain.jobs.contains_key(&owner) {
            continue;
        }
        let Some(path) = resolve_asset(&root, &spec.heightmap) else {
            terrain.specs.insert(owner, digest);
            terrain.status = Some(format!("heightmap não encontrado: {}", spec.heightmap));
            continue;
        };
        terrain.specs.insert(owner, digest);
        let paint = path.with_extension("ahgt.paint");
        let textures = root.join("assets/public/assets/textures");
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { build_terrain(path, paint, spec, textures) });
        terrain.jobs.insert(owner, task);
    }

    if !policy.enabled {
        if !book.entries.is_empty() {
            for (_, entry) in book.entries.drain() {
                if commands.get_entity(entry.preview).is_ok() {
                    commands.entity(entry.preview).despawn();
                }
            }
        }
        book.real_wanted.clear();
        book.force = false;
        return;
    }

    let (camera_pos, screen) = camera_scale(cameras.iter().next());
    let scan_changed = !scan.changed.is_empty();
    let moved = camera_pos.distance_squared(book.last_camera) > policy.camera_move.powi(2);
    let elapsed = time.elapsed_secs() - book.last_refresh;
    if !book.force && !moved && !scan_changed && elapsed < policy.refresh_secs {
        return;
    }
    scan.changed.clear();
    book.force = false;
    book.last_camera = camera_pos;
    book.last_refresh = time.elapsed_secs();

    // Build the candidate list. Every previewable node is scored by apparent
    // pixels; the camera position only decides `distance` (so an orbit zoom
    // out naturally collapses the world into the far mesh).
    let mut candidates: Vec<Candidate> = Vec::new();
    for (owner, node) in &scan.tracked {
        if !node.previewable() || scan.templates.contains(owner) {
            continue;
        }
        let Ok(global) = globals.get(*owner) else {
            continue;
        };
        let Some(source) = bounds.source_for(&root, node) else {
            continue;
        };
        bounds.request(&root, &source);
        let (center, half) = bounds.bounds_or_fallback(&source);
        let distance = global.translation().distance(camera_pos);
        let apparent = screen.apparent_px(half.length().max(0.05), distance);
        let current = book
            .entries
            .get(owner)
            .map_or(Tier::Far, |entry| entry.tier);
        let target = policy.desired_tier(current, apparent);
        if target != current {
            candidates.push(Candidate {
                owner: *owner,
                score: apparent,
                center,
                half,
                source: Some(source),
                category: node.category,
            });
        }
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));

    // Resolve the caps globally so the individual set never exceeds the
    // budget, even when a zoom-in promotes hundreds of nodes at once.
    let mut live = book.entries.len();
    let mut real = book
        .entries
        .values()
        .filter(|entry| entry.tier == Tier::Real)
        .count();
    let mut spawns = 0;
    let mut despawns = 0;
    for candidate in candidates {
        let current = book
            .entries
            .get(&candidate.owner)
            .map(|entry| entry.tier)
            .unwrap_or(Tier::Far);
        let mut target = policy.desired_tier(current, candidate.score);
        if target == Tier::Real && !mode.real {
            target = Tier::Box;
        }
        if target == Tier::Real && real >= policy.max_real {
            target = Tier::Box;
        }
        if target != Tier::Far && live >= policy.max_live {
            target = Tier::Far;
        }

        match (current, target) {
            (Tier::Far, Tier::Far) => {}
            (Tier::Far, Tier::Box | Tier::Real) => {
                if spawns >= policy.spawn_budget {
                    continue;
                }
                let Some(source) = candidate.source.clone() else {
                    continue;
                };
                let entity = spawn_box(
                    &mut commands,
                    &mut assets,
                    &mut meshes,
                    &mut materials,
                    candidate.owner,
                    candidate.center,
                    candidate.half,
                    candidate.category,
                );
                book.entries.insert(
                    candidate.owner,
                    PreviewEntry {
                        preview: entity,
                        tier: Tier::Box,
                        score: candidate.score,
                        source: Some(source),
                    },
                );
                if target == Tier::Real {
                    book.real_wanted.insert(candidate.owner);
                    real += 1;
                }
                live += 1;
                spawns += 1;
            }
            (Tier::Box, Tier::Far) => {
                if despawns >= policy.despawn_budget {
                    continue;
                }
                if let Some(entry) = book.entries.remove(&candidate.owner) {
                    if commands.get_entity(entry.preview).is_ok() {
                        commands.entity(entry.preview).despawn();
                    }
                }
                book.real_wanted.remove(&candidate.owner);
                live = live.saturating_sub(1);
                despawns += 1;
            }
            (Tier::Box, Tier::Box) => {
                book.real_wanted.remove(&candidate.owner);
            }
            (Tier::Box, Tier::Real) => {
                book.real_wanted.insert(candidate.owner);
                if let Some(entry) = book.entries.get_mut(&candidate.owner) {
                    entry.score = candidate.score;
                }
                real += 1;
            }
            (Tier::Real, Tier::Real) => {
                book.real_wanted.remove(&candidate.owner);
                if let Some(entry) = book.entries.get_mut(&candidate.owner) {
                    entry.score = candidate.score;
                }
            }
            (Tier::Real, Tier::Box) => {
                if despawns >= policy.despawn_budget {
                    continue;
                }
                book.real_wanted.remove(&candidate.owner);
                let Some(source) = candidate.source.clone() else {
                    continue;
                };
                if let Some(entry) = book.entries.remove(&candidate.owner)
                    && commands.get_entity(entry.preview).is_ok()
                {
                    commands.entity(entry.preview).despawn();
                }
                if spawns < policy.spawn_budget {
                    let entity = spawn_box(
                        &mut commands,
                        &mut assets,
                        &mut meshes,
                        &mut materials,
                        candidate.owner,
                        candidate.center,
                        candidate.half,
                        candidate.category,
                    );
                    book.entries.insert(
                        candidate.owner,
                        PreviewEntry {
                            preview: entity,
                            tier: Tier::Box,
                            score: candidate.score,
                            source: Some(source),
                        },
                    );
                    spawns += 1;
                } else {
                    live = live.saturating_sub(1);
                }
                real = real.saturating_sub(1);
                despawns += 1;
            }
            (Tier::Real, Tier::Far) => {
                if despawns >= policy.despawn_budget {
                    continue;
                }
                book.real_wanted.remove(&candidate.owner);
                if let Some(entry) = book.entries.remove(&candidate.owner)
                    && commands.get_entity(entry.preview).is_ok()
                {
                    commands.entity(entry.preview).despawn();
                }
                real = real.saturating_sub(1);
                live = live.saturating_sub(1);
                despawns += 1;
            }
        }
    }

    // Drop bookkeeping for previews that no longer exist (the editor can
    // despawn a node's hierarchy without telling us) and for owners that are
    // no longer previewable.
    let mut dead = Vec::new();
    for (owner, entry) in &book.entries {
        let still_previewable = scan
            .tracked
            .get(owner)
            .is_some_and(|node| node.previewable() && !scan.templates.contains(owner));
        if !still_previewable || commands.get_entity(entry.preview).is_err() {
            dead.push(*owner);
        }
    }
    for owner in dead {
        if let Some(entry) = book.entries.remove(&owner)
            && commands.get_entity(entry.preview).is_ok()
        {
            commands.entity(entry.preview).despawn();
        }
        book.real_wanted.remove(&owner);
    }
}

fn spawn_box(
    commands: &mut Commands,
    assets: &mut PreviewAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    owner: Entity,
    center: Vec3,
    half: Vec3,
    category: Category,
) -> Entity {
    let cube = assets.cube(meshes, category);
    let material = assets.cube_material(materials);
    let local = Transform::from_translation(center).with_scale(half.max(Vec3::splat(0.05)) * 2.0);
    commands
        .spawn((
            WorldPreview,
            EditorEntity,
            EditorHidden,
            SkipSerialization,
            Mesh3d(cube),
            MeshMaterial3d(material),
            local,
            Visibility::default(),
            NotShadowCaster,
            ChildOf(owner),
        ))
        .id()
}

/// Start decode jobs for the boxes whose tier is Real, and swap them for the
/// real scene once the GLB is in the cache. Only the near models are decoded;
/// everything else stays a box or part of the far mesh.
pub(super) fn poll_models(
    mut commands: Commands,
    policy: Res<PreviewPolicy>,
    mode: Res<ModelPreviewMode>,
    mut cache: ResMut<GlbCache>,
    mut book: ResMut<PreviewBook>,
    scan: Res<NodeScan>,
    bounds: Res<BoundsCache>,
    scene: Option<Res<SceneFilePath>>,
    asset_server: Res<AssetServer>,
) {
    if !policy.enabled || !mode.real {
        book.real_wanted.clear();
        return;
    }
    let Some(root) = project_root(scene.as_deref()) else {
        return;
    };
    let assets_root = root.join("assets/public");

    let mut finished = Vec::new();
    for (source, task) in cache.jobs.iter_mut() {
        if let Some(result) = block_on(poll_once(task)) {
            finished.push((source.clone(), result));
        }
    }
    for (source, result) in finished {
        cache.jobs.remove(&source);
        cache.ready.insert(source, result);
    }

    let live: HashSet<Entity> = book.entries.keys().copied().collect();
    book.real_wanted.retain(|owner| live.contains(owner));
    let mut wanted: Vec<(Entity, f32, String, Entity)> = book
        .real_wanted
        .iter()
        .filter_map(|owner| {
            let entry = book.entries.get(owner)?;
            if entry.tier != Tier::Box {
                return None;
            }
            let source = entry.source.clone()?;
            Some((*owner, entry.score, source, entry.preview))
        })
        .collect();
    wanted.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut upgrades = 0;
    for (owner, score, source, box_entity) in wanted {
        if upgrades >= policy.upgrade_budget {
            break;
        }
        match cache.ready.get(&source) {
            Some(Some(path)) => {
                let real = commands
                    .spawn((
                        WorldPreview,
                        EditorEntity,
                        EditorHidden,
                        SkipSerialization,
                        WorldAssetRoot(
                            asset_server.load(GltfAssetLabel::Scene(0).from_asset(path.clone())),
                        ),
                        Transform::default(),
                        Visibility::default(),
                        ChildOf(owner),
                    ))
                    .id();
                if commands.get_entity(box_entity).is_ok() {
                    commands
                        .entity(box_entity)
                        .insert((RetiringBox, ChildOf(real)));
                }
                book.entries.insert(
                    owner,
                    PreviewEntry {
                        preview: real,
                        tier: Tier::Real,
                        score,
                        source: Some(source),
                    },
                );
                book.real_wanted.remove(&owner);
                upgrades += 1;
            }
            Some(None) => {
                // Undecodable source: keep the box; do not retry forever.
                book.real_wanted.remove(&owner);
            }
            None => {
                if !cache.jobs.contains_key(&source) && cache.jobs.len() < MAX_DECODE_JOBS {
                    let assets_root = assets_root.clone();
                    let source_for_job = source.clone();
                    let task = AsyncComputeTaskPool::get()
                        .spawn(async move { decode_into_cache(&assets_root, &source_for_job) });
                    cache.jobs.insert(source, task);
                }
            }
        }
    }

    // Warm the cache with the rest of the world while nothing near the
    // camera waits for a decode. `chosen` is filled by the refresh pass, so
    // the queue uses the same LOD the upgrades will ask for.
    if !cache.prewarm_seeded && !bounds.chosen.is_empty() {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        for node in scan.tracked.values() {
            if !node.previewable() {
                continue;
            }
            if let Some(source) = bounds.chosen_source(node)
                && seen.insert(source.to_string())
            {
                queue.push_back(source.to_string());
            }
        }
        cache.prewarm = queue;
        cache.prewarm_seeded = true;
    }
    if book.real_wanted.is_empty() {
        while cache.jobs.len() < PREWARM_JOBS {
            let Some(source) = cache.prewarm.pop_front() else {
                break;
            };
            if cache.ready.contains_key(&source) || cache.jobs.contains_key(&source) {
                continue;
            }
            let assets_root = assets_root.clone();
            let source_for_job = source.clone();
            let task = AsyncComputeTaskPool::get()
                .spawn(async move { decode_into_cache(&assets_root, &source_for_job) });
            cache.jobs.insert(source, task);
        }
    }
}

pub(super) fn poll_terrain(
    mut commands: Commands,
    mut terrain: ResMut<TerrainPreviews>,
    scan: Res<NodeScan>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let material = terrain
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                perceptual_roughness: 1.0,
                ..default()
            })
        })
        .clone();

    let mut finished = Vec::new();
    for (owner, task) in terrain.jobs.iter_mut() {
        if let Some(result) = block_on(poll_once(task)) {
            finished.push((*owner, result));
        }
    }
    for (owner, result) in finished {
        terrain.jobs.remove(&owner);
        let still_terrain = scan
            .tracked
            .get(&owner)
            .is_some_and(|node| node.terrain.is_some());
        if !still_terrain {
            continue;
        }
        match result {
            Ok(built) => {
                let material = match built.albedo {
                    Some(albedo) => materials.add(StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(images.add(albedo)),
                        unlit: true,
                        perceptual_roughness: 1.0,
                        ..default()
                    }),
                    None => material.clone(),
                };
                if let Some(old) = terrain.by_owner.remove(&owner)
                    && commands.get_entity(old).is_ok()
                {
                    commands.entity(old).despawn();
                }
                let preview = commands
                    .spawn((
                        TerrainPreview,
                        EditorEntity,
                        EditorHidden,
                        SkipSerialization,
                        NotShadowCaster,
                        Pickable::IGNORE,
                        Mesh3d(meshes.add(built.mesh)),
                        MeshMaterial3d(material),
                        Transform::default(),
                        Visibility::default(),
                    ))
                    .id();
                terrain.by_owner.insert(owner, preview);
                terrain.status = Some(built.status);
            }
            Err(err) => {
                warn!("preview: terreno {owner}: {err}");
                terrain.status = Some(format!("erro: {err}"));
            }
        }
    }

    // A terrain node that disappeared takes its unparented preview with it.
    let owned: Vec<Entity> = terrain.by_owner.keys().copied().collect();
    for owner in owned {
        let still_terrain = scan
            .tracked
            .get(&owner)
            .is_some_and(|node| node.terrain.is_some());
        if !still_terrain {
            if let Some(preview) = terrain.by_owner.remove(&owner)
                && commands.get_entity(preview).is_ok()
            {
                commands.entity(preview).despawn();
            }
            terrain.specs.remove(&owner);
        }
    }
    if terrain.by_owner.is_empty() && terrain.jobs.is_empty() {
        terrain.status = None;
    }
}

fn build_terrain(
    heightmap: PathBuf,
    paint_path: PathBuf,
    spec: TerrainSpec,
    textures: PathBuf,
) -> Result<TerrainBuilt, String> {
    let field =
        AhgtHeightField::from_path_with(&heightmap, Some(spec.world_size), Some(spec.max_height))
            .map_err(|err| format!("{}: {err:#}", heightmap.display()))?;
    let paint = if paint_path.is_file() {
        match PaintGrid::from_path(&paint_path) {
            Ok(paint) => Some(paint),
            Err(err) => {
                warn!("preview: paint {}: {err:#}", paint_path.display());
                None
            }
        }
    } else {
        None
    };
    let layers = SplatLayers::load(&textures);
    let albedo = layers
        .as_ref()
        .map(|layers| bake_albedo(&field, spec.world_size, paint.as_ref(), layers));
    let (mesh, grid) = terrain_mesh(&field, spec.world_size, paint.as_ref(), albedo.is_some());
    let mut status = format!("{} m, {grid}²", spec.world_size.round());
    if paint.is_some() {
        status.push_str(" + paint");
    }
    if let Some(layers) = &layers {
        status.push_str(&format!(" + splat ({} texturas)", layers.loaded));
    }
    Ok(TerrainBuilt {
        mesh,
        status,
        albedo,
    })
}

pub(super) fn cleanup_previews(
    mut commands: Commands,
    mut book: ResMut<PreviewBook>,
    previews: Query<(), With<WorldPreview>>,
    owners: Query<()>,
) {
    let dead: Vec<Entity> = book
        .entries
        .iter()
        .filter(|(owner, entry)| {
            owners.get(**owner).is_err() || previews.get(entry.preview).is_err()
        })
        .map(|(owner, _)| *owner)
        .collect();
    for owner in dead {
        if let Some(entry) = book.entries.remove(&owner)
            && commands.get_entity(entry.preview).is_ok()
        {
            commands.entity(entry.preview).despawn();
        }
        book.real_wanted.remove(&owner);
    }
}

/// Rebuild the single merged box mesh with every model that is not currently
/// an individual preview. The fingerprint makes the rebuild a no-op when the
/// far set and the transforms did not move.
pub(super) fn rebuild_far_mesh(
    mut commands: Commands,
    policy: Res<PreviewPolicy>,
    scan: Res<NodeScan>,
    book: Res<PreviewBook>,
    bounds: Res<BoundsCache>,
    mut far: ResMut<FarMesh>,
    mut assets: ResMut<PreviewAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    globals: Query<&GlobalTransform>,
) {
    if !policy.enabled {
        if let Some(entity) = far.entity.take()
            && commands.get_entity(entity).is_ok()
        {
            commands.entity(entity).despawn();
        }
        far.fingerprint = None;
        return;
    }

    let mut boxes: Vec<(u64, [f32; 16], [f32; 3], [f32; 3], Category)> = Vec::new();
    for (owner, node) in &scan.tracked {
        if !node.previewable() || scan.templates.contains(owner) || book.entries.contains_key(owner)
        {
            continue;
        }
        let Ok(global) = globals.get(*owner) else {
            continue;
        };
        let Some(source) = bounds.chosen_source(node) else {
            continue;
        };
        let (center, half) = bounds.bounds_or_fallback(source);
        let affine = global.affine()
            * Transform::from_translation(center)
                .with_scale(half.max(Vec3::splat(0.05)) * 2.0)
                .compute_affine();
        boxes.push((
            owner.to_bits(),
            Mat4::from(affine).to_cols_array(),
            center.to_array(),
            half.to_array(),
            node.category,
        ));
    }
    boxes.sort_unstable_by_key(|item| item.0);

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (bits, matrix, center, half, category) in &boxes {
        category.hash(&mut hasher);
        bits.hash(&mut hasher);
        for value in matrix {
            value.to_bits().hash(&mut hasher);
        }
        for value in center.iter().chain(half.iter()) {
            value.to_bits().hash(&mut hasher);
        }
    }
    let fingerprint = hasher.finish();
    if far.fingerprint == Some(fingerprint)
        && far
            .entity
            .is_some_and(|entity| commands.get_entity(entity).is_ok())
    {
        return;
    }

    if boxes.is_empty() {
        if let Some(entity) = far.entity.take()
            && commands.get_entity(entity).is_ok()
        {
            commands.entity(entity).despawn();
        }
        far.fingerprint = Some(fingerprint);
        return;
    }

    let mut positions = Vec::with_capacity(boxes.len() * 24);
    let mut colors = Vec::with_capacity(boxes.len() * 24);
    let mut indices = Vec::with_capacity(boxes.len() * 36);
    for (_, matrix, _, _, category) in &boxes {
        shaded_cube(
            Mat4::from_cols_array(matrix),
            category.color(),
            &mut positions,
            &mut colors,
            &mut indices,
        );
    }
    let mesh = shaded_mesh(positions, colors, indices);
    let mesh = meshes.add(mesh);
    let material = assets.cube_material(&mut materials);

    match far.entity {
        Some(entity) if commands.get_entity(entity).is_ok() => {
            commands.entity(entity).insert(Mesh3d(mesh));
        }
        _ => {
            let entity = commands
                .spawn((
                    FarPreview,
                    EditorEntity,
                    EditorHidden,
                    SkipSerialization,
                    NotShadowCaster,
                    Pickable::IGNORE,
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            far.entity = Some(entity);
        }
    }
    far.fingerprint = Some(fingerprint);
}

pub(super) fn update_status(
    scan: Res<NodeScan>,
    book: Res<PreviewBook>,
    terrain: Res<TerrainPreviews>,
    mut status: ResMut<PreviewStatus>,
) {
    status.tracked = scan.tracked.len();
    status.previewable = scan
        .tracked
        .iter()
        .filter(|(entity, node)| node.previewable() && !scan.templates.contains(*entity))
        .count();
    status.boxes = book
        .entries
        .values()
        .filter(|entry| entry.tier == Tier::Box)
        .count();
    status.real = book
        .entries
        .values()
        .filter(|entry| entry.tier == Tier::Real)
        .count();
    status.far = status.previewable.saturating_sub(book.entries.len());
    status.queued = book.real_wanted.len();
    status.terrain = terrain.status.clone();
}

/// Ordem dos slots do `.paint` (= pastas em `assets/textures/<nome>/`).
const SPLAT_LAYERS: [&str; 13] = [
    "grass",
    "vale_grass",
    "dirt",
    "dirt_trail",
    "forest_floor",
    "gravel",
    "mountain_stone",
    "sand",
    "desert_sand",
    "snow_peak",
    "swamp_mud",
    "dirt_road",
    "pebbles",
];
/// Lado da textura de albedo do mundo (≈2 m por texel no mundo de 4 km).
const SPLAT_SIZE: usize = 2048;
/// Lado das amostras de cada camada (reduzidas de 1–2k).
const LAYER_SAMPLE: usize = 128;
/// Metros por repetição da textura de uma camada.
const LAYER_TILE_M: f32 = 8.0;

/// Albedo de cada camada, reduzido a `LAYER_SAMPLE²` RGB sRGB.
struct SplatLayers {
    texels: Vec<Option<Vec<[u8; 3]>>>,
    loaded: usize,
}

impl SplatLayers {
    fn load(dir: &Path) -> Option<Self> {
        let texels: Vec<Option<Vec<[u8; 3]>>> = SPLAT_LAYERS
            .iter()
            .map(|name| load_layer_albedo(&dir.join(name).join("albedo.webp")))
            .collect();
        let loaded = texels.iter().filter(|t| t.is_some()).count();
        (loaded > 0).then_some(Self { texels, loaded })
    }

    /// Cor da camada `slot` no ponto do mundo (repetida a cada `LAYER_TILE_M`).
    fn sample(&self, slot: usize, x: f32, z: f32) -> [f32; 3] {
        match self.texels.get(slot).and_then(Option::as_ref) {
            Some(texels) => {
                let u = (x / LAYER_TILE_M).rem_euclid(1.0);
                let v = (z / LAYER_TILE_M).rem_euclid(1.0);
                let i = ((u * LAYER_SAMPLE as f32) as usize).min(LAYER_SAMPLE - 1);
                let j = ((v * LAYER_SAMPLE as f32) as usize).min(LAYER_SAMPLE - 1);
                let c = texels[j * LAYER_SAMPLE + i];
                [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
            }
            None => {
                // Sem textura (ex.: `pebbles`): a cor média do runtime, em sRGB.
                let flat = bevy_ext_terrain::paint::SLOT_FLAT[slot.min(12)];
                let c = Color::linear_rgb(flat[0], flat[1], flat[2]).to_srgba();
                [c.red, c.green, c.blue]
            }
        }
    }
}

/// `albedo.webp` → RGB reduzido. (O decoder KTX2/UASTC do Bevy 0.20 rebenta
/// com estas texturas e o pânico atravessa o dylib; o webp é Rust puro.)
fn load_layer_albedo(path: &Path) -> Option<Vec<[u8; 3]>> {
    let bytes = std::fs::read(path).ok()?;
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|err| warn!("preview: splat {}: {err}", path.display()))
        .ok()?;
    let (w, h) = decoder.dimensions();
    let (w, h) = (w as usize, h as usize);
    let channels = if decoder.has_alpha() { 4 } else { 3 };
    let mut data = vec![0_u8; decoder.output_buffer_size()?];
    decoder
        .read_image(&mut data)
        .map_err(|err| warn!("preview: splat {}: {err}", path.display()))
        .ok()?;
    if w == 0 || h == 0 || data.len() < w * h * channels {
        return None;
    }
    let mut out = Vec::with_capacity(LAYER_SAMPLE * LAYER_SAMPLE);
    for j in 0..LAYER_SAMPLE {
        for i in 0..LAYER_SAMPLE {
            let x = i * w / LAYER_SAMPLE;
            let y = j * h / LAYER_SAMPLE;
            let p = (y * w + x) * channels;
            out.push([data[p], data[p + 1], data[p + 2]]);
        }
    }
    Some(out)
}

/// Albedo do mundo: a base procedural do runtime (relva; rocha no declive;
/// neve no alto) com as texturas reais, misturada com o `.paint`.
fn bake_albedo(
    field: &AhgtHeightField,
    world_size: f32,
    paint: Option<&PaintGrid>,
    layers: &SplatLayers,
) -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    // Normais/alturas numa grelha grossa (o declive não precisa de 2 m).
    const COARSE: usize = 257;
    let half = world_size * 0.5;
    let coarse_step = world_size / (COARSE - 1) as f32;
    let mut slope = vec![0.0_f32; COARSE * COARSE];
    let mut height = vec![0.0_f32; COARSE * COARSE];
    for j in 0..COARSE {
        for i in 0..COARSE {
            let x = i as f32 * coarse_step - half;
            let z = j as f32 * coarse_step - half;
            slope[j * COARSE + i] = 1.0 - field.sample_normal(x, z, coarse_step).y;
            height[j * COARSE + i] = field.sample(x, z);
        }
    }
    let texel = world_size / SPLAT_SIZE as f32;
    let mut data = Vec::with_capacity(SPLAT_SIZE * SPLAT_SIZE * 4);
    for j in 0..SPLAT_SIZE {
        let z = (j as f32 + 0.5) * texel - half;
        let cj = (((z + half) / coarse_step).round() as usize).min(COARSE - 1);
        for i in 0..SPLAT_SIZE {
            let x = (i as f32 + 0.5) * texel - half;
            let ci = (((x + half) / coarse_step).round() as usize).min(COARSE - 1);
            let s = slope[cj * COARSE + ci];
            let h = height[cj * COARSE + ci];
            let mix = |a: [f32; 3], b: [f32; 3], t: f32| {
                let t = t.clamp(0.0, 1.0);
                [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ]
            };
            let mut c = layers.sample(0, x, z);
            c = mix(c, layers.sample(6, x, z), (s - 0.12) * 4.0);
            c = mix(c, layers.sample(9, x, z), (h - 150.0) / 40.0);
            if let Some((slot, weight)) = paint.and_then(|paint| paint.sample(x, z)) {
                c = mix(c, layers.sample(slot as usize, x, z), weight);
            }
            data.extend_from_slice(&[
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
                255,
            ]);
        }
    }
    Image::new(
        Extent3d {
            width: SPLAT_SIZE as u32,
            height: SPLAT_SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn terrain_mesh(
    field: &AhgtHeightField,
    world_size: f32,
    paint: Option<&PaintGrid>,
    textured: bool,
) -> (Mesh, usize) {
    let n = terrain_grid(world_size);
    let step = world_size / (n - 1) as f32;
    let half = world_size * 0.5;
    let mut positions = Vec::with_capacity(n * n);
    let mut normals = Vec::with_capacity(n * n);
    let mut uvs = Vec::with_capacity(n * n);
    let mut colors = Vec::with_capacity(n * n);
    let sun = Vec3::new(-0.45, 0.8, -0.4).normalize();
    for z in 0..n {
        for x in 0..n {
            let wx = x as f32 * step - half;
            let wz = z as f32 * step - half;
            let wy = field.sample(wx, wz);
            positions.push([wx, wy, wz]);
            let normal = field.sample_normal(wx, wz, step);
            normals.push(normal.to_array());
            if textured {
                // A cor vem da textura do splat; o vértice só leva a luz
                // fixa (material unlit: o editor não tem o sol do jogo).
                let light = 0.35 + 0.65 * normal.dot(sun).max(0.0);
                colors.push([light, light, light, 1.0]);
                uvs.push([(wx + half) / world_size, (wz + half) / world_size]);
                continue;
            }
            let base = terrain_color(wy, normal, sun);
            let color = match paint {
                Some(paint) => paint.tint(wx, wz, [base[0], base[1], base[2]], 0.9),
                None => [base[0], base[1], base[2]],
            };
            colors.push([color[0], color[1], color[2], 1.0]);
            uvs.push([wx / 8.0, wz / 8.0]);
        }
    }
    let mut indices = Vec::with_capacity((n - 1) * (n - 1) * 6);
    for z in 0..n - 1 {
        for x in 0..n - 1 {
            let i = (z * n + x) as u32;
            let right = i + 1;
            let down = i + n as u32;
            let down_right = down + 1;
            indices.extend_from_slice(&[i, down, right, right, down, down_right]);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    (mesh, n)
}

/// Slope/height tint with a fixed sun, so the unlit preview reads as relief.
fn terrain_color(height: f32, normal: Vec3, sun: Vec3) -> [f32; 4] {
    let grass = Vec3::new(0.40, 0.50, 0.30);
    let rock = Vec3::new(0.50, 0.47, 0.42);
    let snow = Vec3::new(0.90, 0.92, 0.95);
    let slope = (1.0 - normal.y).clamp(0.0, 1.0);
    let mut base = grass.lerp(rock, (slope * 3.0).clamp(0.0, 1.0));
    base = base.lerp(snow, ((height - 150.0) / 40.0).clamp(0.0, 1.0));
    let light = 0.35 + 0.65 * normal.dot(sun).max(0.0);
    let c = base * light;
    [c.x, c.y, c.z, 1.0]
}

fn project_root(scene: Option<&SceneFilePath>) -> Option<PathBuf> {
    if let Some(scene) = scene.and_then(|scene| scene.path.as_deref()) {
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

fn resolve_asset(root: &Path, url: &str) -> Option<PathBuf> {
    let clean = url.split('#').next().unwrap_or(url).trim_start_matches('/');
    [root.join(clean), root.join("assets/public").join(clean)]
        .into_iter()
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_have_hysteresis() {
        let policy = PreviewPolicy {
            real_px: 14.0,
            box_px: 0.6,
            ..PreviewPolicy::default()
        };
        assert_eq!(policy.desired_tier(Tier::Far, 20.0), Tier::Real);
        assert_eq!(policy.desired_tier(Tier::Far, 1.0), Tier::Box);
        assert_eq!(policy.desired_tier(Tier::Far, 0.1), Tier::Far);
        // A real preview survives down to 80% of the promotion threshold.
        assert_eq!(policy.desired_tier(Tier::Real, 11.5), Tier::Real);
        assert_eq!(policy.desired_tier(Tier::Real, 8.0), Tier::Box);
        assert_eq!(policy.desired_tier(Tier::Box, 13.0), Tier::Box);
        assert_eq!(policy.desired_tier(Tier::Box, 14.0), Tier::Real);
    }

    #[test]
    fn terrain_grid_tracks_world_size_and_stays_odd() {
        assert_eq!(terrain_grid(4000.0), 501);
        assert_eq!(terrain_grid(220.0), 129);
        assert_eq!(terrain_grid(10_000.0), 513);
        assert_eq!(terrain_grid(1000.0) % 2, 1);
    }

    #[test]
    fn scatter_templates_are_not_previewable() {
        let node = WorldNode {
            tag: "GLTFLoader".into(),
            attrs: vec![bevy_ext_world::bsn::WorldProp {
                key: "url".into(),
                value: "/assets/meshes/forest/pine_dark_lod0.glb".into(),
            }],
            ..default()
        };
        let tracked = TrackedNode::from_node(&node);
        assert!(tracked.previewable());
        assert_eq!(
            tracked.url.as_deref(),
            Some("assets/meshes/forest/pine_dark_lod0.glb")
        );
    }
}
