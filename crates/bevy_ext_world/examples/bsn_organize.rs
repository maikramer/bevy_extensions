//! Arruma documentos de um mundo aberto para o Outliner.
//!
//! - `--drop-empty-overrides`: tira as árvores-espelho de overrides vazias
//!   que o editor deixa debaixo das instâncias (`PrefabEntityId(k)` sem mais
//!   nada) — ruído que não muda nada no mundo;
//! - `--cells <m>`: um grupo com mais de `--min-children` filhos ganha pastas
//!   por célula da grelha (`célula x2 z-1`, como o World Partition), pela
//!   posição local de cada filho. As pastas são transparentes para o loader;
//!   antes de mudar um nó de sítio, a sua identidade (ficheiro#caminho, a
//!   chave dos saves) fica fixa num `WorldOrigin`.
//!
//! ```text
//! cargo run -p bevy_ext_world --example bsn_organize -- --root <projecto> \
//!     [--drop-empty-overrides] [--cells 128 --min-children 60] [--dry-run] <ficheiros.bsn>
//! ```
//! Verificar depois com `bsn_equiv --unordered` e `bsn_origin_equiv`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::ecs::entity::Entity;
use jackdaw_bsn::{
    BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue, SceneBsnAst,
    patch_type_path,
};

const ISA: &str = "jackdaw::prefab::components::IsA";
const PREFAB_ID: &str = "jackdaw::prefab::components::PrefabEntityId";
const NODE_ID: &str = "jackdaw_scene_types::node_id::SceneNodeId";
const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
const WORLD_ORIGIN: &str = "bevy_world_xml::bsn::WorldOrigin";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

fn children(ast: &SceneBsnAst, node: Entity) -> Vec<Entity> {
    ast.get_children_ast(node)
}

/// Só estrutura (id de prefab, nome, id de nó) e filhos igualmente vazios.
fn is_empty_override(ast: &SceneBsnAst, node: Entity) -> bool {
    let Some(patches) = ast.get_patches(node) else {
        return false;
    };
    let mut has_peid = false;
    for &pe in &patches.0 {
        let Some(patch) = ast.get_patch(pe) else {
            return false;
        };
        match patch {
            BsnPatch::Name(_) | BsnPatch::Children(_) => continue,
            _ => {}
        }
        match patch_type_path(patch) {
            Some(PREFAB_ID) => has_peid = true,
            Some(NODE_ID) => {}
            _ => return false,
        }
    }
    has_peid && children(ast, node).into_iter().all(|kid| is_empty_override(ast, kid))
}

fn drop_empty_overrides(ast: &mut SceneBsnAst) -> usize {
    let mut dropped = 0;
    for instance in ast.entities_with_component(ISA) {
        for kid in children(ast, instance) {
            if is_empty_override(ast, kid) {
                dropped += 1 + count_subtree(ast, kid);
                ast.remove_child_from_ast(instance, kid);
            }
        }
    }
    ast.drop_unreachable();
    dropped
}

fn count_subtree(ast: &SceneBsnAst, node: Entity) -> usize {
    children(ast, node)
        .into_iter()
        .map(|kid| 1 + count_subtree(ast, kid))
        .sum()
}

fn translation_xz(ast: &SceneBsnAst, node: Entity) -> Option<(f64, f64)> {
    let BsnValue::Struct(data) = jackdaw_bsn::get_bsn_field(ast, node, TRANSFORM, "translation")? else {
        return None;
    };
    let get = |name: &str| {
        data.fields.0.iter().find(|f| f.name == name).and_then(|f| match f.value {
            BsnValue::Float(v) => Some(v),
            BsnValue::Int(v) => Some(v as f64),
            _ => None,
        })
    };
    Some((get("x")?, get("z")?))
}

fn has_own_origin(ast: &SceneBsnAst, node: Entity) -> bool {
    if ast.find_patch_by_type_path(node, WORLD_ORIGIN).is_some() {
        return true;
    }
    matches!(
        jackdaw_bsn::get_bsn_field(ast, node, WORLD_NODE, "origin_file"),
        Some(BsnValue::String(file)) if !file.is_empty()
    )
}

/// Identidade de cada nó no mundo carregado (`SceneNodeId` → ficheiro
/// relativo à raiz + caminho), a que o loader lhe dá hoje.
type Identities = std::collections::HashMap<u64, (String, Vec<u32>)>;

fn identities(world: &Path, root: &Path) -> anyhow::Result<Identities> {
    let loaded = bevy_ext_world::bsn::load::load_bsn_world(world)?;
    let root = root.canonicalize().unwrap_or(root.to_path_buf());
    let mut out = Identities::new();
    fn walk(n: &bevy_ext_world::xml::XmlNode, root: &Path, out: &mut Identities) {
        if let Some(origin) = &n.origin
            && let Some(id) = bevy_ext_world::bsn::specs::scene_node_id_for(origin)
        {
            let file = origin.file.strip_prefix(root).unwrap_or(&origin.file);
            out.insert(id, (file.to_string_lossy().replace('\\', "/"), origin.path.clone()));
        }
        for c in &n.children {
            walk(c, root, out);
        }
    }
    for n in &loaded.nodes {
        walk(n, &root, &mut out);
    }
    Ok(out)
}

fn node_id(ast: &SceneBsnAst, node: Entity) -> Option<u64> {
    ast.stable_id_of(node)
}

fn push_patch(ast: &mut SceneBsnAst, node: Entity, patch: BsnPatch) {
    let pe = ast.world.spawn(patch).id();
    if let Some(patches) = ast.get_patches_mut(node) {
        patches.0.push(pe);
    }
}

/// Fixa a identidade de `node` e da subárvore (a que o loader lhes dá hoje,
/// lida do mundo carregado) antes de mudarem de sítio. As instâncias de
/// prefab não se descem: os nós de dentro vêm do ficheiro do prefab. `Err`
/// quando um nó não tem identidade conhecida (não se mexe no grupo).
fn pin_identity(ast: &mut SceneBsnAst, node: Entity, ids: &Identities) -> Result<usize, String> {
    let mut pinned = 0;
    let is_node = ast.find_patch_by_type_path(node, WORLD_NODE).is_some()
        || ast.find_patch_by_type_path(node, ISA).is_some();
    if is_node && !has_own_origin(ast, node) {
        let id = node_id(ast, node).ok_or("nó sem SceneNodeId")?;
        let (file, path) = ids.get(&id).cloned().ok_or(format!("id {id} sem identidade no mundo"))?;
        push_patch(
            ast,
            node,
            BsnPatch::Struct(BsnStructData {
                type_path: WORLD_ORIGIN.into(),
                fields: BsnStructFields(vec![
                    BsnField {
                        name: "file".into(),
                        value: BsnValue::String(file),
                    },
                    BsnField {
                        name: "path".into(),
                        value: BsnValue::List(path.iter().map(|&i| BsnValue::Int(i as i128)).collect()),
                    },
                ]),
            }),
        );
        pinned += 1;
    }
    if ast.find_patch_by_type_path(node, ISA).is_some() {
        return Ok(pinned);
    }
    for kid in children(ast, node) {
        pinned += pin_identity(ast, kid, ids)?;
    }
    Ok(pinned)
}

fn cell_folders(ast: &mut SceneBsnAst, ids: &Identities, cell: f64, min_children: usize) -> (usize, usize, usize) {
    let (mut groups, mut folders, mut pinned) = (0, 0, 0);
    let mut stack = ast.roots.clone();
    let mut targets = Vec::new();
    while let Some(node) = stack.pop() {
        let kids = children(ast, node);
        let positioned = kids.iter().filter(|&&k| translation_xz(ast, k).is_some()).count();
        // Só grupos de nós (não instâncias) com filhos todos posicionados e
        // sem pastas lá dentro.
        let all_nodes = kids.iter().all(|&k| {
            ast.find_patch_by_type_path(k, WORLD_NODE).is_some() || ast.find_patch_by_type_path(k, ISA).is_some()
        });
        if ast.find_patch_by_type_path(node, ISA).is_none()
            && kids.len() >= min_children
            && positioned == kids.len()
            && all_nodes
        {
            targets.push(node);
        } else {
            stack.extend(kids);
        }
    }
    for group in targets {
        let kids = children(ast, group);
        let mut by_cell: BTreeMap<(i64, i64), Vec<Entity>> = BTreeMap::new();
        for &kid in &kids {
            let (x, z) = translation_xz(ast, kid).unwrap_or((0.0, 0.0));
            by_cell
                .entry(((x / cell).floor() as i64, (z / cell).floor() as i64))
                .or_default()
                .push(kid);
        }
        if by_cell.len() < 2 {
            continue;
        }
        // Identidade fixa antes de mexer; um nó sem identidade conhecida
        // deixa o grupo como está.
        let mut trial = 0;
        let mut ok = true;
        for &kid in &kids {
            match pin_identity(ast, kid, ids) {
                Ok(n) => trial += n,
                Err(why) => {
                    eprintln!("  grupo deixado como estava: {why}");
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        pinned += trial;
        for kid in &kids {
            ast.remove_child_from_ast(group, *kid);
        }
        for ((cx, cz), members) in by_cell {
            let folder = ast.create_entity_node(vec![
                BsnPatch::Name(format!("célula x{cx} z{cz}")),
                BsnPatch::Struct(BsnStructData {
                    type_path: TRANSFORM.into(),
                    fields: BsnStructFields(Vec::new()),
                }),
                BsnPatch::TupleStruct(BsnTupleStructData {
                    type_path: NODE_ID.into(),
                    values: vec![BsnValue::Int(jackdaw_scene_types::SceneNodeId::next().0 as i128)],
                }),
            ]);
            for member in members {
                ast.add_child_to_ast(folder, member);
            }
            ast.add_child_to_ast(group, folder);
            folders += 1;
        }
        groups += 1;
    }
    (groups, folders, pinned)
}

fn main() -> anyhow::Result<()> {
    let mut root = None;
    let mut world_file: Option<PathBuf> = None;
    let (mut drop, mut cells, mut min_children, mut dry_run) = (false, None, 60usize, false);
    let mut files = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--root" => root = it.next().map(PathBuf::from),
            "--world" => world_file = it.next().map(PathBuf::from),
            "--drop-empty-overrides" => drop = true,
            "--cells" => cells = it.next().and_then(|v| v.parse::<f64>().ok()),
            "--min-children" => min_children = it.next().and_then(|v| v.parse().ok()).unwrap_or(60),
            "--dry-run" => dry_run = true,
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let root = root.ok_or_else(|| anyhow::anyhow!("--root <projecto> em falta"))?;
    let ids = match (&cells, &world_file) {
        (Some(_), Some(world)) => identities(world, &root)?,
        (Some(_), None) => anyhow::bail!("--cells precisa de --world <world.bsn> (identidades)"),
        _ => Identities::new(),
    };
    for file in files {
        let text = std::fs::read_to_string(&file)?;
        let mut ast = jackdaw_bsn::parse_bsn_text(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(Path::new(&file))
            .to_string_lossy()
            .replace('\\', "/");
        let mut report = Vec::new();
        if drop {
            let n = drop_empty_overrides(&mut ast);
            if n > 0 {
                report.push(format!("{n} nós-espelho vazios fora"));
            }
        }
        if let Some(cell) = cells {
            let (g, f, p) = cell_folders(&mut ast, &ids, cell, min_children);
            if g > 0 {
                report.push(format!("{g} grupos em {f} células ({p} identidades fixadas)"));
            }
        }
        if report.is_empty() {
            continue;
        }
        println!("{rel}: {}", report.join("; "));
        if !dry_run {
            let out = jackdaw_bsn::emit_scene(&ast);
            jackdaw_bsn::parse_bsn_text(&out).map_err(|e| anyhow::anyhow!("{rel}: ficaria inválido: {e}"))?;
            std::fs::write(&file, out)?;
        }
    }
    Ok(())
}
