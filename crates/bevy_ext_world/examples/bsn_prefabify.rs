//! Extrai prefabs dos objectos repetidos de um mundo `.bsn`.
//!
//! Um nó folha com `WorldNode` cujos componentes (tag, atributos sem
//! nome/pose, specs) se repetem pelo menos `--min` vezes vira um prefab em
//! `<prefabs>/<categoria>/<modelo>.bsn`; cada cópia passa a ser uma instância
//! (`IsA`) que só guarda o que é seu: nome, `Transform`, `SceneNodeId` e a
//! identidade de origem (`WorldNode.origin_file/origin_path`, a chave estável
//! do nó — saves e colheita dependem dela).
//!
//! ```text
//! cargo run -p bevy_ext_world --example bsn_prefabify -- \
//!     --root <projecto> --prefabs world/prefabs --min 4 [--dry-run] <ficheiros.bsn>
//! ```
//! Não escreve nada que o loader leia diferente: depois de converter, cada
//! ficheiro é comparado com o original (`worlds_equivalent`) e a escrita é
//! abortada se algum diferir.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use bevy::ecs::entity::Entity;
use jackdaw_bsn::{
    BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue, SceneBsnAst,
    patch_type_path,
};

const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const NODE_ID: &str = "jackdaw_scene_types::node_id::SceneNodeId";
const PREFAB_ID: &str = "jackdaw::prefab::components::PrefabEntityId";
const ISA: &str = "jackdaw::prefab::components::IsA";
/// Atributos que são da instância (o loader recompõe-nos do `#Name`/`Transform`).
/// Tags que são peças de outra coisa (UI, primitivas de uma composição,
/// marcadores de cena) — não fazem sentido como objecto reutilizável.
const NOT_PREFABS: &[&str] = &[
    "Use", "Prototype", "NpcBody", "SceneEntry", "ScenePortal", "SpawnExclusion", "Composition",
    "Box", "Sphere", "Capsule", "Cylinder", "Cone", "Torus", "Lathe", "Sweep", "Extruded", "Plane",
];

const OWN_ATTRS: &[&str] = &["name", "translation", "euler", "rotation", "scale", "pos", "transform"];

struct Args {
    root: PathBuf,
    prefabs: PathBuf,
    min: usize,
    dry_run: bool,
    files: Vec<PathBuf>,
}

fn args() -> anyhow::Result<Args> {
    let mut root = None;
    let mut prefabs = PathBuf::from("world/prefabs");
    let mut min = 4;
    let mut dry_run = false;
    let mut files = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--root" => root = it.next().map(PathBuf::from),
            "--prefabs" => prefabs = PathBuf::from(it.next().unwrap_or_default()),
            "--min" => min = it.next().and_then(|v| v.parse().ok()).unwrap_or(4),
            "--dry-run" => dry_run = true,
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let root = root.ok_or_else(|| anyhow::anyhow!("--root <projecto> em falta"))?;
    Ok(Args {
        root,
        prefabs,
        min,
        dry_run,
        files,
    })
}

fn str_field(data: &BsnStructData, name: &str) -> Option<String> {
    data.fields.0.iter().find(|f| f.name == name).and_then(|f| match &f.value {
        BsnValue::String(s) => Some(s.clone()),
        _ => None,
    })
}

fn prop_key(item: &BsnValue) -> Option<String> {
    match item {
        BsnValue::Struct(prop) => str_field(prop, "key"),
        _ => None,
    }
}

/// O que o prefab guarda de um nó (tudo menos o que é da instância) e a
/// assinatura que agrupa as cópias.
struct Shape {
    signature: String,
    tag: String,
    url: Option<String>,
    /// `FloraStation.species` / `Harvestable.kind`, para nomear sem modelo.
    species: Option<String>,
    harvest: Option<String>,
    /// Marcas legíveis do que o arquétipo tem (para nomear variantes).
    tokens: std::collections::BTreeSet<String>,
    /// Parâmetros que dão nome a luzes/emissores (`warm-5200`, `r16`).
    detail: Option<String>,
    prefab_patches: Vec<BsnPatch>,
}

/// `0xffa54f` → quente / frio / branco.
fn hue_class(hex: &str) -> &'static str {
    let digits = hex.trim_start_matches("0x").trim_start_matches('#');
    let channel = |i: usize| u8::from_str_radix(digits.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255) as f32 / 255.0;
    let (r, g, b) = (channel(0), channel(2), channel(4));
    if r > b + 0.2 {
        "warm"
    } else if b > r + 0.2 {
        "cool"
    } else if g > r + 0.2 && g > b + 0.2 {
        "green"
    } else {
        "white"
    }
}

fn shape_of(ast: &SceneBsnAst, node: Entity) -> Option<Shape> {
    let patches = ast.get_patches(node)?;
    let mut tag = None;
    let mut url = None;
    let mut species = None;
    let mut harvest = None;
    let mut tokens = std::collections::BTreeSet::new();
    let mut detail = None;
    let mut light_color = None;
    let mut light_intensity = None;
    let mut prefab_patches = Vec::new();
    let mut parts = Vec::new();
    for &pe in &patches.0 {
        let patch = ast.get_patch(pe)?;
        match patch {
            BsnPatch::Children(kids) if !kids.is_empty() => return None,
            BsnPatch::Name(_) | BsnPatch::Children(_) => continue,
            BsnPatch::Base(_) | BsnPatch::Template(..) => return None,
            _ => {}
        }
        let path = patch_type_path(patch).unwrap_or("");
        if path == ISA {
            return None; // já é instância
        }
        if [TRANSFORM, NODE_ID, PREFAB_ID].contains(&path) {
            continue;
        }
        if path == WORLD_NODE {
            let BsnPatch::Struct(data) = patch else {
                return None;
            };
            let mut fields = Vec::new();
            for field in &data.fields.0 {
                match field.name.as_str() {
                    "origin_file" | "origin_path" => {}
                    "attrs" => {
                        let BsnValue::List(items) = &field.value else {
                            return None;
                        };
                        let kept: Vec<BsnValue> = items
                            .iter()
                            .filter(|item| {
                                prop_key(item).is_none_or(|key| !OWN_ATTRS.contains(&key.as_str()))
                            })
                            .cloned()
                            .collect();
                        for item in &kept {
                            if let BsnValue::Struct(prop) = item {
                                match str_field(prop, "key").as_deref() {
                                    Some("url") => url = str_field(prop, "value"),
                                    Some("color") => light_color = str_field(prop, "value"),
                                    Some("intensity") => light_intensity = str_field(prop, "value"),
                                    _ => {}
                                }
                            }
                        }
                        let mut sig: Vec<String> = kept.iter().map(|i| format!("{i:?}")).collect();
                        sig.sort();
                        parts.push(sig.join(";"));
                        fields.push(BsnField {
                            name: field.name.clone(),
                            value: BsnValue::List(kept),
                        });
                    }
                    other => {
                        if other == "tag" {
                            if let BsnValue::String(t) = &field.value {
                                tag = Some(t.clone());
                            }
                        }
                        parts.push(format!("{other}={:?}", field.value));
                        fields.push(field.clone());
                    }
                }
            }
            prefab_patches.push(BsnPatch::Struct(BsnStructData {
                type_path: data.type_path.clone(),
                fields: BsnStructFields(fields),
            }));
            continue;
        }
        if let BsnPatch::Struct(data) = patch {
            if path.ends_with("FloraStation") {
                species = str_field(data, "species");
            } else if path.ends_with("Harvestable") {
                harvest = str_field(data, "kind");
                if let Some(kind) = &harvest {
                    tokens.insert(slug(kind));
                }
            } else if path.ends_with("ColliderSpec") {
                tokens.insert("solid".into());
            } else if path.ends_with("Destructible") {
                tokens.insert("breakable".into());
            } else if path.ends_with("ScriptRef") {
                if let Some(script) = str_field(data, "path") {
                    let stem = Path::new(&script).file_stem().map(|s| s.to_string_lossy().to_string());
                    tokens.insert(slug(&stem.unwrap_or(script)));
                }
            } else if path.ends_with("ParticleSpec") {
                let preset = data.fields.0.iter().find(|f| f.name == "preset").and_then(|f| match &f.value {
                    BsnValue::String(s) => Some(s.clone()),
                    BsnValue::TupleStruct(t) => t.values.first().and_then(|v| match v {
                        BsnValue::String(s) => Some(s.clone()),
                        _ => None,
                    }),
                    _ => None,
                });
                if let Some(preset) = preset {
                    tokens.insert(format!("fx:{}", slug(&preset)));
                }
                let rate = data.fields.0.iter().find(|f| f.name == "emitter").and_then(|f| {
                    let text = format!("{:?}", f.value);
                    let at = text.find("emission_rate")?;
                    let rest = &text[at..];
                    let num: String = rest
                        .chars()
                        .skip_while(|c| !c.is_ascii_digit())
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    Some(num.trim_end_matches(".0").to_string())
                });
                if let Some(rate) = rate {
                    detail = Some(format!("r{rate}"));
                }
            } else if path.ends_with("AuthoredProps") || path.ends_with("CustomProperties") {
                tokens.insert("props".into());
            }
        }
        parts.push(format!("{patch:?}"));
        prefab_patches.push(patch.clone());
    }
    let tag = tag?;
    if (tag == "PointLight" || tag == "SpotLight") && light_color.is_some() {
        detail = Some(format!(
            "{}-{}",
            hue_class(light_color.as_deref().unwrap_or("")),
            slug(light_intensity.as_deref().unwrap_or("x"))
        ));
    }
    if tag.starts_with("Ui") || NOT_PREFABS.contains(&tag.as_str()) {
        return None;
    }
    parts.sort();
    Some(Shape {
        signature: format!("{tag}|{}", parts.join("|")),
        tag,
        url,
        species,
        harvest,
        tokens,
        detail,
        prefab_patches,
    })
}

fn children_of(ast: &SceneBsnAst, node: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    if let Some(patches) = ast.get_patches(node) {
        for &pe in &patches.0 {
            if let Some(BsnPatch::Children(kids)) = ast.get_patch(pe) {
                out.extend(kids.iter().copied());
            }
        }
    }
    out
}

fn all_nodes(ast: &SceneBsnAst) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack: Vec<Entity> = ast.roots.clone();
    while let Some(node) = stack.pop() {
        out.push(node);
        stack.extend(children_of(ast, node));
    }
    out
}

fn vec3(x: f64, y: f64, z: f64) -> BsnValue {
    BsnValue::Struct(BsnStructData {
        type_path: "glam::Vec3".into(),
        fields: BsnStructFields(
            [("x", x), ("y", y), ("z", z)]
                .into_iter()
                .map(|(n, v)| BsnField {
                    name: n.into(),
                    value: BsnValue::Float(v),
                })
                .collect(),
        ),
    })
}

fn identity_transform() -> BsnPatch {
    BsnPatch::Struct(BsnStructData {
        type_path: TRANSFORM.into(),
        fields: BsnStructFields(vec![
            BsnField {
                name: "translation".into(),
                value: vec3(0.0, 0.0, 0.0),
            },
            BsnField {
                name: "rotation".into(),
                value: BsnValue::Struct(BsnStructData {
                    type_path: "glam::Quat".into(),
                    fields: BsnStructFields(
                        [("x", 0.0), ("y", 0.0), ("z", 0.0), ("w", 1.0)]
                            .into_iter()
                            .map(|(n, v)| BsnField {
                                name: n.into(),
                                value: BsnValue::Float(v),
                            })
                            .collect(),
                    ),
                }),
            },
            BsnField {
                name: "scale".into(),
                value: vec3(1.0, 1.0, 1.0),
            },
        ]),
    })
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// `<categoria>/<modelo>` de um arquétipo: a pasta e o ficheiro do modelo;
/// sem modelo, o que ele é (efeito, luz, árvore procedural, recurso).
fn prefab_stem(shape: &Shape) -> String {
    if let Some(url) = &shape.url {
        let path = Path::new(url);
        let category = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| slug(&s.to_string_lossy()))
            .unwrap_or_else(|| "misc".into());
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().trim_end_matches("_lod0").to_string())
            .unwrap_or_default();
        return format!("{category}/{}", slug(&stem));
    }
    let fx = shape.tokens.iter().find_map(|t| t.strip_prefix("fx:"));
    let detail = shape.detail.as_ref().map(|d| format!("-{d}")).unwrap_or_default();
    match (shape.tag.as_str(), fx, &shape.species, &shape.harvest) {
        ("ParticleSystem", Some(preset), _, _) => format!("fx/{preset}{detail}"),
        ("ParticleSystem", None, _, _) => format!("fx/emitter{detail}"),
        ("PointLight", ..) => format!("lights/point{detail}"),
        ("SpotLight", ..) => format!("lights/spot{detail}"),
        (_, _, Some(species), _) => format!("flora/{}", slug(species)),
        (_, _, None, Some(kind)) => format!("harvest/{}", slug(kind)),
        (tag, ..) => format!("misc/{}", slug(tag)),
    }
}

/// Sufixo de uma variante: o que ela tem e a base (a mais comum) não tem.
fn variant_suffix(shape: &Shape, base: &Shape) -> String {
    let extra: Vec<String> = shape
        .tokens
        .difference(&base.tokens)
        .filter(|t| !t.starts_with("fx:"))
        .cloned()
        .collect();
    extra.join("-")
}

fn main() -> anyhow::Result<()> {
    let args = args()?;
    let mut docs: Vec<(PathBuf, String, SceneBsnAst)> = Vec::new();
    for file in &args.files {
        let text = std::fs::read_to_string(file)?;
        let ast = jackdaw_bsn::parse_bsn_text(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        docs.push((file.clone(), text, ast));
    }

    // 1. Arquétipos e as suas cópias.
    let mut groups: BTreeMap<String, Vec<(usize, Entity)>> = BTreeMap::new();
    let mut shapes: HashMap<String, Shape> = HashMap::new();
    for (index, (_, _, ast)) in docs.iter().enumerate() {
        for node in all_nodes(ast) {
            // A raiz do documento nunca vira instância.
            if ast.roots.contains(&node) {
                continue;
            }
            if let Some(shape) = shape_of(ast, node) {
                groups.entry(shape.signature.clone()).or_default().push((index, node));
                shapes.entry(shape.signature.clone()).or_insert(shape);
            }
        }
    }
    let mut chosen: Vec<(String, Vec<(usize, Entity)>)> =
        groups.into_iter().filter(|(_, v)| v.len() >= args.min).collect();
    chosen.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));

    // 2. Nomes de prefab: a variante mais comum de cada modelo fica com o
    //    nome simples; as outras levam o que as distingue (`-solid`,
    //    `-breakable`, `-<recurso>`…) e só em último caso um número.
    // Base de cada modelo: a variante mais simples (menos marcas), e entre
    // essas a mais comum — as outras nomeiam-se pelo que acrescentam.
    let mut base_of: HashMap<String, String> = HashMap::new();
    for (sig, _) in &chosen {
        let stem = prefab_stem(&shapes[sig]);
        let better = base_of
            .get(&stem)
            .is_none_or(|held| shapes[sig].tokens.len() < shapes[held].tokens.len());
        if better {
            base_of.insert(stem, sig.clone());
        }
    }
    let mut taken: HashMap<String, usize> = HashMap::new();
    let mut named: Vec<(String, String, Vec<(usize, Entity)>)> = Vec::new();
    for (sig, nodes) in chosen {
        let stem = prefab_stem(&shapes[&sig]);
        let base = &shapes[&base_of[&stem]];
        let suffix = if base.signature == sig {
            String::new()
        } else {
            variant_suffix(&shapes[&sig], base)
        };
        let wanted = if suffix.is_empty() { stem.clone() } else { format!("{stem}-{suffix}") };
        let n = taken.entry(wanted.clone()).or_default();
        *n += 1;
        let name = if *n == 1 { wanted } else { format!("{wanted}-v{n}") };
        named.push((sig, name, nodes));
    }

    // 3. Ficheiros de prefab e substituição das cópias.
    let mut prefab_files: Vec<(PathBuf, String)> = Vec::new();
    let mut converted = 0usize;
    for (sig, stem, nodes) in &named {
        let shape = &shapes[sig];
        let rel = args.prefabs.join(format!("{stem}.bsn"));
        let source = rel.to_string_lossy().replace('\\', "/");
        let mut prefab = SceneBsnAst::default();
        let mut patches = vec![BsnPatch::Name(stem.rsplit('/').next().unwrap_or(stem).to_string()), identity_transform()];
        patches.extend(shape.prefab_patches.iter().cloned());
        patches.push(BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: NODE_ID.into(),
            values: vec![BsnValue::Int(jackdaw_scene_types::SceneNodeId::next().0 as i128)],
        }));
        let root = prefab.create_entity_node(patches);
        prefab.add_to_roots(root);
        prefab_files.push((args.root.join(&rel), jackdaw_bsn::emit_scene(&prefab)));

        for &(doc, node) in nodes {
            let ast = &mut docs[doc].2;
            let old: Vec<BsnPatch> = ast
                .get_patches(node)
                .map(|p| p.0.iter().filter_map(|&pe| ast.get_patch(pe).cloned()).collect())
                .unwrap_or_default();
            let mut new = Vec::new();
            for patch in &old {
                match patch {
                    BsnPatch::Name(_) => new.push(patch.clone()),
                    _ => {}
                }
            }
            new.push(BsnPatch::Struct(BsnStructData {
                type_path: ISA.into(),
                fields: BsnStructFields(vec![
                    BsnField {
                        name: "source".into(),
                        value: BsnValue::String(source.clone()),
                    },
                    BsnField {
                        name: "deleted".into(),
                        value: BsnValue::List(Vec::new()),
                    },
                ]),
            }));
            for patch in &old {
                let path = patch_type_path(patch).unwrap_or("");
                if path == TRANSFORM || path == NODE_ID {
                    new.push(patch.clone());
                }
                // A identidade estável do nó fica na instância
                // (`WorldOrigin`: um `WorldNode` aqui substituía o do prefab).
                if path == WORLD_NODE
                    && let BsnPatch::Struct(data) = patch
                {
                    let file = str_field(data, "origin_file").unwrap_or_default();
                    let origin_path = data
                        .fields
                        .0
                        .iter()
                        .find(|f| f.name == "origin_path")
                        .map(|f| f.value.clone())
                        .unwrap_or(BsnValue::List(Vec::new()));
                    if !file.is_empty() {
                        new.push(BsnPatch::Struct(BsnStructData {
                            type_path: "bevy_world_xml::bsn::WorldOrigin".into(),
                            fields: BsnStructFields(vec![
                                BsnField {
                                    name: "file".into(),
                                    value: BsnValue::String(file),
                                },
                                BsnField {
                                    name: "path".into(),
                                    value: origin_path,
                                },
                            ]),
                        }));
                    }
                }
            }
            new.push(BsnPatch::TupleStruct(BsnTupleStructData {
                type_path: PREFAB_ID.into(),
                values: vec![BsnValue::Int(0)],
            }));
            let entities: Vec<Entity> = new.into_iter().map(|p| ast.world.spawn(p).id()).collect();
            if let Some(patches) = ast.get_patches_mut(node) {
                let old_entities = std::mem::replace(&mut patches.0, entities);
                for pe in old_entities {
                    let _ = ast.world.despawn(pe);
                }
            }
            converted += 1;
        }
    }
    println!(
        "{} prefabs, {converted} cópias → instâncias ({} ficheiros)",
        named.len(),
        docs.len()
    );
    for (sig, stem, nodes) in named.iter().take(15) {
        let _ = sig;
        println!("  {:>5}  {stem}", nodes.len());
    }
    if args.dry_run {
        return Ok(());
    }

    // 4. Escreve prefabs, depois os mundos — cada um só se o loader o ler igual.
    for (path, text) in &prefab_files {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, text)?;
    }
    let mut failed = Vec::new();
    for (file, original, ast) in &docs {
        let emitted = jackdaw_bsn::emit_scene(ast);
        if emitted == *original {
            continue;
        }
        let backup = file.with_extension("bsn.pre-prefab");
        std::fs::write(&backup, original)?;
        std::fs::write(file, &emitted)?;
        let before = bevy_ext_world::bsn::load::load_bsn_world_in(&backup, &args.root);
        let after = bevy_ext_world::bsn::load::load_bsn_world_in(file, &args.root);
        let same = match (&before, &after) {
            (Ok(a), Ok(b)) => {
                bevy_ext_world::bsn::worlds_equivalent(&a.root_attrs, &a.nodes, &b.root_attrs, &b.nodes)
            }
            (Err(e), _) | (_, Err(e)) => Err(format!("{e:#}")),
        };
        match same {
            Ok(()) => {
                std::fs::remove_file(&backup)?;
            }
            Err(err) => {
                if std::env::var_os("PREFABIFY_KEEP").is_none() {
                    std::fs::write(file, original)?;
                }
                std::fs::remove_file(&backup)?;
                failed.push(format!("{}: {err}", file.display()));
            }
        }
    }
    if !failed.is_empty() {
        for f in &failed {
            eprintln!("REPOSTO (não equivalente) {}", f.chars().take(400).collect::<String>());
        }
        anyhow::bail!("{} ficheiro(s) repostos", failed.len());
    }
    Ok(())
}
