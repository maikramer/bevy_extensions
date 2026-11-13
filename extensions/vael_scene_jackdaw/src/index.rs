//! Índice da cena: o que cada entidade autorada é, lido por reflexão.
//!
//! O `WorldNode` e os componentes do `bevy_ext_scene` chegam ao editor pelo
//! schema do jogo: o tipo registado não é a cópia deste dylib. Lemo-los pelo
//! *type path* (`ReflectComponent` + `FromReflect`/campos refletidos) e só
//! relemos as entidades cujos ticks mudaram desde a última passagem.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::*;
use bevy::reflect::{FromReflect, PartialReflect, ReflectRef};
use bevy_ext_scene::{
    ColliderShape, ColliderSpec, Destructible, FloraStation, Harvestable, PortalSpec, ScriptRef,
    SpawnExclusionSpec,
};
use jackdaw_extension::document::SceneFilePath;
use jackdaw_extension::prelude::Icon;

pub const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
pub const COLLIDER: &str = "bevy_ext_scene::ColliderSpec";
pub const BODY: &str = "bevy_ext_scene::BodySpec";
pub const DESTRUCTIBLE: &str = "bevy_ext_scene::Destructible";
pub const HARVEST: &str = "bevy_ext_scene::Harvestable";
pub const EXCLUSION: &str = "bevy_ext_scene::SpawnExclusionSpec";
pub const PORTAL: &str = "bevy_ext_scene::PortalSpec";
pub const FLORA: &str = "bevy_ext_scene::FloraStation";
pub const SCRIPT: &str = "bevy_ext_scene::ScriptRef";

/// Anel vertical (raio no plano XZ) desenhado pelos gizmos.
#[derive(Clone, Copy, Debug)]
pub struct Ring {
    /// Centro em XZ; `None` usa a posição da entidade.
    pub at: Option<Vec2>,
    pub radius: f32,
    pub color: Color,
}

/// O que o índice sabe de uma entidade.
#[derive(Clone, Debug, Default)]
pub struct NodeInfo {
    pub tag: String,
    pub name: Option<String>,
    pub icon: Option<Icon>,
    pub collider: Option<ColliderSpec>,
    pub body: bool,
    pub destructible: Option<Destructible>,
    pub harvest: Option<Harvestable>,
    pub ring: Option<Ring>,
    /// Região AABB (mundo) de um `DynamicSpawner`.
    pub region: Option<(Vec3, Vec3)>,
    /// Atributos do DSL que ainda podiam ser tipados.
    pub legacy_attrs: usize,
    pub portal_target: Option<String>,
    /// Modelo do nó (`url` do `GltfScene`).
    pub url: Option<String>,
    /// `Terrain`: heightmap, lado do mundo e altura máxima.
    pub terrain: Option<(String, f32, f32)>,
    pub script: Option<String>,
    pub problems: Vec<String>,
}

#[derive(Resource, Default)]
pub struct SceneIndex {
    pub nodes: HashMap<Entity, NodeInfo>,
    last_tick: Option<Tick>,
    timer: f32,
    files: HashMap<String, bool>,
    pub generation: u64,
}

/// Ícones por entidade para o predicado do Outliner (uma `fn` sem estado).
pub static ICONS: LazyLock<RwLock<HashMap<Entity, Icon>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn icon_predicate(entity: EntityRef) -> Option<Icon> {
    ICONS.read().ok()?.get(&entity.id()).copied()
}

struct Ids {
    world_node: Option<(ComponentId, ReflectComponent)>,
    collider: Option<(ComponentId, ReflectComponent)>,
    body: Option<ComponentId>,
    destructible: Option<(ComponentId, ReflectComponent)>,
    harvest: Option<(ComponentId, ReflectComponent)>,
    exclusion: Option<(ComponentId, ReflectComponent)>,
    portal: Option<(ComponentId, ReflectComponent)>,
    flora: Option<(ComponentId, ReflectComponent)>,
    script: Option<(ComponentId, ReflectComponent)>,
}

fn lookup(world: &World, registry: &bevy::reflect::TypeRegistry, path: &str) -> Option<(ComponentId, ReflectComponent)> {
    let registration = registry.get_with_type_path(path)?;
    let reflect = registration.data::<ReflectComponent>()?.clone();
    let id = world.components().get_id(registration.type_id())?;
    Some((id, reflect))
}

const SCAN_INTERVAL: f32 = 0.5;

pub fn scan(world: &mut World) {
    let delta = world.resource::<Time>().delta_secs();
    {
        let mut index = world.resource_mut::<SceneIndex>();
        index.timer -= delta;
        if index.timer > 0.0 {
            return;
        }
        index.timer = SCAN_INTERVAL;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let ids = Ids {
        world_node: lookup(world, &registry, WORLD_NODE),
        collider: lookup(world, &registry, COLLIDER),
        body: lookup(world, &registry, BODY).map(|(id, _)| id),
        destructible: lookup(world, &registry, DESTRUCTIBLE),
        harvest: lookup(world, &registry, HARVEST),
        exclusion: lookup(world, &registry, EXCLUSION),
        portal: lookup(world, &registry, PORTAL),
        flora: lookup(world, &registry, FLORA),
        script: lookup(world, &registry, SCRIPT),
    };
    let Some((world_node, _)) = &ids.world_node else {
        return;
    };
    let world_node = *world_node;
    let root = project_root(world.get_resource::<SceneFilePath>());
    let last = world.resource::<SceneIndex>().last_tick;
    let now = world.change_tick();
    let watched: Vec<ComponentId> = [
        Some(world_node),
        ids.collider.as_ref().map(|(id, _)| *id),
        ids.body,
        ids.destructible.as_ref().map(|(id, _)| *id),
        ids.harvest.as_ref().map(|(id, _)| *id),
        ids.exclusion.as_ref().map(|(id, _)| *id),
        ids.portal.as_ref().map(|(id, _)| *id),
        ids.flora.as_ref().map(|(id, _)| *id),
        ids.script.as_ref().map(|(id, _)| *id),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut alive = HashSet::new();
    let mut changed = Vec::new();
    let mut query = world.query::<EntityRef>();
    for entity in query.iter(world) {
        if !entity.contains_id(world_node) {
            continue;
        }
        alive.insert(entity.id());
        let dirty = match last {
            None => true,
            Some(last) => watched.iter().any(|id| {
                entity
                    .get_change_ticks_by_id(*id)
                    .is_some_and(|ticks| ticks.is_added(last, now) || ticks.is_changed(last, now))
            }),
        };
        if dirty {
            changed.push((entity.id(), read_node(entity, &ids)));
        }
    }
    drop(registry);

    let mut index = world.resource_mut::<SceneIndex>();
    let removed = index.nodes.keys().any(|entity| !alive.contains(entity));
    if changed.is_empty() && !removed && last.is_some() {
        index.last_tick = Some(now);
        return;
    }
    index.nodes.retain(|entity, _| alive.contains(entity));
    for (entity, mut info) in changed {
        check(&mut info, root.as_deref(), &mut index.files);
        index.nodes.insert(entity, info);
    }
    index.last_tick = Some(now);
    index.generation += 1;
    if index.generation == 1 || index.generation % 50 == 0 {
        info!(
            "vael scene: {} nós indexados ({} com ícone)",
            index.nodes.len(),
            index.nodes.values().filter(|info| info.icon.is_some()).count()
        );
    }
    if let Ok(mut icons) = ICONS.write() {
        icons.clear();
        icons.extend(
            index
                .nodes
                .iter()
                .filter_map(|(entity, info)| info.icon.map(|icon| (*entity, icon))),
        );
    }
}

fn read_node(entity: EntityRef, ids: &Ids) -> NodeInfo {
    let mut info = NodeInfo::default();
    if let Some((_, reflect)) = &ids.world_node
        && let Some(node) = reflect.reflect(entity)
    {
        let node = node.as_partial_reflect();
        info.tag = struct_string(node, "tag").unwrap_or_default();
        let pairs = attrs(node);
        let get = |key: &str| attr(&pairs, key);
        info.name = get("name").map(str::to_string);
        info.url = get("url").map(str::to_string);
        if info.tag == "Terrain"
            && let Some(heightmap) = get("heightmap")
        {
            let num = |key: &str, default: f32| {
                get(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
            };
            info.terrain = Some((
                heightmap.to_string(),
                num("world-size", 4000.0),
                num("max-height", 200.0),
            ));
        }
        info.legacy_attrs = [
            "collider",
            "rigidbody",
            "destructible",
            "resource-kind",
            "lod1-url",
            "prop-flora",
            "script",
            "particle-emitter",
        ]
            .iter()
            .filter(|key| get(key).is_some())
            .count();
        info.ring = ring_of(&info.tag, &pairs);
        info.region = region_of(&info.tag, &pairs);
        info.icon = icon_of(&info.tag, get("url"));
        if let Some(url) = get("url") {
            info.problems.push(format!("?url {url}"));
        }
    }
    info.collider = read_typed::<ColliderSpec>(entity, ids.collider.as_ref());
    info.body = ids.body.is_some_and(|id| entity.contains_id(id));
    info.destructible = read_typed::<Destructible>(entity, ids.destructible.as_ref());
    info.harvest = read_typed::<Harvestable>(entity, ids.harvest.as_ref());
    // Tipos do `bevy_ext_scene` ganham aos attrs por migrar.
    if let Some(exclusion) = read_typed::<SpawnExclusionSpec>(entity, ids.exclusion.as_ref()) {
        info.ring = Some(Ring {
            at: Some(exclusion.at),
            radius: exclusion.radius,
            color: Color::srgb(0.95, 0.35, 0.30),
        });
        info.icon = Some(Icon::Ban);
    }
    if let Some(portal) = read_typed::<PortalSpec>(entity, ids.portal.as_ref()) {
        info.ring = portal.auto_radius.or(portal.radius).map(|radius| Ring {
            at: None,
            radius,
            color: Color::srgb(0.55, 0.45, 1.0),
        });
        info.icon = Some(Icon::DoorOpen);
        info.portal_target = Some(portal.target);
    }
    if read_typed::<FloraStation>(entity, ids.flora.as_ref()).is_some() {
        info.icon = Some(Icon::TreePine);
    }
    if let Some(script) = read_typed::<ScriptRef>(entity, ids.script.as_ref()) {
        info.script = Some(script.path);
    }
    if let Some(harvest) = &info.harvest {
        info.icon = Some(match harvest.kind.as_str() {
            kind if kind.contains("wood") => Icon::TreePine,
            "stone" => Icon::Pickaxe,
            _ => Icon::Sparkles,
        });
    }
    info
}

fn read_typed<T: FromReflect>(entity: EntityRef, id: Option<&(ComponentId, ReflectComponent)>) -> Option<T> {
    let (_, reflect) = id?;
    let value = reflect.reflect(entity)?;
    T::from_reflect(value.as_partial_reflect())
}

/// Verificações baratas (ficheiros em falta, valores sem sentido). Os
/// `?url` pendentes de `read_node` são resolvidos aqui contra o disco.
fn check(info: &mut NodeInfo, root: Option<&Path>, files: &mut HashMap<String, bool>) {
    let mut exists = |url: &str| -> bool {
        let Some(root) = root else {
            return true;
        };
        *files
            .entry(url.to_string())
            .or_insert_with(|| resolve(root, url).is_some())
    };
    let pending: Vec<String> = info
        .problems
        .drain(..)
        .filter_map(|p| p.strip_prefix("?url ").map(str::to_string))
        .collect();
    for url in pending {
        if !exists(&url) {
            info.problems.push(format!("modelo em falta: {url}"));
        }
    }
    if let Some(collider) = &info.collider {
        match &collider.shape {
            ColliderShape::Mesh { url, .. } | ColliderShape::ConvexHull { url } => {
                if url.is_empty() {
                    info.problems.push("colisor de malha sem mesh-url".into());
                } else if !exists(url) {
                    info.problems.push(format!("malha de colisão em falta: {url}"));
                }
            }
            ColliderShape::Cylinder { radius, half_height }
            | ColliderShape::Capsule { radius, half_height } => {
                if *radius <= 0.0 || *half_height <= 0.0 {
                    info.problems.push("colisor com dimensão ≤ 0".into());
                }
            }
            ColliderShape::Box { size, .. } if size.min_element() <= 0.0 => {
                info.problems.push("caixa de colisão com lado ≤ 0".into());
            }
            _ => {}
        }
    }
    if let Some(script) = info.script.clone()
        && let Some(root) = root
        && !root.join("scripts").join(&script).is_file()
    {
        info.problems.push(format!("script em falta: scripts/{script}"));
    }
    if let Some(target) = info.portal_target.clone()
        && let Some(root) = root
        && target != "main"
        && !root.join("scenes").join(format!("{target}.bsn")).is_file()
    {
        info.problems.push(format!("portal para cena inexistente: {target}"));
    }
    if let Some(destructible) = &info.destructible
        && destructible.hits == 0
    {
        info.problems.push("destrutível com 0 golpes".into());
    }
    if let Some(harvest) = &info.harvest
        && harvest.amount == 0
    {
        info.problems.push("colheita de 0 unidades".into());
    }
    if info.destructible.is_some() && info.collider.is_none() && info.legacy_attrs == 0 {
        info.problems.push("destrutível sem colisor (não se acerta)".into());
    }
}

fn resolve(root: &Path, url: &str) -> Option<PathBuf> {
    let clean = url.split('#').next().unwrap_or(url).trim_start_matches('/');
    [root.join(clean), root.join("assets/public").join(clean)]
        .into_iter()
        .find(|path| path.is_file())
}

pub fn project_root(scene: Option<&SceneFilePath>) -> Option<PathBuf> {
    let scene = scene?.path.as_deref()?;
    let mut dir = Path::new(scene).parent();
    while let Some(candidate) = dir {
        if candidate.join("jackdaw.toml").is_file() {
            return Some(candidate.to_path_buf());
        }
        dir = candidate.parent();
    }
    None
}

fn struct_string(value: &dyn PartialReflect, field: &str) -> Option<String> {
    let ReflectRef::Struct(value) = value.reflect_ref() else {
        return None;
    };
    value
        .field(field)?
        .try_downcast_ref::<String>()
        .cloned()
}

fn attrs(node: &dyn PartialReflect) -> Vec<(String, String)> {
    let ReflectRef::Struct(node) = node.reflect_ref() else {
        return Vec::new();
    };
    let Some(field) = node.field("attrs") else {
        return Vec::new();
    };
    let ReflectRef::List(list) = field.reflect_ref() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|prop| Some((struct_string(prop, "key")?, struct_string(prop, "value")?)))
        .collect()
}

fn floats(text: &str) -> Vec<f32> {
    text.split([' ', ','])
        .filter_map(|part| part.trim().parse().ok())
        .collect()
}

fn attr<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn ring_of(tag: &str, pairs: &[(String, String)]) -> Option<Ring> {
    let get = |key: &str| attr(pairs, key);
    let radius = |key: &str| get(key).and_then(|v| v.trim().parse::<f32>().ok());
    match tag {
        "SpawnExclusion" => Some(Ring {
            at: get("at").map(floats).and_then(|v| (v.len() == 2).then(|| Vec2::new(v[0], v[1]))),
            radius: radius("radius")?,
            color: Color::srgb(0.95, 0.35, 0.30),
        }),
        "ScenePortal" => Some(Ring {
            at: None,
            radius: radius("auto-radius")?,
            color: Color::srgb(0.55, 0.45, 1.0),
        }),
        "PointLight" => Some(Ring {
            at: None,
            radius: radius("radius")?,
            color: Color::srgb(1.0, 0.85, 0.40),
        }),
        _ => None,
    }
}

fn region_of(tag: &str, pairs: &[(String, String)]) -> Option<(Vec3, Vec3)> {
    let get = |key: &str| attr(pairs, key);
    if tag != "DynamicSpawner" {
        return None;
    }
    let min = floats(get("region-min")?);
    let max = floats(get("region-max")?);
    (min.len() == 3 && max.len() == 3)
        .then(|| (Vec3::new(min[0], min[1], min[2]), Vec3::new(max[0], max[1], max[2])))
}

fn icon_of(tag: &str, url: Option<&str>) -> Option<Icon> {
    Some(match tag {
        "GltfScene" | "GLTFLoader" => {
            let url = url.unwrap_or_default().to_ascii_lowercase();
            if url.contains("/characters/") || url.contains("/creatures/") {
                Icon::User
            } else if ["/forest/", "/vegetation/", "tree", "pine", "bush", "cactus"]
                .iter()
                .any(|needle| url.contains(needle))
            {
                Icon::TreePine
            } else if ["/village/", "/infra/", "/interiors/", "/farm/", "house", "wall"]
                .iter()
                .any(|needle| url.contains(needle))
            {
                Icon::House
            } else {
                Icon::Package
            }
        }
        "PointLight" | "SpotLight" | "DirectionalLight" => Icon::Lightbulb,
        "ParticleSystem" => Icon::Flame,
        "ScenePortal" => Icon::DoorOpen,
        "SceneEntry" => Icon::MapPin,
        "DynamicSpawner" | "Creature" => Icon::PawPrint,
        "NpcBody" => Icon::User,
        "SpawnExclusion" => Icon::Ban,
        "Road" | "Way" | "Line" | "Segment" => Icon::Route,
        "MusicLayer" => Icon::Music,
        "UiText" => Icon::Type,
        "Include" => Icon::Package,
        "Box" | "Cylinder" | "Capsule" | "Composition" => Icon::Shapes,
        "Terrain" | "TerrainPad" | "Cliff" => Icon::Mountain,
        "Scatter" => Icon::Layers,
        _ => return None,
    })
}
