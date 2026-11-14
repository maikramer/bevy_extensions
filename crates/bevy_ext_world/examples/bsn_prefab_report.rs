//! Relatório de candidatos a prefab: nós folha com os mesmos componentes
//! (tag, atributos sem nome/pose, specs) repetidos pelo mundo.
//! `cargo run -p bevy_ext_world --example bsn_prefab_report -- <ficheiros.bsn>`
use std::collections::BTreeMap;

use jackdaw_bsn::{BsnPatch, BsnValue, SceneBsnAst, patch_type_path};

const POSE: &[&str] = &["name", "translation", "euler", "rotation", "scale", "pos", "transform"];
const SKIP: &[&str] = &[
    "bevy_transform::components::transform::Transform",
    "jackdaw_scene_types::node_id::SceneNodeId",
    "jackdaw::prefab::components::PrefabEntityId",
];

fn signature(ast: &SceneBsnAst, node: bevy::ecs::entity::Entity) -> Option<(String, String)> {
    let patches = ast.get_patches(node)?;
    let mut tag = None;
    let mut parts = Vec::new();
    for &pe in &patches.0 {
        let patch = ast.get_patch(pe)?;
        match patch {
            BsnPatch::Children(kids) if !kids.is_empty() => return None, // só folhas
            BsnPatch::Name(_) | BsnPatch::Children(_) => continue,
            _ => {}
        }
        let path = patch_type_path(patch).unwrap_or("");
        if SKIP.contains(&path) {
            continue;
        }
        if path == "bevy_world_xml::bsn::WorldNode" {
            if let BsnPatch::Struct(data) = patch {
                for field in &data.fields.0 {
                    match (field.name.as_str(), &field.value) {
                        ("tag", BsnValue::String(t)) => tag = Some(t.clone()),
                        ("attrs", BsnValue::List(items)) => {
                            let mut attrs: Vec<String> = items
                                .iter()
                                .filter_map(|item| match item {
                                    BsnValue::Struct(prop) => {
                                        let get = |n: &str| prop.fields.0.iter().find(|f| f.name == n).map(|f| format!("{:?}", f.value));
                                        let key = get("key")?;
                                        if POSE.iter().any(|p| key == format!("String({p:?})")) {
                                            return None;
                                        }
                                        Some(format!("{key}={}", get("value").unwrap_or_default()))
                                    }
                                    _ => None,
                                })
                                .collect();
                            attrs.sort();
                            parts.push(attrs.join(";"));
                        }
                        ("text", v) => parts.push(format!("text={v:?}")),
                        _ => {}
                    }
                }
            }
            continue;
        }
        parts.push(format!("{patch:?}"));
    }
    Some((tag?, parts.join(" | ")))
}

fn main() -> anyhow::Result<()> {
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut total = 0usize;
    for file in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&file)?;
        let ast = jackdaw_bsn::parse_bsn_text(&text).map_err(|e| anyhow::anyhow!("{file}: {e}"))?;
        let mut stack: Vec<_> = ast.roots.clone();
        while let Some(node) = stack.pop() {
            if let Some(patches) = ast.get_patches(node) {
                for &pe in &patches.0 {
                    if let Some(BsnPatch::Children(kids)) = ast.get_patch(pe) {
                        stack.extend(kids.iter().copied());
                    }
                }
            }
            if let Some(sig) = signature(&ast, node) {
                total += 1;
                groups.entry(sig).or_default().push(file.clone());
            }
        }
    }
    let mut ranked: Vec<_> = groups.into_iter().filter(|(_, v)| v.len() >= 4).collect();
    ranked.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let covered: usize = ranked.iter().map(|(_, v)| v.len()).sum();
    println!("folhas com WorldNode: {total}; em grupos ≥4: {covered} ({} arquétipos)", ranked.len());
    for ((tag, sig), files) in ranked.iter().take(40) {
        let mut per: BTreeMap<&str, usize> = BTreeMap::new();
        for f in files {
            *per.entry(f.rsplit('/').next().unwrap_or(f)).or_default() += 1;
        }
        let short: String = sig.chars().take(150).collect();
        println!("{:>5}  <{tag}> {short}\n       {per:?}", files.len());
    }
    Ok(())
}
