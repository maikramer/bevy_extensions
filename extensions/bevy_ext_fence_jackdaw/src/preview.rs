//! Preview das cercas no viewport do Jackdaw.
//!
//! Cada entidade com `FenceSpec` ganha filhos ocultos (`EditorHidden` /
//! `SkipSerialization`) com a malha que o jogo vai gerar: o mesmo
//! [`bevy_ext_fence::layout`] + [`bevy_ext_fence::mesh`] do crate, com as
//! texturas do pool — o editor e o jogo não podem discordar do aspecto. Mover
//! um ponto ou mudar o estilo refaz a cerca (varredura de 0,5 s).
//!
//! Sem chão: o viewport do editor não tem o terreno do jogo, e os postes
//! ficam à cota autorada (no jogo eles descem até ao chão).
//!
//! Os componentes são lidos **por reflexão**, pelo caminho do tipo: cada dylib
//! tem a sua cópia e só um regista o tipo no editor — um `Query<&FenceSpec>`
//! só via a entidade que recebeu a cópia *deste* dylib.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use bevy::asset::{AssetId, LoadState};
use bevy::ecs::reflect::ReflectComponent;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::reflect::{FromReflect, TypeRegistry};
use bevy_ext_fence::material::FenceMaterials;
use bevy_ext_fence::{FenceSettings, FenceSpec, FenceStyle, layout, mesh, points, resolve, seed_key};
use bevy_ext_gltf::textures::WorldTiledTextures;
use jackdaw_extension::AppState;
use jackdaw_extension::EditorEntity;
use jackdaw_extension::prelude::*;
use jackdaw_extension::scene::{EditorHidden, SkipSerialization};
use jackdaw_extension::selection::Selection;

/// Caminhos dos tipos lidos por reflexão.
pub(crate) const SPEC: &str = "bevy_ext_scene::FenceSpec";
pub(crate) const STYLE: &str = "bevy_ext_fence::FenceStyle";
pub(crate) const SETTINGS: &str = "bevy_ext_fence::FenceSettings";

/// Cadência da varredura: o editor reescreve campos pelo inspector e uma
/// cerca é barata de refazer.
const SCAN_INTERVAL: f32 = 0.5;
/// Onde estão os conjuntos de texturas (a raiz que o jogo usa).
const TEXTURE_ROOT: &str = "/assets/textures/";

#[derive(Resource)]
pub(crate) struct FencePreviews {
    timer: f32,
    entries: HashMap<Entity, Entry>,
    /// A extensão dos mapas: `ktx2` como no jogo; `webp` se o carregador do
    /// editor não souber ler KTX2.
    ext: &'static str,
    probe: Option<AssetId<Image>>,
    pub status: String,
}

impl Default for FencePreviews {
    fn default() -> Self {
        Self {
            timer: 0.0,
            entries: HashMap::new(),
            ext: "ktx2",
            probe: None,
            status: String::new(),
        }
    }
}

struct Entry {
    children: Vec<Entity>,
    fingerprint: u64,
}

/// Marca a malha de preview (para a limpar e para a selecção passar ao nó).
#[derive(Component)]
struct FencePreview;

/// O componente `type_path` da entidade, convertido para a cópia deste dylib.
pub(crate) fn reflected<T: FromReflect>(registry: &TypeRegistry, entity: &EntityRef, type_path: &str) -> Option<T> {
    let registration = registry.get_with_type_path(type_path)?;
    let component = registration.data::<ReflectComponent>()?;
    T::from_reflect(component.reflect(entity)?)
}

/// As predefinições da cena (`world/settings/fences.bsn`), por ordem estável.
pub(crate) fn styles(registry: &TypeRegistry, nodes: &Query<EntityRef, With<GlobalTransform>>) -> Vec<FenceStyle> {
    let mut found: Vec<(Entity, FenceStyle)> = nodes
        .iter()
        .filter_map(|entity| reflected::<FenceStyle>(registry, &entity, STYLE).map(|style| (entity.id(), style)))
        .collect();
    found.sort_by_key(|(entity, _)| entity.index());
    found.into_iter().map(|(_, style)| style).collect()
}

/// As cercas da cena: a entidade e o pedido, por ordem estável.
pub(crate) fn fences(registry: &TypeRegistry, nodes: &Query<EntityRef, With<GlobalTransform>>) -> Vec<(Entity, FenceSpec)> {
    let mut found: Vec<(Entity, FenceSpec)> = nodes
        .iter()
        .filter_map(|entity| reflected::<FenceSpec>(registry, &entity, SPEC).map(|spec| (entity.id(), spec)))
        .collect();
    found.sort_by_key(|(entity, _)| entity.index());
    found
}

pub(crate) fn settings(registry: &TypeRegistry, nodes: &Query<EntityRef, With<GlobalTransform>>) -> FenceSettings {
    nodes
        .iter()
        .filter_map(|entity| reflected::<FenceSettings>(registry, &entity, SETTINGS))
        .next()
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn refresh(
    time: Res<Time>,
    mut state: ResMut<FencePreviews>,
    registry: Res<AppTypeRegistry>,
    server: Res<AssetServer>,
    mut tiled: Option<ResMut<WorldTiledTextures>>,
    mut materials: ResMut<FenceMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    nodes: Query<EntityRef, With<GlobalTransform>>,
    mut commands: Commands,
) {
    state.timer -= time.delta_secs();
    if state.timer > 0.0 {
        return;
    }
    state.timer = SCAN_INTERVAL;

    // O editor lê KTX2? Sonda uma vez com o albedo do carvalho; se falhar, os
    // mapas passam a vir dos WebP irmãos (o script gera sempre as duas).
    if state.probe.is_none() {
        state.probe = Some(
            server
                .load::<Image>(format!("{}wood_oak_hewn/albedo.ktx2", TEXTURE_ROOT.trim_start_matches('/')))
                .id(),
        );
    }
    if let Some(probe) = state.probe
        && state.ext == "ktx2"
        && matches!(server.get_load_state(probe), Some(LoadState::Failed(_)))
    {
        warn!("cercas: o editor não lê KTX2 — o preview usa os WebP do pool");
        state.ext = "webp";
        materials.clear();
        for entry in state.entries.values() {
            for child in &entry.children {
                commands.entity(*child).despawn();
            }
        }
        state.entries.clear();
    }

    let registry = registry.read();
    let styles = styles(&registry, &nodes);
    let settings = settings(&registry, &nodes);
    let defaults = settings.clone();
    let specs = fences(&registry, &nodes);
    let count = specs.len();
    let mut seen: Vec<Entity> = Vec::with_capacity(count);
    for (entity, spec) in &specs {
        let (entity, spec) = (*entity, spec);
        seen.push(entity);
        let Ok(entity_ref) = nodes.get(entity) else {
            continue;
        };
        let key = spec.style.clone().unwrap_or_else(|| defaults.default_style.clone());
        let Some(style) = bevy_ext_fence::find(&styles, &key) else {
            continue;
        };
        let Some(global) = entity_ref.get::<GlobalTransform>() else {
            continue;
        };
        let name = entity_ref.get::<Name>();
        let frame = bevy_ext_fence::frame_of(global);
        let style = resolve(&spec, style);
        let fingerprint = fingerprint(&spec, &style, &frame);
        if state.entries.get(&entity).is_some_and(|entry| entry.fingerprint == fingerprint) {
            continue;
        }
        // Refazer: a malha antiga sai (os filhos foram todos criados aqui).
        if let Some(entry) = state.entries.remove(&entity) {
            for child in entry.children {
                if commands.get_entity(child).is_ok() {
                    commands.entity(child).despawn();
                }
            }
        }
        let key = seed_key(name, &spec);
        let ground = |_: f32, _: f32| None;
        let Ok(fence) = layout::layout(&spec, &style, &frame, &ground, key) else {
            continue;
        };
        let iron = materials.iron(defaults.iron_color, &mut standard);
        let mut handles: [Option<Handle<StandardMaterial>>; 3] = [None, None, Some(iron)];
        let mut load = |set: &str, materials: &mut FenceMaterials, tiled: &mut Option<ResMut<WorldTiledTextures>>| {
            materials.wood_from(set, defaults.materials_root.as_str(), state.ext, &server, tiled.as_mut().map(|t| t.reborrow()), &mut standard)
        };
        handles[0] = Some(load(style.material.as_str(), &mut materials, &mut tiled));
        handles[1] = Some(if style.secondary_material.trim().is_empty() {
            handles[0].clone().unwrap()
        } else {
            load(style.secondary_material.as_str(), &mut materials, &mut tiled)
        });
        let mut children = Vec::new();
        for (slot, surface) in mesh::build(&fence, &style) {
            let Some(material) = handles[slot as usize].clone() else {
                continue;
            };
            children.push(
                commands
                    .spawn((
                        FencePreview,
                        EditorEntity,
                        EditorHidden,
                        SkipSerialization,
                        Mesh3d(meshes.add(surface.to_mesh())),
                        MeshMaterial3d(material),
                        Transform::default(),
                        Visibility::Inherited,
                        NotShadowCaster,
                        ChildOf(entity),
                    ))
                    .id(),
            );
        }
        state.entries.insert(entity, Entry { children, fingerprint });
    }

    // As cercas que desapareceram levam o preview consigo (apagar um nó leva
    // os filhos; conferir se ainda existem).
    let stale: Vec<Entity> = state.entries.keys().copied().filter(|entity| !seen.contains(entity)).collect();
    for entity in stale {
        if let Some(entry) = state.entries.remove(&entity) {
            for child in entry.children {
                if commands.get_entity(child).is_ok() {
                    commands.entity(child).despawn();
                }
            }
        }
    }
    let text = format!(
        "{} predefinição(ões) · {} cerca(s) na cena · mapas {}",
        styles.len(),
        count,
        state.ext
    );
    if state.status != text {
        state.status = text;
    }
}

/// O que faz a cerca: o pedido, o estilo já com as sobreposições e a moldura.
fn fingerprint(spec: &FenceSpec, style: &FenceStyle, frame: &bevy_ext_fence::FenceFrame) -> u64 {
    let mut hasher = DefaultHasher::new();
    (format!("{spec:?}"), format!("{style:?}")).hash(&mut hasher);
    for value in [frame.origin.x, frame.origin.y, frame.origin.z, frame.yaw, frame.scale] {
        (value * 1000.0).round().to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// A linha e os pontos da cerca seleccionada (só desenha; não entra no documento).
pub(crate) fn draw_gizmos(
    mut gizmos: Gizmos,
    registry: Res<AppTypeRegistry>,
    selection: Option<Res<Selection>>,
    nodes: Query<EntityRef, With<GlobalTransform>>,
) {
    let Some(selected) = selection.map(|selection| selection.entities.clone()) else {
        return;
    };
    let registry = registry.read();
    let line = Color::srgb(0.91, 0.79, 0.53);
    let joint = Color::srgb(0.85, 0.47, 0.18);
    for entity in selected {
        let Ok(entity_ref) = nodes.get(entity) else {
            continue;
        };
        let Some(spec) = reflected::<FenceSpec>(&registry, &entity_ref, SPEC) else {
            continue;
        };
        let Some(global) = entity_ref.get::<GlobalTransform>() else {
            continue;
        };
        let Some(points) = points::parse_points(&spec.points) else {
            continue;
        };
        let mut world: Vec<Vec3> = points.iter().map(|point| global.transform_point(*point)).collect();
        if spec.closed.unwrap_or(false) && world.len() > 2 {
            world.push(world[0]);
        }
        gizmos.linestrip(world.iter().copied(), line);
        for point in world {
            gizmos.sphere(point, 0.12, joint);
        }
    }
}

/// Clicar na malha de preview selecciona o nó autorado, não o filho oculto.
fn redirect_selection(
    added: On<Add<jackdaw_extension::selection::Selected>>,
    previews: Query<(), With<FencePreview>>,
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
            warn!("cercas: não foi possível seleccionar o nó da cerca: {err}");
        }
    });
}

pub(crate) fn app(app: &mut App) {
    if !app.is_plugin_added::<bevy_ext_gltf::textures::TexturesPlugin>() {
        app.add_plugins(bevy_ext_gltf::textures::TexturesPlugin);
    }
    app.init_resource::<FenceMaterials>()
        .init_resource::<FencePreviews>()
        .add_systems(Update, (refresh, draw_gizmos).run_if(in_state(AppState::Editor)))
        .add_observer(redirect_selection);
}
