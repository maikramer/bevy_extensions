//! Ponte dos componentes `bevy_ext_terrain::voxel::*` para a vista DSL.
//!
//! O Jackdaw autoravac cavernas/arcos/pontes como componentes em entidades
//! sem `WorldNode` (filhas de um `VoxelTerrain`); o runtime do Viber continua
//! a parsear a vista DSL (`<Cave>`/`<Arch>`/`<Bridge>`). Esta ponte devolve os
//! componentes ao parser durante a leitura do `.bsn`, como o [`super::specs`]
//! faz com os componentes do `bevy_ext_scene`:
//!
//! * `VoxelTerrain` funde no `<Terrain>` as chaves com ida-e-volta exacta
//!   (`heightmap`, `world-size`, `max-height`); o componente ganha aos
//!   homónimos. Os parâmetros de runtime (LOD, splat, …) continuam no nó DSL.
//! * `VoxelCave`/`VoxelArch`/`VoxelBridge` numa entidade sem `WorldNode`
//!   tornam-se o nó DSL correspondente com o conjunto canónico completo de
//!   atributos; as coordenadas do componente são locais ao pai, como no DSL, e
//!   a pose da entidade é emitida como num nó DSL qualquer. O componente é
//!   consumido aqui — nunca chega ao caminho `native`.
//!
//! [`migrate_terrain_text`] é a direcção inversa (âncora + `<Cave>` →
//! componente) e só move o que volta exactamente igual, à imagem do
//! `specs::migrate_text`.

use std::any::TypeId;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use bevy::ecs::entity::Entity;
use bevy::reflect::{FromReflect, PartialReflect, Reflect, TypePath, TypeRegistry};
use bevy_ext_terrain::{
    VoxelArch, VoxelArchProfile, VoxelBridge, VoxelBridgeStyle, VoxelCave, VoxelChamber,
    VoxelCliff, VoxelShaft, VoxelTerrain,
};
use jackdaw_bsn::{BsnPatch, SceneBsnAst, emit_scene, parse_bsn_text};

use super::components::WorldNode;
use super::load::{set_attr, transform_patch, world_node};
use super::pose::{Pose, format_f32, write_pose_attrs};
use crate::xml::NodeOrigin;
use crate::xml::XmlNode;
use crate::xml::values::{parse_bool, parse_f32, parse_vec2_list};

const TYPE_TERRAIN: &str = "bevy_ext_terrain::voxel::VoxelTerrain";
const TYPE_CAVE: &str = "bevy_ext_terrain::voxel::VoxelCave";
const TYPE_ARCH: &str = "bevy_ext_terrain::voxel::VoxelArch";
const TYPE_BRIDGE: &str = "bevy_ext_terrain::voxel::VoxelBridge";
const TYPE_CLIFF: &str = "bevy_ext_terrain::voxel::VoxelCliff";

const CLIFF_PROFILES: [&str; 7] = [
    "vertical", "concave", "convex", "columnar", "terraced", "overhang", "arch",
];
const CLIFF_SIDES: [&str; 3] = ["auto", "left", "right"];

fn registry() -> &'static TypeRegistry {
    static REGISTRY: OnceLock<TypeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = TypeRegistry::default();
        bevy_ext_terrain::register_types(&mut registry);
        registry
    })
}

fn read<T>(ast: &SceneBsnAst, entity: Entity) -> Option<T>
where
    T: Reflect + FromReflect + Default + TypePath,
{
    let value = jackdaw_bsn::get_bsn_field(ast, entity, T::type_path(), "")?;
    let partial = jackdaw_bsn::bsn_value_to_reflect(&value, TypeId::of::<T>(), registry(), None)?;
    let mut out = T::default();
    out.try_apply(partial.as_ref()).ok()?;
    Some(out)
}

/// Funde o `VoxelTerrain` nas chaves com ida-e-volta exacta do `<Terrain>`.
pub(crate) fn apply_terrain(
    ast: &SceneBsnAst,
    entity: Entity,
    tag: &str,
    attrs: &mut Vec<(String, String)>,
) {
    if !tag.eq_ignore_ascii_case("terrain") {
        return;
    }
    let Some(terrain) = read::<VoxelTerrain>(ast, entity) else {
        return;
    };
    if !terrain.heightmap.is_empty() {
        set_attr(attrs, "heightmap", terrain.heightmap);
    }
    set_attr(attrs, "world-size", format_f32(terrain.world_size));
    set_attr(attrs, "max-height", format_f32(terrain.max_height));
}

fn apply_anchor_attrs(terrain: &VoxelTerrain, attrs: &mut Vec<(String, String)>) {
    if !terrain.heightmap.is_empty() {
        set_attr(attrs, "heightmap", terrain.heightmap.clone());
    }
    set_attr(attrs, "world-size", format_f32(terrain.world_size));
    set_attr(attrs, "max-height", format_f32(terrain.max_height));
}

/// A entidade traz uma feature de terreno como componente?
pub(crate) fn is_terrain_feature(ast: &SceneBsnAst, entity: Entity) -> bool {
    [TYPE_CAVE, TYPE_ARCH, TYPE_BRIDGE, TYPE_CLIFF]
        .iter()
        .any(|path| ast.find_patch_by_type_path(entity, path).is_some())
}

/// Constrói o nó DSL de uma feature de terreno (entidade sem `WorldNode`).
/// `children` são os filhos convertidos normalmente; as salas/chaminés da
/// cave do componente entram à frente delas.
pub(crate) fn terrain_feature_node(
    ast: &SceneBsnAst,
    entity: Entity,
    children: Vec<XmlNode>,
    current_path: &Path,
    prefix: &[u32],
) -> Option<XmlNode> {
    if let Some(cave) = read::<VoxelCave>(ast, entity) {
        let mut attrs = Vec::new();
        cave_attrs(&cave, &mut attrs);
        let mut all = Vec::with_capacity(cave.chambers.len() + cave.shafts.len() + children.len());
        all.extend(cave.chambers.iter().map(chamber_node));
        all.extend(cave.shafts.iter().map(shaft_node));
        all.extend(children);
        return Some(feature_node(
            "Cave",
            attrs,
            all,
            ast,
            entity,
            current_path,
            prefix,
        ));
    }
    if let Some(arch) = read::<VoxelArch>(ast, entity) {
        let mut attrs = Vec::new();
        arch_attrs(&arch, &mut attrs);
        return Some(feature_node(
            "Arch",
            attrs,
            children,
            ast,
            entity,
            current_path,
            prefix,
        ));
    }
    if let Some(bridge) = read::<VoxelBridge>(ast, entity) {
        let mut attrs = Vec::new();
        bridge_attrs(&bridge, &mut attrs);
        return Some(feature_node(
            "Bridge",
            attrs,
            children,
            ast,
            entity,
            current_path,
            prefix,
        ));
    }
    if let Some(cliff) = read::<VoxelCliff>(ast, entity) {
        let mut attrs = Vec::new();
        cliff_attrs(&cliff, &mut attrs);
        return Some(feature_node(
            "Cliff",
            attrs,
            children,
            ast,
            entity,
            current_path,
            prefix,
        ));
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn feature_node(
    tag: &str,
    mut attrs: Vec<(String, String)>,
    children: Vec<XmlNode>,
    ast: &SceneBsnAst,
    entity: Entity,
    current_path: &Path,
    prefix: &[u32],
) -> XmlNode {
    if let Some((translation, rotation, scale)) = transform_patch(ast, entity) {
        let pose = Pose {
            translation,
            rotation,
            scale,
        };
        if pose != Pose::default() {
            write_pose_attrs(&mut attrs, pose);
        }
    }
    XmlNode {
        tag: tag.to_string(),
        attrs,
        text: String::new(),
        children,
        origin: Some(Arc::new(NodeOrigin {
            file: Arc::from(current_path),
            path: prefix.to_vec(),
        })),
    }
}

fn vec2_text([x, z]: [f32; 2]) -> String {
    format!("{} {}", format_f32(x), format_f32(z))
}

fn vec2_list(points: &[[f32; 2]]) -> String {
    points
        .iter()
        .map(|point| vec2_text(*point))
        .collect::<Vec<_>>()
        .join(" ")
}

fn float_list(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| format_f32(*value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn cave_attrs(cave: &VoxelCave, attrs: &mut Vec<(String, String)>) {
    if !cave.name.is_empty() {
        set_attr(attrs, "name", cave.name.clone());
    }
    set_attr(attrs, "path", vec2_list(&cave.path));
    set_attr(attrs, "radius", float_list(&cave.radius));
    set_attr(attrs, "depth", format_f32(cave.depth));
    set_attr(attrs, "open-ends", cave.open_ends.to_string());
    set_attr(attrs, "mouth-flare", format_f32(cave.mouth_flare));
    set_attr(attrs, "mouth-fraction", format_f32(cave.mouth_fraction));
    set_attr(attrs, "mound-height", format_f32(cave.mound_height));
    set_attr(attrs, "mound-radius", format_f32(cave.mound_radius));
    set_attr(attrs, "mouth-lift", cave.mouth_lift.to_string());
}

fn chamber_node(chamber: &VoxelChamber) -> XmlNode {
    let mut attrs = Vec::new();
    if !chamber.name.is_empty() {
        set_attr(&mut attrs, "name", chamber.name.clone());
    }
    set_attr(&mut attrs, "at", vec2_text(chamber.at));
    set_attr(&mut attrs, "radius", format_f32(chamber.radius));
    set_attr(&mut attrs, "height", format_f32(chamber.height));
    if let Some(depth) = chamber.depth {
        set_attr(&mut attrs, "depth", format_f32(depth));
    }
    XmlNode {
        tag: "Chamber".to_string(),
        attrs,
        text: String::new(),
        children: Vec::new(),
        origin: None,
    }
}

fn shaft_node(shaft: &VoxelShaft) -> XmlNode {
    let mut attrs = Vec::new();
    if !shaft.name.is_empty() {
        set_attr(&mut attrs, "name", shaft.name.clone());
    }
    set_attr(&mut attrs, "at", vec2_text(shaft.at));
    set_attr(&mut attrs, "radius", format_f32(shaft.radius));
    if let Some(depth) = shaft.depth {
        set_attr(&mut attrs, "depth", format_f32(depth));
    }
    XmlNode {
        tag: "Shaft".to_string(),
        attrs,
        text: String::new(),
        children: Vec::new(),
        origin: None,
    }
}

fn arch_attrs(arch: &VoxelArch, attrs: &mut Vec<(String, String)>) {
    if !arch.name.is_empty() {
        set_attr(attrs, "name", arch.name.clone());
    }
    // `at` (um portal) e `path` (pés no próprio chão) são exclusivos no
    // parser — emitir sempre os dois faria o mundo inválido.
    if arch.path.is_empty() {
        set_attr(attrs, "at", vec2_text(arch.at));
    } else {
        set_attr(attrs, "path", vec2_list(&arch.path));
    }
    if let Some(span) = arch.span {
        set_attr(attrs, "width", format_f32(span));
    }
    set_attr(attrs, "height", format_f32(arch.height));
    set_attr(attrs, "thickness", format_f32(arch.thickness));
    set_attr(attrs, "depth", format_f32(arch.depth));
    set_attr(attrs, "yaw", format_f32(arch.yaw));
    set_attr(attrs, "spans", arch.spans.to_string());
    set_attr(
        attrs,
        "profile",
        match arch.profile {
            VoxelArchProfile::Portal => "portal".to_string(),
            VoxelArchProfile::Natural => "natural".to_string(),
        },
    );
}

fn cliff_attrs(cliff: &VoxelCliff, attrs: &mut Vec<(String, String)>) {
    if !cliff.name.is_empty() {
        set_attr(attrs, "name", cliff.name.clone());
    }
    set_attr(attrs, "path", vec2_list(&cliff.path));
    set_attr(attrs, "width", format_f32(cliff.width));
    if let Some(height) = cliff.height {
        set_attr(attrs, "height", format_f32(height));
    }
    if let Some(angle) = cliff.angle {
        set_attr(attrs, "angle", format_f32(angle));
    }
    set_attr(attrs, "profile", cliff.profile.clone());
    set_attr(attrs, "side", cliff.side.clone());
    set_attr(attrs, "noise", format_f32(cliff.noise));
    set_attr(attrs, "gullies", format_f32(cliff.gullies));
    set_attr(attrs, "notches", format_f32(cliff.notches));
    set_attr(attrs, "talus", cliff.talus.to_string());
    set_attr(attrs, "talus-angle", format_f32(cliff.talus_angle));
    set_attr(attrs, "seed", cliff.seed.to_string());
}

fn bridge_attrs(bridge: &VoxelBridge, attrs: &mut Vec<(String, String)>) {
    if !bridge.name.is_empty() {
        set_attr(attrs, "name", bridge.name.clone());
    }
    set_attr(attrs, "path", vec2_list(&bridge.path));
    set_attr(attrs, "width", format_f32(bridge.width));
    set_attr(attrs, "rise", format_f32(bridge.rise));
    set_attr(attrs, "thickness", format_f32(bridge.thickness));
    set_attr(
        attrs,
        "style",
        match bridge.style {
            VoxelBridgeStyle::Stone => "stone".to_string(),
            VoxelBridgeStyle::Natural => "natural".to_string(),
        },
    );
    if let Some(spans) = bridge.spans {
        set_attr(attrs, "spans", spans.to_string());
    }
    set_attr(attrs, "pier-width", format_f32(bridge.pier_width));
    set_attr(attrs, "parapet", format_f32(bridge.parapet));
    set_attr(attrs, "clearance", format_f32(bridge.clearance));
}

// ─── Migração (âncora + `<Cave>` → componentes) ─────────────────────────────

/// Relatório de [`migrate_terrain_text`].
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct TerrainMigrateReport {
    /// Nós `<Terrain>` que ganharam a âncora `VoxelTerrain`.
    pub anchors: usize,
    /// Nós `<Cave>` que passaram a `VoxelCave`.
    pub caves: usize,
    /// Nós `<Arch>` que passaram a `VoxelArch`.
    pub arches: usize,
    /// Nós `<Cliff>` que passaram a `VoxelCliff`.
    pub cliffs: usize,
    /// Nós que ficaram em DSL porque a ida-e-volta não é exacta.
    pub skipped: Vec<String>,
}

/// Posição de um nó na árvore do documento.
enum Slot {
    Root(usize),
    Child { parent: Entity, index: usize },
}

fn collect_nodes(ast: &SceneBsnAst) -> Vec<(Entity, Slot)> {
    fn walk(ast: &SceneBsnAst, parent: Entity, out: &mut Vec<(Entity, Slot)>) {
        for (index, child) in ast.get_children_ast(parent).into_iter().enumerate() {
            out.push((child, Slot::Child { parent, index }));
            walk(ast, child, out);
        }
    }
    let mut out = Vec::new();
    for (index, &root) in ast.roots.iter().enumerate() {
        out.push((root, Slot::Root(index)));
        walk(ast, root, &mut out);
    }
    out
}

fn sorted_pairs(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = pairs.to_vec();
    pairs.sort();
    pairs
}

/// Os attrs de um `WorldNode` como pares ordenados (o que o
/// `worlds_equivalent` compara).
fn node_pairs(node: &WorldNode) -> Vec<(String, String)> {
    sorted_pairs(
        &node
            .attrs
            .iter()
            .map(|prop| (prop.key.clone(), prop.value.clone()))
            .collect::<Vec<_>>(),
    )
}

/// Cada par emitido tem de existir igual no nó original (o componente ganha
/// aos homónimos; uma chave nova mudaria a vista).
fn merges_equal(emitted: &[(String, String)], original: &[(String, String)]) -> bool {
    emitted.iter().all(|(key, value)| {
        original
            .iter()
            .any(|(other_key, other_value)| other_key == key && other_value == value)
    })
}

fn anchor_from_node(node: &WorldNode) -> Option<VoxelTerrain> {
    let heightmap = node.attr("heightmap")?.trim().to_string();
    if heightmap.is_empty() {
        return None;
    }
    let world_size = parse_f32(node.attr("world-size")?, "world-size").ok()?;
    let max_height = parse_f32(node.attr("max-height")?, "max-height").ok()?;
    Some(VoxelTerrain {
        heightmap,
        world_size,
        max_height,
        ..Default::default()
    })
}

/// Um único par `x z` (o `at` de uma sala/chaminé).
fn parse_vec2(value: &str, ctx: &str) -> std::result::Result<[f32; 2], String> {
    let mut points = parse_vec2_list(value, ctx).map_err(|err| err.to_string())?;
    if points.len() != 1 {
        return Err(format!("{ctx}: esperava um par x z, veio {value}"));
    }
    Ok(points.pop().unwrap())
}

/// O `<Cave>` (com os filhos `<Chamber>`/`<Shaft>`) em componente. `Err`
/// traz o motivo pelo qual o nó não é migrável (attr desconhecido, pose,
/// texto, filho inesperado ou formatação que não é byte-exacta).
fn cave_from_node(
    ast: &SceneBsnAst,
    entity: Entity,
    node: &WorldNode,
) -> std::result::Result<VoxelCave, String> {
    let bail = |reason: String| Err(reason);
    if !node.text.is_empty() {
        return bail("texto".to_string());
    }
    if let Some((translation, rotation, scale)) = transform_patch(ast, entity)
        && (translation, rotation, scale)
            != (
                bevy::math::Vec3::ZERO,
                bevy::math::Quat::IDENTITY,
                bevy::math::Vec3::ONE,
            )
    {
        return bail("pose".to_string());
    }
    let mut cave = VoxelCave::default();
    for prop in &node.attrs {
        let (key, value) = (prop.key.as_str(), prop.value.as_str());
        let ctx = format!("<Cave> {key}");
        match key {
            "name" => cave.name = value.to_string(),
            "path" => cave.path = parse_vec2_list(value, &ctx).map_err(|err| err.to_string())?,
            "radius" => {
                let radius = value
                    .split_whitespace()
                    .map(|part| parse_f32(part, &ctx))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|err| err.to_string())?;
                if radius.is_empty() {
                    return bail("radius vazio".to_string());
                }
                cave.radius = radius;
            }
            "depth" => {
                cave.depth = parse_f32(value, &ctx).map_err(|err| err.to_string())?;
            }
            "open-ends" => {
                cave.open_ends = parse_bool(value, &ctx).map_err(|err| err.to_string())?;
            }
            "mouth-flare" => {
                cave.mouth_flare = parse_f32(value, &ctx).map_err(|err| err.to_string())?;
            }
            "mouth-fraction" => {
                cave.mouth_fraction = parse_f32(value, &ctx).map_err(|err| err.to_string())?;
            }
            "mound-height" => {
                cave.mound_height = parse_f32(value, &ctx).map_err(|err| err.to_string())?;
            }
            "mound-radius" => {
                cave.mound_radius = parse_f32(value, &ctx).map_err(|err| err.to_string())?;
            }
            "mouth-lift" => {
                cave.mouth_lift = parse_bool(value, &ctx).map_err(|err| err.to_string())?;
            }
            other => return bail(format!("attr desconhecido `{other}`")),
        }
    }
    if cave.path.len() < 2 {
        return bail("path com menos de 2 pontos".to_string());
    }
    for child in ast.get_children_ast(entity) {
        let child_node = world_node(ast, child).ok_or_else(|| "filho sem WorldNode".to_string())?;
        match child_node.tag.to_ascii_lowercase().as_str() {
            "chamber" => {
                let mut chamber = VoxelChamber::default();
                for prop in &child_node.attrs {
                    let (key, value) = (prop.key.as_str(), prop.value.as_str());
                    let ctx = format!("<Chamber> {key}");
                    match key {
                        "name" => chamber.name = value.to_string(),
                        "at" => chamber.at = parse_vec2(value, &ctx)?,
                        "radius" => {
                            chamber.radius =
                                parse_f32(value, &ctx).map_err(|err| err.to_string())?;
                        }
                        "height" => {
                            chamber.height =
                                parse_f32(value, &ctx).map_err(|err| err.to_string())?;
                        }
                        "depth" => {
                            chamber.depth =
                                Some(parse_f32(value, &ctx).map_err(|err| err.to_string())?);
                        }
                        other => return bail(format!("<Chamber> attr desconhecido `{other}`")),
                    }
                }
                let formatted = chamber_node(&chamber);
                if !attrs_match_by_value("chamber", &formatted.attrs, &child_node) {
                    return bail("<Chamber> não volta exacto".to_string());
                }
                cave.chambers.push(chamber);
            }
            "shaft" => {
                let mut shaft = VoxelShaft::default();
                for prop in &child_node.attrs {
                    let (key, value) = (prop.key.as_str(), prop.value.as_str());
                    let ctx = format!("<Shaft> {key}");
                    match key {
                        "name" => shaft.name = value.to_string(),
                        "at" => shaft.at = parse_vec2(value, &ctx)?,
                        "radius" => {
                            shaft.radius = parse_f32(value, &ctx).map_err(|err| err.to_string())?;
                        }
                        "depth" => {
                            shaft.depth =
                                Some(parse_f32(value, &ctx).map_err(|err| err.to_string())?);
                        }
                        other => return bail(format!("<Shaft> attr desconhecido `{other}`")),
                    }
                }
                let formatted = shaft_node(&shaft);
                if !attrs_match_by_value("shaft", &formatted.attrs, &child_node) {
                    return bail("<Shaft> não volta exacto".to_string());
                }
                cave.shafts.push(shaft);
            }
            other => return bail(format!("filho inesperado <{other}>")),
        }
        if !child_node.text.is_empty() {
            return bail("filho com texto".to_string());
        }
    }
    Ok(cave)
}

/// A cave formatada volta ao nó original por **valor**: um attr ausente vale
/// o default do spec e `"2.0"` é `"2"` (é o que o jogo carrega, não os bytes).
fn attrs_match_by_value(tag: &str, formatted: &[(String, String)], node: &WorldNode) -> bool {
    let mut left: std::collections::BTreeMap<String, String> = formatted.iter().cloned().collect();
    let mut right: std::collections::BTreeMap<String, String> = node
        .attrs
        .iter()
        .map(|prop| (prop.key.clone(), prop.value.clone()))
        .collect();
    super::compare::terrain_attrs_equivalent(tag, &mut left, &mut right, "").is_ok()
        && left == right
}

/// Um nó sem filhos, sem pose e sem texto — requisito comum das features
/// simples (o arco e o cliff não têm filhos no DSL; a cave trata os seus).
fn plain_node(
    ast: &SceneBsnAst,
    entity: Entity,
    node: &WorldNode,
) -> std::result::Result<(), String> {
    if !node.text.is_empty() {
        return Err("texto".to_string());
    }
    if let Some((translation, rotation, scale)) = transform_patch(ast, entity)
        && (translation, rotation, scale)
            != (
                bevy::math::Vec3::ZERO,
                bevy::math::Quat::IDENTITY,
                bevy::math::Vec3::ONE,
            )
    {
        return Err("pose".to_string());
    }
    if !ast.get_children_ast(entity).is_empty() {
        return Err("filhos".to_string());
    }
    Ok(())
}

/// O `<Arch>` em componente. `Err` traz o motivo de não ser migrável.
fn arch_from_node(
    ast: &SceneBsnAst,
    entity: Entity,
    node: &WorldNode,
) -> std::result::Result<VoxelArch, String> {
    plain_node(ast, entity, node)?;
    let mut arch = VoxelArch::default();
    let mut authored_at = false;
    for prop in &node.attrs {
        let (key, value) = (prop.key.as_str(), prop.value.as_str());
        let ctx = format!("<Arch> {key}");
        match key {
            "name" => arch.name = value.to_string(),
            "at" => {
                arch.at = parse_vec2(value, &ctx)?;
                authored_at = true;
            }
            "path" => arch.path = parse_vec2_list(value, &ctx).map_err(|err| err.to_string())?,
            "width" | "span" => {
                arch.span = Some(parse_f32(value, &ctx).map_err(|err| err.to_string())?)
            }
            "height" => arch.height = parse_f32(value, &ctx).map_err(|err| err.to_string())?,
            "thickness" => {
                arch.thickness = parse_f32(value, &ctx).map_err(|err| err.to_string())?
            }
            "depth" => arch.depth = parse_f32(value, &ctx).map_err(|err| err.to_string())?,
            "yaw" => arch.yaw = parse_f32(value, &ctx).map_err(|err| err.to_string())?,
            "spans" => {
                arch.spans = crate::xml::values::parse_u32(value, &ctx)
                    .map_err(|err| err.to_string())?
                    .max(1)
            }
            "profile" if value == "portal" => arch.profile = VoxelArchProfile::Portal,
            "profile" if value == "natural" => arch.profile = VoxelArchProfile::Natural,
            "profile" => return Err(format!("profile desconhecido `{value}`")),
            other => return Err(format!("attr desconhecido `{other}`")),
        }
    }
    let has_path = arch.path.len() >= 2;
    if authored_at && has_path {
        return Err("at e path juntos".to_string());
    }
    Ok(arch)
}

/// O `<Cliff>` em componente. `Err` traz o motivo de não ser migrável.
fn cliff_from_node(
    ast: &SceneBsnAst,
    entity: Entity,
    node: &WorldNode,
) -> std::result::Result<VoxelCliff, String> {
    plain_node(ast, entity, node)?;
    let mut cliff = VoxelCliff::default();
    for prop in &node.attrs {
        let (key, value) = (prop.key.as_str(), prop.value.as_str());
        let ctx = format!("<Cliff> {key}");
        match key {
            "name" => cliff.name = value.to_string(),
            "path" => cliff.path = parse_vec2_list(value, &ctx).map_err(|err| err.to_string())?,
            "width" => cliff.width = parse_f32(value, &ctx).map_err(|err| err.to_string())?,
            "height" => cliff.height = Some(parse_f32(value, &ctx).map_err(|err| err.to_string())?),
            "angle" => cliff.angle = Some(parse_f32(value, &ctx).map_err(|err| err.to_string())?),
            "profile" if CLIFF_PROFILES.contains(&value) => cliff.profile = value.to_string(),
            "profile" => return Err(format!("profile desconhecido `{value}`")),
            "side" if CLIFF_SIDES.contains(&value) => cliff.side = value.to_string(),
            "side" => return Err(format!("side desconhecido `{value}`")),
            "noise" => cliff.noise = parse_f32(value, &ctx).map_err(|err| err.to_string())?,
            "gullies" => {
                cliff.gullies = parse_f32(value, &ctx)
                    .map_err(|err| err.to_string())?
                    .clamp(0.0, 0.6)
            }
            "notches" => {
                cliff.notches = parse_f32(value, &ctx)
                    .map_err(|err| err.to_string())?
                    .clamp(0.0, 0.5)
            }
            "talus" => cliff.talus = parse_bool(value, &ctx).map_err(|err| err.to_string())?,
            "talus-angle" => {
                cliff.talus_angle = parse_f32(value, &ctx).map_err(|err| err.to_string())?
            }
            "seed" => {
                cliff.seed = parse_f32(value, &ctx)
                    .map_err(|err| err.to_string())?
                    .max(0.0) as u64
            }
            other => return Err(format!("attr desconhecido `{other}`")),
        }
    }
    if cliff.path.len() < 2 {
        return Err("path com menos de 2 pontos".to_string());
    }
    Ok(cliff)
}

/// Uma feature pronta a trocar pelo seu componente.
enum FeatureSwap {
    Cave(VoxelCave),
    Arch(VoxelArch),
    Cliff(VoxelCliff),
}

impl FeatureSwap {
    fn tag(&self) -> &'static str {
        match self {
            FeatureSwap::Cave(_) => "Cave",
            FeatureSwap::Arch(_) => "Arch",
            FeatureSwap::Cliff(_) => "Cliff",
        }
    }

    fn name(&self) -> &str {
        match self {
            FeatureSwap::Cave(cave) => &cave.name,
            FeatureSwap::Arch(arch) => &arch.name,
            FeatureSwap::Cliff(cliff) => &cliff.name,
        }
    }

    /// Os attrs canónicos voltam ao nó original por valor.
    fn attrs_match(&self, node: &WorldNode) -> bool {
        let mut formatted = Vec::new();
        match self {
            FeatureSwap::Cave(cave) => cave_attrs(cave, &mut formatted),
            FeatureSwap::Arch(arch) => arch_attrs(arch, &mut formatted),
            FeatureSwap::Cliff(cliff) => cliff_attrs(cliff, &mut formatted),
        }
        attrs_match_by_value(self.tag().to_ascii_lowercase().as_str(), &formatted, node)
    }

    fn into_patches(self) -> Vec<BsnPatch> {
        let name = self.name().to_string();
        let component = match self {
            FeatureSwap::Cave(cave) => Box::new(cave) as Box<dyn PartialReflect>,
            FeatureSwap::Arch(arch) => Box::new(arch) as Box<dyn PartialReflect>,
            FeatureSwap::Cliff(cliff) => Box::new(cliff) as Box<dyn PartialReflect>,
        };
        let mut patches = Vec::new();
        if !name.is_empty() {
            patches.push(BsnPatch::Name(name));
        }
        patches.push(jackdaw_bsn::component_to_bsn_patch(
            component.as_ref(),
            registry(),
        ));
        patches
    }
}

/// Move a âncora e as features (cave/arco/cliff) de um documento `.bsn` para
/// componentes. Só migra o que volta exactamente igual; idempotente.
pub fn migrate_terrain_ast(ast: &mut SceneBsnAst) -> TerrainMigrateReport {
    let nodes = collect_nodes(ast);
    let mut report = TerrainMigrateReport::default();
    struct Swap {
        old: Entity,
        slot: Slot,
        feature: FeatureSwap,
    }
    let mut swaps: Vec<Swap> = Vec::new();

    for (entity, slot) in nodes {
        let Some(node) = world_node(ast, entity) else {
            continue;
        };
        let tag = node.tag.to_ascii_lowercase();
        if tag == "terrain" {
            if ast.find_patch_by_type_path(entity, TYPE_TERRAIN).is_some() {
                continue;
            }
            let Some(anchor) = anchor_from_node(&node) else {
                continue;
            };
            let mut attrs = Vec::new();
            apply_anchor_attrs(&anchor, &mut attrs);
            if !merges_equal(&attrs, &node_pairs(&node)) {
                report
                    .skipped
                    .push("Terrain: a âncora não volta igual".to_string());
                continue;
            }
            let patch = jackdaw_bsn::component_to_bsn_patch(&anchor, registry());
            let patch_entity = ast.world.spawn(patch).id();
            if let Some(patches) = ast.get_patches_mut(entity) {
                patches.0.push(patch_entity);
            }
            report.anchors += 1;
            continue;
        }
        let feature = match tag.as_str() {
            "cave" if ast.find_patch_by_type_path(entity, TYPE_CAVE).is_none() => {
                cave_from_node(ast, entity, &node).map(FeatureSwap::Cave)
            }
            "arch" if ast.find_patch_by_type_path(entity, TYPE_ARCH).is_none() => {
                arch_from_node(ast, entity, &node).map(FeatureSwap::Arch)
            }
            "cliff" if ast.find_patch_by_type_path(entity, TYPE_CLIFF).is_none() => {
                cliff_from_node(ast, entity, &node).map(FeatureSwap::Cliff)
            }
            _ => continue,
        };
        let display_tag = match feature {
            Ok(ref feature) => feature.tag(),
            Err(_) => node.tag.as_str(),
        };
        let feature = match feature {
            Ok(feature) => feature,
            Err(reason) => {
                report
                    .skipped
                    .push(format!("{}: {reason}", display_name_of(&node, display_tag)));
                continue;
            }
        };
        if !feature.attrs_match(&node) {
            report.skipped.push(format!(
                "{}: attrs não voltam exactos",
                display_name_of(&node, feature.tag())
            ));
            continue;
        }
        swaps.push(Swap {
            old: entity,
            slot,
            feature,
        });
    }

    for swap in swaps {
        let tag = swap.feature.tag();
        let fresh = ast.create_entity_node(swap.feature.into_patches());
        match swap.slot {
            Slot::Root(index) => {
                ast.roots.insert(index, fresh);
                ast.remove_from_roots(swap.old);
            }
            Slot::Child { parent, index } => {
                ast.insert_child_in_ast(parent, fresh, index);
                ast.remove_child_from_ast(parent, swap.old);
            }
        }
        match tag {
            "Cave" => report.caves += 1,
            "Arch" => report.arches += 1,
            "Cliff" => report.cliffs += 1,
            _ => {}
        }
    }
    report
}

fn display_name_of(node: &WorldNode, tag: &str) -> String {
    let attrs: Vec<(String, String)> = node
        .attrs
        .iter()
        .map(|prop| (prop.key.clone(), prop.value.clone()))
        .collect();
    super::components::display_name(tag, &attrs)
}

/// [`migrate_terrain_ast`] sobre o texto de um `.bsn`; devolve o documento
/// re-emitido.
pub fn migrate_terrain_text(text: &str) -> Result<(String, TerrainMigrateReport)> {
    let mut ast = parse_bsn_text(text)?;
    let report = migrate_terrain_ast(&mut ast);
    Ok((emit_scene(&ast), report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsn::components::WorldProp;
    use crate::bsn::load::load_bsn_world_in;
    use tempfile::TempDir;

    fn registry_with_world() -> TypeRegistry {
        let mut registry = TypeRegistry::default();
        registry.register::<WorldNode>();
        registry.register::<WorldProp>();
        bevy_ext_terrain::register_types(&mut registry);
        registry
    }

    fn world_patch(tag: &str, attrs: &[(&str, &str)], registry: &TypeRegistry) -> BsnPatch {
        let node = WorldNode {
            tag: tag.to_string(),
            attrs: attrs
                .iter()
                .map(|(key, value)| WorldProp {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
            ..Default::default()
        };
        jackdaw_bsn::component_to_bsn_patch(&node, registry)
    }

    #[test]
    fn cave_component_becomes_the_dsl_node() {
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);
        let cave = VoxelCave {
            name: "gruta".into(),
            path: vec![[-20.0, 0.0], [20.0, 0.0]],
            radius: vec![2.5, 4.0],
            depth: 9.0,
            open_ends: true,
            mouth_flare: 1.0,
            mouth_fraction: 0.5,
            chambers: vec![VoxelChamber {
                name: String::new(),
                at: [0.0, 0.0],
                radius: 6.0,
                height: 5.0,
                depth: Some(12.0),
            }],
            shafts: vec![VoxelShaft {
                name: String::new(),
                at: [5.0, 0.0],
                radius: 1.5,
                depth: None,
            }],
            mound_height: 6.0,
            mound_radius: 18.0,
            mouth_lift: true,
        };
        let child =
            ast.create_entity_node(vec![jackdaw_bsn::component_to_bsn_patch(&cave, &registry)]);
        ast.add_child_to_ast(root, child);
        let text = emit_scene(&ast);

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("world.bsn");
        std::fs::write(&path, &text).unwrap();
        let loaded = load_bsn_world_in(&path, dir.path()).unwrap();

        assert!(
            loaded.native.is_empty(),
            "o componente é consumido pela ponte"
        );
        let [cave] = &loaded.nodes[..] else {
            panic!("esperava um único nó, veio {:?}", loaded.nodes);
        };
        assert_eq!(cave.tag, "Cave");
        let get = |key: &str| {
            cave.attrs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
                .unwrap_or_default()
        };
        assert_eq!(get("name"), "gruta");
        assert_eq!(get("path"), "-20 0 20 0");
        assert_eq!(get("radius"), "2.5 4");
        assert_eq!(get("depth"), "9");
        assert_eq!(get("open-ends"), "true");
        assert_eq!(get("mouth-flare"), "1");
        assert_eq!(get("mouth-fraction"), "0.5");
        assert_eq!(get("mound-height"), "6");
        assert_eq!(get("mound-radius"), "18");
        assert_eq!(get("mouth-lift"), "true");
        assert_eq!(cave.children.len(), 2, "chamber + shaft");
        let chamber = cave
            .children
            .iter()
            .find(|node| node.tag == "Chamber")
            .expect("chamber");
        assert_eq!(
            chamber
                .attrs
                .iter()
                .find(|(key, _)| key == "at")
                .map(|(_, value)| value.as_str()),
            Some("0 0")
        );
        let shaft = cave
            .children
            .iter()
            .find(|node| node.tag == "Shaft")
            .expect("shaft");
        assert!(shaft.attrs.iter().all(|(key, _)| key != "depth"));
    }

    #[test]
    fn anchor_merges_into_the_terrain_node() {
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);
        let terrain = VoxelTerrain {
            heightmap: "world.data/terrain.ahgt".into(),
            world_size: 4000.0,
            max_height: 200.0,
            ..Default::default()
        };
        let anchor = jackdaw_bsn::component_to_bsn_patch(&terrain, &registry);
        let child = ast.create_entity_node(vec![
            world_patch(
                "Terrain",
                &[
                    ("render-distance", "950"),
                    ("world-size", "4000"),
                    ("max-height", "200"),
                ],
                &registry,
            ),
            anchor,
        ]);
        ast.add_child_to_ast(root, child);
        let text = emit_scene(&ast);

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("world.bsn");
        std::fs::write(&path, &text).unwrap();
        let loaded = load_bsn_world_in(&path, dir.path()).unwrap();

        let [terrain] = &loaded.nodes[..] else {
            panic!("esperava um único nó, veio {:?}", loaded.nodes);
        };
        assert_eq!(terrain.tag, "Terrain");
        let get = |key: &str| {
            terrain
                .attrs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
                .unwrap_or_default()
        };
        assert_eq!(get("heightmap"), "world.data/terrain.ahgt");
        assert_eq!(get("world-size"), "4000");
        assert_eq!(get("max-height"), "200");
        // Os parâmetros de runtime continuam no nó DSL.
        assert_eq!(get("render-distance"), "950");
    }

    #[test]
    fn migrate_moves_a_dsl_cave_and_keeps_the_view() {
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);

        let cave_attrs: &[(&str, &str)] = &[
            ("name", "gruta"),
            ("path", "-20 0 20 0"),
            ("radius", "2.5 4"),
            ("depth", "9"),
            ("open-ends", "true"),
            ("mouth-flare", "1"),
            ("mouth-fraction", "0.5"),
            ("mound-height", "6"),
            ("mound-radius", "18"),
            ("mouth-lift", "true"),
        ];
        let cave = ast.create_entity_node(vec![world_patch("Cave", cave_attrs, &registry)]);
        ast.add_child_to_ast(root, cave);
        let chamber = ast.create_entity_node(vec![world_patch(
            "Chamber",
            &[
                ("at", "0 0"),
                ("radius", "6"),
                ("height", "5"),
                ("depth", "12"),
            ],
            &registry,
        )]);
        ast.add_child_to_ast(cave, chamber);
        let before = emit_scene(&ast);

        let (after, report) = migrate_terrain_text(&before).unwrap();
        assert_eq!(report.caves, 1);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);

        let dir = TempDir::new().unwrap();
        let load = |text: &str| {
            let path = dir.path().join("world.bsn");
            std::fs::write(&path, text).unwrap();
            load_bsn_world_in(&path, dir.path()).unwrap()
        };
        let left = load(&before);
        let right = load(&after);
        let count =
            |nodes: &[XmlNode]| -> usize { nodes.iter().map(|node| 1 + node.children.len()).sum() };
        assert_eq!(count(&left.nodes), count(&right.nodes));
        assert_eq!(left.nodes.len(), right.nodes.len());
        assert_eq!(left.nodes[0].tag, right.nodes[0].tag);
        assert_eq!(
            sorted_pairs(&left.nodes[0].attrs),
            sorted_pairs(&right.nodes[0].attrs)
        );

        // Idempotente: a segunda passagem não encontra caves em DSL.
        let (_, again) = migrate_terrain_text(&after).unwrap();
        assert_eq!(again.caves, 0);
    }

    #[test]
    fn migrate_accepts_authored_defaults_and_reformatted_numbers() {
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);
        // Estilo das grutas: attrs default omitidos (mound-*/mouth-lift) e
        // `radius` de shaft com ".0" — a vista canónica difere nos bytes, mas
        // não no que o jogo carrega.
        let cave = ast.create_entity_node(vec![world_patch(
            "Cave",
            &[
                ("name", "mina"),
                ("path", "-60 -216 -74 -236"),
                ("radius", "2.4 3.6"),
                ("depth", "10"),
                ("open-ends", "true"),
                ("mouth-flare", "1"),
                ("mouth-fraction", "0.12"),
            ],
            &registry,
        )]);
        ast.add_child_to_ast(root, cave);
        let shaft = ast.create_entity_node(vec![world_patch(
            "Shaft",
            &[("at", "-74 -236"), ("radius", "2.0"), ("depth", "12")],
            &registry,
        )]);
        ast.add_child_to_ast(cave, shaft);
        let before = emit_scene(&ast);

        let (after, report) = migrate_terrain_text(&before).unwrap();
        assert_eq!(report.caves, 1, "{:?}", report.skipped);
        assert_eq!(report.anchors, 0);

        let dir = TempDir::new().unwrap();
        let load = |text: &str| {
            let path = dir.path().join("world.bsn");
            std::fs::write(&path, text).unwrap();
            load_bsn_world_in(&path, dir.path()).unwrap()
        };
        let left = load(&before);
        let right = load(&after);
        let [cave_before] = &left.nodes[..] else {
            panic!("esperava uma cave, veio {:?}", left.nodes);
        };
        let [cave_after] = &right.nodes[..] else {
            panic!("esperava uma cave, veio {:?}", right.nodes);
        };
        assert_eq!(cave_before.tag, "Cave");
        assert_eq!(cave_after.tag, "Cave");
        // O `worlds_equivalent` trata ausente ⇔ default e compara por valor.
        super::super::compare::worlds_equivalent(
            &[],
            std::slice::from_ref(cave_before),
            &[],
            std::slice::from_ref(cave_after),
        )
        .unwrap_or_else(|err| panic!("vistas diferentes: {err}"));
    }

    #[test]
    fn migrate_skips_caves_that_do_not_roundtrip() {
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);
        // Attr fora do contrato: não há como o representar no componente.
        let cave = ast.create_entity_node(vec![world_patch(
            "Cave",
            &[
                ("path", "-20 0 20 0"),
                ("radius", "2.5"),
                ("depth", "9"),
                ("open-ends", "true"),
                ("mouth-flare", "1"),
                ("mouth-fraction", "0.5"),
                ("mound-height", "6"),
                ("mound-radius", "18"),
                ("foo", "1"),
            ],
            &registry,
        )]);
        ast.add_child_to_ast(root, cave);
        let before = emit_scene(&ast);

        let (after, report) = migrate_terrain_text(&before).unwrap();
        assert_eq!(report.caves, 0);
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].contains("attr desconhecido `foo`"));
        // O documento fica intacto.
        assert_eq!(after, before);
    }

    #[test]
    fn migrate_moves_arches_and_cliffs() {
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);
        // Arco na forma `at` com o spelling `width` e defaults omitidos.
        let arch = ast.create_entity_node(vec![world_patch(
            "Arch",
            &[
                ("name", "arco"),
                ("at", "352 96"),
                ("profile", "natural"),
                ("width", "20"),
                ("height", "12"),
                ("thickness", "6"),
                ("depth", "7"),
                ("yaw", "40"),
            ],
            &registry,
        )]);
        ast.add_child_to_ast(root, arch);
        // Cliff com `talus="1"` (bool por inteiro) e talus-angle omitido.
        let cliff = ast.create_entity_node(vec![world_patch(
            "Cliff",
            &[
                ("name", "escarpa"),
                ("path", "92 -156 126 -150"),
                ("width", "12"),
                ("height", "22"),
                ("profile", "columnar"),
                ("side", "auto"),
                ("noise", "0.22"),
                ("seed", "31"),
                ("gullies", "0.25"),
                ("notches", "0.15"),
                ("talus", "1"),
            ],
            &registry,
        )]);
        ast.add_child_to_ast(root, cliff);
        let before = emit_scene(&ast);

        let (after, report) = migrate_terrain_text(&before).unwrap();
        assert_eq!(report.arches, 1, "{:?}", report.skipped);
        assert_eq!(report.cliffs, 1, "{:?}", report.skipped);

        let dir = TempDir::new().unwrap();
        let load = |text: &str| {
            let path = dir.path().join("world.bsn");
            std::fs::write(&path, text).unwrap();
            load_bsn_world_in(&path, dir.path()).unwrap()
        };
        let left = load(&before);
        let right = load(&after);
        assert_eq!(left.nodes.len(), 2);
        assert_eq!(right.nodes.len(), 2);
        super::super::compare::worlds_equivalent(&[], &left.nodes, &[], &right.nodes)
            .unwrap_or_else(|err| panic!("vistas diferentes: {err}"));

        // O arco migrated não pode emitir `at` e `path` ao mesmo tempo.
        let arch_after = right
            .nodes
            .iter()
            .find(|node| node.tag == "Arch")
            .expect("arco");
        let has = |key: &str| arch_after.attrs.iter().any(|(k, _)| k == key);
        assert!(has("width"));
        assert!(!has("span"));
        assert!(has("at"));
        assert!(!has("path"));
    }

    #[test]
    fn anchor_children_are_hoisted_to_siblings() {
        // O editor anexa as features à âncora; o parser ignora filhos de
        // `<Terrain>` — a vista tem de as subir a irmãs.
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);
        let terrain_patch = world_patch(
            "Terrain",
            &[("heightmap", "world.data/terrain.ahgt")],
            &registry,
        );
        let anchor = jackdaw_bsn::component_to_bsn_patch(&VoxelTerrain::default(), &registry);
        let terrain = ast.create_entity_node(vec![terrain_patch, anchor]);
        ast.add_child_to_ast(root, terrain);
        let cave = VoxelCave {
            path: vec![[-20.0, 0.0], [20.0, 0.0]],
            depth: 9.0,
            ..VoxelCave::default()
        };
        let child =
            ast.create_entity_node(vec![jackdaw_bsn::component_to_bsn_patch(&cave, &registry)]);
        ast.add_child_to_ast(terrain, child);
        let text = emit_scene(&ast);

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("world.bsn");
        std::fs::write(&path, &text).unwrap();
        let loaded = load_bsn_world_in(&path, dir.path()).unwrap();

        let [terrain, cave] = &loaded.nodes[..] else {
            panic!("esperava Terrain + Cave irmãos, veio {:?}", loaded.nodes);
        };
        assert_eq!(terrain.tag, "Terrain");
        assert!(
            terrain.children.is_empty(),
            "a cave não pode ficar filha do <Terrain>"
        );
        assert_eq!(cave.tag, "Cave");
    }

    #[test]
    fn migrate_skips_arches_with_at_and_path() {
        let registry = registry_with_world();
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![world_patch("world", &[], &registry)]);
        ast.add_to_roots(root);
        let arch = ast.create_entity_node(vec![world_patch(
            "Arch",
            &[("at", "10 10"), ("path", "0 0 20 0"), ("height", "12")],
            &registry,
        )]);
        ast.add_child_to_ast(root, arch);
        let before = emit_scene(&ast);

        let (_, report) = migrate_terrain_text(&before).unwrap();
        assert_eq!(report.arches, 0);
        assert!(report.skipped[0].contains("at e path juntos"));
    }
}
