//! Painel "Vael: Flora": espécies (`FloraSpecies`) com preview real do
//! `bevy_flora` e as árvores das estações (`prop-flora`) no viewport.
//!
//! - **Pré-visualizar espécies**: uma fila por espécie com as N variantes,
//!   em frente à câmara; reconstrói quando um campo muda no inspector.
//! - **Árvores nas estações**: cada `WorldNode` com `prop-flora` ganha um
//!   filho `EditorHidden` com a árvore do pool da espécie (o mesmo pool, LOD
//!   e look frio que o jogo usa), sem impostores.
//!
//! Nada disto entra no `.bsn`: são filhos/entidades `EditorHidden`.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_flora::prelude::{
    LodDistances, LodSettings, PoolColors, PooledLook, PooledTree, ProceduralTree, TreePool,
    build_pool,
};
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;
use jackdaw_extension::scene::EditorHidden;
use jackdaw_extension::viewport::MainViewportCamera;

use bevy::ecs::reflect::ReflectComponent;
use bevy::reflect::ReflectRef;

use bevy_ext_flora::{FloraSettings, FloraSpecies};

use crate::ui;

pub const WINDOW_ID: &str = "vael.flora";
const SPECIES_TYPE: &str = "bevy_ext_flora::FloraSpecies";
/// Estações plantadas por frame (o mundo tem ~1600).
const BATCH: usize = 256;
/// Só se plantam estações dentro deste raio da câmara do viewport.
const STATION_RADIUS: f32 = 250.0;
/// Histerese ao desplantar, para não oscilar no limiar.
const STATION_HYSTERESIS: f32 = 1.25;
/// O `WorldNode` é registado pela extensão do terreno; cada dylib tem a sua
/// cópia do tipo, por isso aqui lê-se por reflexão (pelo type path).
const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
const FLORA_STATION: &str = "bevy_ext_scene::FloraStation";

/// Estações (`prop-flora`) da cena, indexadas por reflexão.
#[derive(Resource, Default)]
pub(crate) struct StationIndex {
    stations: Vec<(Entity, String)>,
    counts: HashMap<String, usize>,
    /// Nº de entidades da última indexação (re-indexa quando muda).
    entities: usize,
    timer: f32,
}

#[derive(Component)]
pub(crate) struct FloraStatus;

#[derive(Resource, Default)]
pub(crate) struct FloraPreview {
    /// Fila de espécies em frente à câmara.
    species: bool,
    /// Onde a fila nasce (fixada ao ligar).
    anchor: Option<Transform>,
    rebuild_species: bool,
    /// Árvores nas estações do mundo.
    stations: bool,
    pools: HashMap<String, (Handle<TreePool>, u32)>,
    /// Estação → árvore plantada; permite desplantar as que ficam longe.
    planted: HashMap<Entity, Entity>,
    rebuild_pools: bool,
}

/// Árvore da fila de preview (sem pai: fica onde a câmara estava).
#[derive(Component)]
struct SpeciesPreviewTree;

/// Árvore filha de uma estação.
#[derive(Component)]
struct StationTree;

pub(crate) fn app(app: &mut App) {
    app.init_resource::<FloraPreview>()
        .init_resource::<StationIndex>()
        .add_systems(
            Update,
            (
                index_stations,
                species_preview,
                station_pools,
                plant_stations,
            )
                .chain()
                .run_if(in_state(jackdaw_extension::AppState::Editor)),
        );
}

pub(crate) fn build(world: &mut World, parent: Entity) {
    ui::column(world, parent, |root| {
        ui::heading(root, "Flora (bevy_flora)");
        ui::status(root, FloraStatus);
        ui::row(
            root,
            &[
                ("Pré-visualizar espécies", "vael.flora.toggle_species"),
                ("Árvores nas estações", "vael.flora.toggle_stations"),
            ],
        );
        ui::row(
            root,
            &[
                ("Nova espécie", "vael.flora.add_species"),
                ("Reconstruir", "vael.flora.rebuild"),
            ],
        );
        ui::label(
            root,
            "Cada espécie é uma entidade em Definições › Flora: seleciona-a e edita \
             `params` (tronco, ramos, folhagem, cores) no inspector — o preview \
             reconstrói. `key` é o `prop-flora` das estações. Grava e o jogo \
             reconstrói os pools.",
        );
    });
}

pub(crate) fn refresh(
    species: Query<&FloraSpecies>,
    changed: Query<(), Changed<FloraSpecies>>,
    index: Res<StationIndex>,
    preview: Res<FloraPreview>,
    mut status: Query<&mut Text, With<FloraStatus>>,
    mut last: Local<(usize, usize, bool, bool, usize)>,
) {
    let Ok(mut status) = status.single_mut() else {
        return;
    };
    let species_count = species.iter().count();
    let key = (
        species_count,
        index.stations.len(),
        preview.species,
        preview.stations,
        preview.planted.len(),
    );
    if *last == key && changed.is_empty() && status.0 != "…" {
        return;
    }
    *last = key;
    let stations: HashMap<&str, usize> = index
        .counts
        .iter()
        .map(|(prop, count)| (prop.as_str(), *count))
        .collect();
    let mut lines = Vec::new();
    if species_count == 0 {
        lines.push("Sem FloraSpecies nesta cena (abre o world.bsn).".to_string());
    } else {
        let mut list: Vec<String> = species
            .iter()
            .map(|s| {
                format!(
                    "{} ×{} — {} estações",
                    if s.key.is_empty() {
                        "(sem key)"
                    } else {
                        &s.key
                    },
                    s.variants,
                    stations.get(s.key.as_str()).copied().unwrap_or(0)
                )
            })
            .collect();
        list.sort();
        lines.push(format!("{species_count} espécies:"));
        lines.extend(list);
    }
    let orphan: Vec<&str> = stations
        .keys()
        .copied()
        .filter(|prop| !species.iter().any(|s| s.key == *prop))
        .collect();
    if !orphan.is_empty() {
        lines.push(format!("⚠ estações sem espécie: {}", orphan.join(", ")));
    }
    lines.push(format!(
        "Preview: espécies {} · estações {} ({} plantadas)",
        if preview.species {
            "ligado"
        } else {
            "desligado"
        },
        if preview.stations {
            "ligado"
        } else {
            "desligado"
        },
        preview.planted.len()
    ));
    status.0 = lines.join("\n");
}

/// Indexa as estações por reflexão quando o nº de entidades muda (abrir ou
/// trocar de cena) ou a cada 3 s.
fn index_stations(world: &mut World) {
    let entities = world.entities().len() as usize;
    let delta = world.resource::<Time>().delta_secs();
    {
        let mut index = world.resource_mut::<StationIndex>();
        index.timer -= delta;
        if index.entities == entities && index.timer > 0.0 {
            return;
        }
        index.entities = entities;
        index.timer = 3.0;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let component = registry
        .get_with_type_path(WORLD_NODE)
        .and_then(|registration| registration.data::<ReflectComponent>());
    // A estação tipada (`bevy_ext_scene::FloraStation`) é a fonte; o attr
    // `prop-flora` do `WorldNode` só em cenas por migrar.
    let station = registry
        .get_with_type_path(FLORA_STATION)
        .and_then(|registration| registration.data::<ReflectComponent>());
    if component.is_none() && station.is_none() {
        return;
    }
    let mut stations = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut query = world.query::<EntityRef>();
    for entity in query.iter(world) {
        let typed = station
            .and_then(|station| station.reflect(entity))
            .and_then(|value| reflected_string(value.as_partial_reflect(), "species"));
        let prop = typed.or_else(|| {
            let node = component?.reflect(entity)?;
            reflected_attr(node.as_partial_reflect(), "prop-flora")
        });
        if let Some(prop) = prop {
            *counts.entry(prop.clone()).or_default() += 1;
            stations.push((entity.id(), prop));
        }
    }
    drop(registry);
    let mut index = world.resource_mut::<StationIndex>();
    index.stations = stations;
    index.counts = counts;
}

/// Um campo `String` de um struct refletido.
fn reflected_string(value: &dyn PartialReflect, field: &str) -> Option<String> {
    let ReflectRef::Struct(value) = value.reflect_ref() else {
        return None;
    };
    value.field(field)?.try_downcast_ref::<String>().cloned()
}

/// `WorldNode.attrs[key == …].value` por reflexão.
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

/// Fila de variantes por espécie, em frente à câmara.
fn species_preview(
    mut commands: Commands,
    mut preview: ResMut<FloraPreview>,
    species: Query<(Entity, Ref<FloraSpecies>)>,
    trees: Query<Entity, With<SpeciesPreviewTree>>,
) {
    let changed = species.iter().any(|(_, s)| s.is_changed());
    if !preview.species {
        if !trees.is_empty() {
            for tree in &trees {
                commands.entity(tree).despawn();
            }
        }
        return;
    }
    if !preview.rebuild_species && !changed {
        return;
    }
    preview.rebuild_species = false;
    for tree in &trees {
        commands.entity(tree).despawn();
    }
    let anchor = preview.anchor.unwrap_or_default();
    let mut list: Vec<&FloraSpecies> = species.iter().map(|(_, s)| s.into_inner()).collect();
    list.sort_by(|a, b| a.key.cmp(&b.key));
    for (row, species) in list.into_iter().enumerate() {
        let spacing = (species.params.trunk.height * 0.9).max(6.0);
        for variant in 0..species.variants.max(1).min(8) {
            let local = Vec3::new(
                (variant as f32 - 0.5 * (species.variants.min(8) as f32 - 1.0)) * spacing,
                0.0,
                -(row as f32) * 14.0,
            );
            commands.spawn((
                Name::new(format!("flora preview {} #{variant}", species.key)),
                SpeciesPreviewTree,
                EditorHidden,
                ProceduralTree(species.variant_params(variant)),
                Transform::from_translation(anchor.transform_point(local))
                    .with_rotation(anchor.rotation),
                Visibility::default(),
            ));
        }
    }
}

/// Um pool por espécie (como o jogo), reconstruído quando a espécie muda.
fn station_pools(
    mut preview: ResMut<FloraPreview>,
    species: Query<Ref<FloraSpecies>>,
    settings: Query<&FloraSettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut pools: ResMut<Assets<TreePool>>,
) {
    if !preview.stations {
        return;
    }
    let changed = species.iter().any(|s| s.is_changed());
    if !changed && !preview.rebuild_pools && !preview.pools.is_empty() {
        return;
    }
    preview.rebuild_pools = false;
    let settings = settings.iter().next().cloned().unwrap_or_default();
    let lod = LodSettings {
        distances: LodDistances {
            lod1: settings.lod1_distance,
            lod2: settings.lod2_distance,
            // Sem impostores no editor (o bake usa câmaras offscreen).
            impostor: f32::INFINITY,
        },
        ..default()
    };
    for species in &species {
        if species.key.is_empty() {
            continue;
        }
        let variants = species.variants.max(1);
        let mut pool = build_pool(&species.params, variants, f32::INFINITY, &lod, &mut meshes);
        let (wood, leaves) = (species.params.trunk_color, species.params.foliage.color);
        pool.add_look(if species.cold_look {
            PoolColors {
                wood: cold_tint(wood),
                leaves: cold_tint(leaves),
                unlit: false,
            }
        } else {
            PoolColors {
                wood,
                leaves,
                unlit: false,
            }
        });
        match preview.pools.get(&species.key) {
            Some((handle, _)) => {
                let handle = handle.clone();
                let _ = pools.insert(handle.id(), pool);
                preview
                    .pools
                    .insert(species.key.clone(), (handle, variants));
            }
            None => {
                let handle = pools.add(pool);
                preview
                    .pools
                    .insert(species.key.clone(), (handle, variants));
            }
        }
    }
}

/// Mesmo desvio de tom do jogo (`Vael/src/flora.rs`).
fn cold_tint(color: Color) -> Color {
    let c = color.to_srgba();
    Color::srgb(
        (c.red * 0.86).min(1.0),
        (c.green * 0.93).min(1.0),
        (c.blue * 1.12).min(1.0),
    )
}

/// Planta as estações aos lotes, só perto da câmara; as que ficam para trás
/// são despovoadas. Desligar o toggle tira tudo.
fn plant_stations(
    mut commands: Commands,
    mut preview: ResMut<FloraPreview>,
    index: Res<StationIndex>,
    stations: Query<(&GlobalTransform, Option<&Name>)>,
    cameras: Query<&GlobalTransform, With<MainViewportCamera>>,
    trees: Query<Entity, With<StationTree>>,
    settings: Query<&FloraSettings>,
) {
    if !preview.stations {
        if !preview.planted.is_empty() || !trees.is_empty() {
            for tree in &trees {
                commands.entity(tree).despawn();
            }
            preview.planted.clear();
        }
        return;
    }
    if preview.pools.is_empty() {
        return;
    }
    let cold_y = settings
        .iter()
        .next()
        .map_or(FloraSettings::default().cold_look_y, |s| s.cold_look_y);
    let center = cameras.iter().next().map(|camera| camera.translation());

    // Despovoa primeiro o que ficou fora do raio (com histerese). O orçamento
    // é o mesmo lote da plantação, para o frame nunca dar um salto.
    if let Some(center) = center {
        let unplant_radius = STATION_RADIUS * STATION_HYSTERESIS;
        let mut removed = 0;
        let mut stale = Vec::new();
        for (station, tree) in &preview.planted {
            let far = stations
                .get(*station)
                .map(|(transform, _)| transform.translation().distance(center) > unplant_radius)
                .unwrap_or(true);
            if !far {
                continue;
            }
            if removed >= BATCH {
                break;
            }
            if commands.get_entity(*tree).is_ok() {
                commands.entity(*tree).despawn();
            }
            stale.push(*station);
            removed += 1;
        }
        for station in stale {
            preview.planted.remove(&station);
        }
    }

    let mut planted_now = 0;
    for (station, prop) in &index.stations {
        let station = *station;
        if planted_now >= BATCH {
            break;
        }
        if preview.planted.contains_key(&station) {
            continue;
        }
        let Ok((transform, name)) = stations.get(station) else {
            continue;
        };
        if let Some(center) = center
            && transform.translation().distance(center) > STATION_RADIUS
        {
            continue;
        }
        let Some((pool, variants)) = preview.pools.get(prop) else {
            continue;
        };
        let variant = variant_of(name, *variants);
        let mut tree = commands.spawn((
            Name::new("flora estação"),
            StationTree,
            EditorHidden,
            PooledTree {
                pool: pool.clone(),
                variant,
            },
            Transform::default(),
            Visibility::Inherited,
            ChildOf(station),
        ));
        if transform.translation().y > cold_y {
            tree.insert(PooledLook(1));
        }
        preview.planted.insert(station, tree.id());
        planted_now += 1;
    }
}

/// O mesmo hash do jogo (`variant_of` em `Vael/src/flora.rs`).
fn variant_of(name: Option<&Name>, variants: u32) -> u32 {
    let Some(name) = name else {
        return 0;
    };
    let mut hash: u32 = 0x811C_9DC5;
    for byte in name.as_str().bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash % variants.max(1)
}

#[operator(
    id = "vael.flora.toggle_species",
    label = "Flora: pré-visualizar espécies",
    allows_undo = false
)]
pub(crate) fn toggle_species(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    // A fila nasce 25 m à frente da câmara, no plano horizontal dela, 8 m abaixo.
    let anchor = world
        .query_filtered::<&GlobalTransform, With<MainViewportCamera>>()
        .iter(world)
        .next()
        .map(|camera| {
            let forward = camera.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
            let at = camera.translation() + forward * 25.0 - Vec3::Y * 8.0;
            Transform::from_translation(at).looking_to(-forward, Vec3::Y)
        });
    let mut preview = world.resource_mut::<FloraPreview>();
    preview.species = !preview.species;
    preview.anchor = anchor;
    preview.rebuild_species = true;
    OperatorResult::Finished
}

#[operator(
    id = "vael.flora.toggle_stations",
    label = "Flora: árvores nas estações",
    allows_undo = false
)]
pub(crate) fn toggle_stations(
    _params: In<OperatorParameters>,
    mut preview: ResMut<FloraPreview>,
) -> OperatorResult {
    preview.stations = !preview.stations;
    preview.rebuild_pools = true;
    OperatorResult::Finished
}

#[operator(
    id = "vael.flora.rebuild",
    label = "Flora: reconstruir preview",
    allows_undo = false
)]
pub(crate) fn rebuild(
    _params: In<OperatorParameters>,
    mut preview: ResMut<FloraPreview>,
) -> OperatorResult {
    preview.rebuild_species = true;
    preview.rebuild_pools = true;
    OperatorResult::Finished
}

/// Nova espécie (carvalho) como filha da entidade `FloraSettings`: um grupo
/// + `component.add`, com undo e gravada no `.bsn`.
#[operator(
    id = "vael.flora.add_species",
    label = "Flora: nova espécie",
    allows_undo = false
)]
pub(crate) fn add_species(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some(parent) = ui::settings_entity::<FloraSettings>(world) else {
        return ui::missing::<FloraSettings>(world);
    };
    let history = CallOperatorSettings {
        creates_history_entry: true,
        ..default()
    };
    let name = format!(
        "nova_especie_{}",
        world.query::<&FloraSpecies>().iter(world).count() + 1
    );
    if let Err(err) = world
        .operator("entity.add.group")
        .param("name", name.clone())
        .param("parent", parent)
        .settings(history)
        .call()
    {
        warn_caller(world, format!("vael.flora: {err}"));
        return OperatorResult::Cancelled;
    }
    let children: Vec<Entity> = world
        .get::<Children>(parent)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let Some(entity) = children.into_iter().rev().find(|child| {
        world
            .get::<Name>(*child)
            .is_some_and(|n| n.as_str() == name)
    }) else {
        warn_caller(world, "vael.flora: o grupo criado não apareceu");
        return OperatorResult::Cancelled;
    };
    if let Err(err) = world
        .operator("component.add")
        .param("entity", entity)
        .param("type_path", SPECIES_TYPE.to_string())
        .settings(history)
        .call()
    {
        warn_caller(world, format!("vael.flora: {err}"));
        return OperatorResult::Cancelled;
    }
    ui::set_field(world, entity, SPECIES_TYPE, "key", format!("\"{name}\""))
}

pub(crate) fn register(ctx: &mut ExtensionRegistrar<'_>) {
    ctx.register_operator::<ToggleSpeciesOp>()
        .register_operator::<ToggleStationsOp>()
        .register_operator::<RebuildOp>()
        .register_operator::<AddSpeciesOp>();
}
