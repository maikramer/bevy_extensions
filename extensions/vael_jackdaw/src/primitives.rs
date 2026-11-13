//! Gradmesh preview for authored primitives in the Jackdaw viewport.
//!
//! The world's props are `<Composition>` parts (`Box`, `Cylinder`, `Lathe`, …)
//! stored as `WorldNode` tag/attrs, not as GLBs — the voxel-terrain world
//! preview only knows GLB/terrain nodes and drew nothing for them. This module
//! turns every primitive node into a child preview mesh using the SAME
//! conversion as the game (`bevy_ext_gradmesh`: shape + style preset + baked
//! vertical gradient), so the viewport tells the truth about the final look.
//!
//! Like the other previews, the generated children carry `EditorHidden` /
//! `SkipSerialization` and never enter the `.bsn`. The `WorldNode` type is read
//! by reflection (each dylib has its own copy of the type).

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use bevy::ecs::reflect::ReflectComponent;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::reflect::{PartialReflect, ReflectRef};
use jackdaw_extension::AppState;
use jackdaw_extension::EditorEntity;
use jackdaw_extension::prelude::*;
use jackdaw_extension::scene::{EditorHidden, SkipSerialization};

use bevy_ext_gradmesh::{MaterialSpec, Shape};

/// `WorldNode` type path (registered by the terrain extension).
const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
/// Rescan cadence: the editor rewrites attrs through the inspector, and a
/// primitive mesh is cheap to rebuild (the heavy data is cached by shape).
const SCAN_INTERVAL: f32 = 0.5;

#[derive(Resource)]
struct PrimitivePreviewState {
    timer: f32,
    detail: bevy_gradmesh::ShapeDetail,
    /// One child preview per authored primitive node.
    entries: HashMap<Entity, Entry>,
    /// Mesh cache keyed by the procedural shape (shape + detail).
    meshes: HashMap<bevy_gradmesh::Shape, Handle<Mesh>>,
    /// Material cache keyed by the material fingerprint.
    materials: HashMap<u64, Handle<bevy_gradmesh::GradMeshMaterial>>,
}

impl Default for PrimitivePreviewState {
    fn default() -> Self {
        Self {
            timer: 0.0,
            detail: bevy_gradmesh::ShapeDetail::High,
            entries: HashMap::new(),
            meshes: HashMap::new(),
            materials: HashMap::new(),
        }
    }
}

struct Entry {
    child: Entity,
    fingerprint: u64,
}

/// Marks the preview mesh child (so it can be found and despawned).
#[derive(Component)]
struct PrimitivePreview;

/// Registers the gradmesh material/plugin and the preview system.
pub(crate) fn app(app: &mut App) {
    if !app.is_plugin_added::<bevy_gradmesh::GradMeshPlugin>() {
        app.add_plugins(bevy_gradmesh::GradMeshPlugin);
    }
    app.init_resource::<PrimitivePreviewState>()
        .add_systems(Update, refresh.run_if(in_state(AppState::Editor)))
        .add_observer(redirect_selection);
}

#[allow(clippy::too_many_arguments)]
fn refresh(
    time: Res<Time>,
    mut state: ResMut<PrimitivePreviewState>,
    registry: Res<AppTypeRegistry>,
    // Os recursos são entidades no Bevy 0.20: sem o filtro, o `EntityRef`
    // colide com o `ResMut<PrimitivePreviewState>` (B0001).
    nodes: Query<EntityRef, Without<bevy::ecs::resource::IsResource>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<bevy_gradmesh::GradMeshMaterial>>,
) {
    state.timer -= time.delta_secs();
    if state.timer > 0.0 {
        return;
    }
    state.timer = SCAN_INTERVAL;

    let registry = registry.read();
    let Some(component) = registry
        .get_with_type_path(WORLD_NODE)
        .and_then(|registration| registration.data::<ReflectComponent>())
    else {
        return;
    };

    // Prototypes/Scatter recipes are templates: their primitive children sit
    // at the template's origin and must not be previewed as placed props.
    let mut templates: HashSet<Entity> = HashSet::new();
    for entity_ref in nodes.iter() {
        let Some(node) = component.reflect(entity_ref) else {
            continue;
        };
        if let Some(tag) = reflected_str(node.as_partial_reflect(), "tag")
            && (tag.eq_ignore_ascii_case("prototype") || tag.eq_ignore_ascii_case("scatter"))
        {
            templates.insert(entity_ref.id());
        }
    }

    let mut seen: HashSet<Entity> = HashSet::new();
    for entity_ref in nodes.iter() {
        let Some(node) = component.reflect(entity_ref) else {
            continue;
        };
        let Some(tag) = reflected_str(node.as_partial_reflect(), "tag") else {
            continue;
        };
        if !bevy_ext_gradmesh::is_primitive_tag(&tag) {
            continue;
        }
        let entity = entity_ref.id();
        if under_template(entity, &templates, &parents) {
            continue;
        }
        // A tag can be a primitive with broken geometry while the user edits
        // (e.g. a Lathe before its profile is filled): skip it, and let the
        // stale cleanup below drop any old preview.
        let Some(shape) = bevy_ext_gradmesh::shape_from_attrs(&tag, |key| {
            reflected_attr(node.as_partial_reflect(), key)
        }) else {
            continue;
        };
        let material = bevy_ext_gradmesh::material_from_attrs(|key| {
            reflected_attr(node.as_partial_reflect(), key)
        });
        seen.insert(entity);
        let fingerprint = fingerprint(&shape, &material);
        if state
            .entries
            .get(&entity)
            .is_some_and(|entry| entry.fingerprint == fingerprint)
        {
            continue;
        }
        if let Some(entry) = state.entries.remove(&entity)
            && commands.get_entity(entry.child).is_ok()
        {
            commands.entity(entry.child).despawn();
        }
        let Some(mesh) = mesh_handle(&mut state, &mut meshes, &shape) else {
            continue;
        };
        let material = material_handle(&mut state, &mut materials, &material);
        let child = commands
            .spawn((
                PrimitivePreview,
                EditorEntity,
                EditorHidden,
                SkipSerialization,
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::default(),
                Visibility::Inherited,
                NotShadowCaster,
                ChildOf(entity),
            ))
            .id();
        state.entries.insert(entity, Entry { child, fingerprint });
    }

    // Drop previews whose node disappeared, became non-primitive or failed to
    // parse. Despawning a node takes its children with it, so check liveness.
    let stale: Vec<Entity> = state
        .entries
        .keys()
        .filter(|entity| !seen.contains(entity))
        .copied()
        .collect();
    for entity in stale {
        if let Some(entry) = state.entries.remove(&entity)
            && commands.get_entity(entry.child).is_ok()
        {
            commands.entity(entry.child).despawn();
        }
    }
}

fn mesh_handle(
    state: &mut PrimitivePreviewState,
    meshes: &mut Assets<Mesh>,
    shape: &Shape,
) -> Option<Handle<Mesh>> {
    let grad_shape = bevy_ext_gradmesh::gradmesh_shape(shape, state.detail)?;
    if let Some(handle) = state.meshes.get(&grad_shape) {
        return Some(handle.clone());
    }
    let mut mesh = grad_shape.build();
    bevy_ext_gradmesh::bake_vertical_gradient(&mut mesh, bevy_ext_gradmesh::VERTEX_GRADIENT_DARK);
    let handle = meshes.add(mesh);
    state.meshes.insert(grad_shape, handle.clone());
    Some(handle)
}

fn material_handle(
    state: &mut PrimitivePreviewState,
    materials: &mut Assets<bevy_gradmesh::GradMeshMaterial>,
    material: &MaterialSpec,
) -> Handle<bevy_gradmesh::GradMeshMaterial> {
    let key = material_fingerprint(material);
    if let Some(handle) = state.materials.get(&key) {
        return handle.clone();
    }
    let handle = materials.add(bevy_ext_gradmesh::style(material).build());
    state.materials.insert(key, handle.clone());
    handle
}

fn fingerprint(shape: &Shape, material: &MaterialSpec) -> u64 {
    let mut hasher = DefaultHasher::new();
    format!("{shape:?}|{material:?}").hash(&mut hasher);
    hasher.finish()
}

fn material_fingerprint(material: &MaterialSpec) -> u64 {
    let mut hasher = DefaultHasher::new();
    format!("{material:?}").hash(&mut hasher);
    hasher.finish()
}

/// Clicking a preview mesh selects the authored `WorldNode`, not the
/// editor-hidden child (same contract as the voxel-terrain world preview).
fn redirect_selection(
    added: On<Add<jackdaw_extension::selection::Selected>>,
    previews: Query<(), With<PrimitivePreview>>,
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
        if let Err(err) = world
            .operator("selection.select")
            .param("entity", owner)
            .call()
        {
            warn!("gradmesh: não foi possível selecionar o nó da primitiva: {err}");
        }
    });
}

/// Whether `entity` is inside a `Prototype`/`Scatter` template subtree.
fn under_template(entity: Entity, templates: &HashSet<Entity>, parents: &Query<&ChildOf>) -> bool {
    let mut current = entity;
    for _ in 0..64 {
        let Ok(parent) = parents.get(current) else {
            return false;
        };
        current = parent.parent();
        if templates.contains(&current) {
            return true;
        }
    }
    false
}

/// `WorldNode.tag` by reflection.
fn reflected_str(node: &dyn PartialReflect, key: &str) -> Option<String> {
    let ReflectRef::Struct(node) = node.reflect_ref() else {
        return None;
    };
    node.field(key)
        .and_then(|field| field.try_downcast_ref::<String>())
        .cloned()
}

/// `WorldNode.attrs[key == …].value` by reflection.
fn reflected_attr(node: &dyn PartialReflect, key: &str) -> Option<String> {
    let ReflectRef::Struct(node) = node.reflect_ref() else {
        return None;
    };
    let ReflectRef::List(attrs) = node.field("attrs")?.reflect_ref() else {
        return None;
    };
    attrs.iter().find_map(|prop| {
        let ReflectRef::Struct(prop) = prop.reflect_ref() else {
            return None;
        };
        let text = |name: &str| {
            prop.field(name)
                .and_then(|field| field.try_downcast_ref::<String>())
                .cloned()
        };
        (text("key")?.as_str() == key)
            .then(|| text("value"))
            .flatten()
    })
}
