//! Componentes tipados do `bevy_ext_scene` ↔ atributos do DSL.
//!
//! O documento pode trazer `ColliderSpec`/`BodySpec`/`Destructible`/
//! `Harvestable` em vez das strings `collider=`/`rigidbody=`/… no
//! `WorldNode`. O loader devolve-os ao parser do Viber como atributos
//! ([`apply_specs`]); a migração ([`lift_specs`]) move os atributos para os
//! componentes só quando a ida e volta é exata.

use std::any::TypeId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};

use bevy::reflect::{FromReflect, PartialReflect, Reflect, TypePath, TypeRegistry};
use bevy_ext_scene::{
    AttrSpec, AuthoredProps, BodySpec, ColliderSpec, Destructible, FloraStation, Harvestable,
    ModelLod, NodeSpecs, ParticleSpec, PortalSpec, ScriptRef, SpawnExclusionSpec,
};

use crate::xml::NodeOrigin;
use jackdaw_bsn::SceneBsnAst;

fn registry() -> &'static TypeRegistry {
    static REGISTRY: OnceLock<TypeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = TypeRegistry::default();
        bevy_ext_scene::register_types(&mut registry);
        registry
    })
}

fn read<T>(ast: &SceneBsnAst, entity: bevy::ecs::entity::Entity) -> Option<T>
where
    T: Reflect + FromReflect + Default + TypePath,
{
    let value = jackdaw_bsn::get_bsn_field(ast, entity, T::type_path(), "")?;
    let partial = jackdaw_bsn::bsn_value_to_reflect(&value, TypeId::of::<T>(), registry(), None)?;
    let mut out = T::default();
    out.try_apply(partial.as_ref()).ok()?;
    Some(out)
}

fn write_attrs<T: AttrSpec>(spec: &T, attrs: &mut Vec<(String, String)>) {
    for (key, value) in spec.to_attrs() {
        match attrs.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => attrs.push((key.to_string(), value)),
        }
    }
}

/// Specs lidos por nó (ficheiro + caminho do `NodeOrigin`), para o jogo os
/// anexar às entidades que o spawner do Viber cria (`SceneSource`).
static INDEX: LazyLock<RwLock<HashMap<(PathBuf, Vec<u32>), Arc<NodeSpecs>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Os componentes tipados do nó de onde uma entidade veio, se o documento os
/// tinha (o último load de cada ficheiro ganha).
pub fn specs_for(origin: &NodeOrigin) -> Option<Arc<NodeSpecs>> {
    INDEX
        .read()
        .ok()?
        .get(&(origin.file.to_path_buf(), origin.path.clone()))
        .cloned()
}

fn record(origin: &NodeOrigin, specs: NodeSpecs) {
    if let Ok(mut index) = INDEX.write() {
        index.insert(
            (Path::to_path_buf(&origin.file), origin.path.clone()),
            Arc::new(specs),
        );
    }
}

/// Lê os componentes tipados da entidade, regista-os pela origem do nó e
/// escreve nos `attrs` os atributos equivalentes (o parser do Viber ainda os
/// usa como transporte). O componente ganha a um atributo homónimo.
pub(crate) fn apply_specs(
    ast: &SceneBsnAst,
    entity: bevy::ecs::entity::Entity,
    origin: &NodeOrigin,
    attrs: &mut Vec<(String, String)>,
) {
    let specs = NodeSpecs {
        collider: read::<ColliderSpec>(ast, entity),
        body: read::<BodySpec>(ast, entity),
        destructible: read::<Destructible>(ast, entity),
        harvest: read::<Harvestable>(ast, entity),
        lod: read::<ModelLod>(ast, entity),
        flora: read::<FloraStation>(ast, entity),
        script: read::<ScriptRef>(ast, entity),
        props: read::<AuthoredProps>(ast, entity),
        particle: read::<ParticleSpec>(ast, entity),
        exclusion: read::<SpawnExclusionSpec>(ast, entity),
        portal: read::<PortalSpec>(ast, entity),
    };
    if specs.is_empty() {
        return;
    }
    if let Some(spec) = &specs.collider {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.body {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.destructible {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.harvest {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.lod {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.flora {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.script {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.particle {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.exclusion {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.portal {
        write_attrs(spec, attrs);
    }
    if let Some(props) = &specs.props {
        for (key, value) in props.to_attrs() {
            match attrs.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => attrs.push((key, value)),
            }
        }
    }
    record(origin, specs);
}

/// Componentes tipados extraídos de atributos, prontos a emitir.
#[derive(Default)]
pub struct LiftedSpecs {
    pub components: Vec<Box<dyn PartialReflect>>,
    /// Chaves de atributo que saem do `WorldNode`.
    pub consumed: Vec<String>,
}

/// Move para componentes os atributos com ida e volta exata. Chaves com
/// significado por tag (`radius`, `preset`, `at`…) só migram na sua tag.
pub fn lift_specs(tag: &str, attrs: &[(String, String)]) -> LiftedSpecs {
    let mut out = LiftedSpecs::default();
    fn take<T: AttrSpec + PartialReflect>(attrs: &[(String, String)], out: &mut LiftedSpecs) {
        if let Some((spec, keys)) = bevy_ext_scene::lift::<T>(attrs) {
            out.components.push(Box::new(spec));
            out.consumed.extend(keys.into_iter().map(str::to_string));
        }
    }
    take::<ColliderSpec>(attrs, &mut out);
    take::<BodySpec>(attrs, &mut out);
    take::<Destructible>(attrs, &mut out);
    take::<Harvestable>(attrs, &mut out);
    if matches!(tag, "GltfScene" | "GLTFLoader") {
        take::<ModelLod>(attrs, &mut out);
    }
    take::<FloraStation>(attrs, &mut out);
    take::<ScriptRef>(attrs, &mut out);
    if let Some((props, keys)) = AuthoredProps::from_attrs(attrs) {
        out.components.push(Box::new(props));
        out.consumed.extend(keys);
    }
    match tag {
        "ParticleSystem" => take::<ParticleSpec>(attrs, &mut out),
        "SpawnExclusion" => take::<SpawnExclusionSpec>(attrs, &mut out),
        "ScenePortal" => take::<PortalSpec>(attrs, &mut out),
        _ => {}
    }
    out
}

/// Texto `.bsn` de um componente (uma linha ou bloco, sem indentação), na
/// mesma sintaxe que o Jackdaw grava.
pub fn component_text(component: &dyn PartialReflect) -> String {
    let mut ast = SceneBsnAst::default();
    let patch = jackdaw_bsn::component_to_bsn_patch(component, registry());
    let entity = ast.create_entity_node(vec![patch]);
    ast.add_to_roots(entity);
    jackdaw_bsn::emit_scene(&ast).trim().to_string()
}

/// Resultado de [`migrate_text`].
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct MigrateReport {
    /// Nós com pelo menos um componente novo.
    pub nodes: usize,
    /// Componentes escritos, por tipo.
    pub components: std::collections::BTreeMap<String, usize>,
}

const WORLD_NODE_OPEN: &str = "bevy_world_xml::bsn::WorldNode {";
const PROP_PREFIX: &str = "bevy_world_xml::bsn::WorldProp { key: \"";

/// Move os atributos migráveis de cada `WorldNode` de um `.bsn` para
/// componentes tipados, no próprio texto (o resto do ficheiro fica igual).
/// Idempotente: um nó já migrado não tem os atributos e não muda.
pub fn migrate_text(text: &str) -> (String, MigrateReport) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut report = MigrateReport::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == WORLD_NODE_OPEN {
            let indent = &line[..line.len() - line.trim_start().len()];
            // `tag:` e `attrs: [` seguem; os props até `],` ao mesmo nível.
            let attrs_open = format!("{indent}    attrs: [");
            let attrs_close = format!("{indent}    ],");
            let Some(open) =
                (i + 1..lines.len().min(i + 4)).find(|&j| lines[j].trim_end() == attrs_open)
            else {
                out.push_str(line);
                i += 1;
                continue;
            };
            let Some(close) = (open + 1..lines.len()).find(|&j| lines[j].trim_end() == attrs_close)
            else {
                out.push_str(line);
                i += 1;
                continue;
            };
            let props: Vec<Option<(String, String)>> = lines[open + 1..close]
                .iter()
                .map(|l| parse_prop(l))
                .collect();
            if props.iter().any(Option::is_none) {
                out.push_str(line);
                i += 1;
                continue;
            }
            let attrs: Vec<(String, String)> = props.into_iter().flatten().collect();
            let tag = lines
                .get(i + 1)
                .and_then(|l| l.trim().strip_prefix("tag: \""))
                .and_then(|l| l.strip_suffix("\","))
                .unwrap_or_default();
            let lifted = lift_specs(tag, &attrs);
            if lifted.components.is_empty() {
                out.push_str(line);
                i += 1;
                continue;
            }
            report.nodes += 1;
            for component in &lifted.components {
                let name = component
                    .get_represented_type_info()
                    .map(|info| info.type_path_table().short_path().to_string())
                    .unwrap_or_default();
                *report.components.entry(name).or_default() += 1;
                for part in component_text(component.as_ref()).lines() {
                    out.push_str(indent);
                    out.push_str(part);
                    out.push('\n');
                }
            }
            for (k, l) in lines.iter().enumerate().take(close + 1).skip(i) {
                let consumed = k > open
                    && k < close
                    && attrs
                        .get(k - open - 1)
                        .is_some_and(|(key, _)| lifted.consumed.contains(key));
                if !consumed {
                    out.push_str(l);
                }
            }
            i = close + 1;
            continue;
        }
        out.push_str(line);
        i += 1;
    }
    (out, report)
}

fn parse_prop(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix(PROP_PREFIX)?;
    let rest = rest.strip_suffix(" },")?;
    let (key, rest) = rest.split_once("\", value: \"")?;
    let value = rest.strip_suffix('"')?;
    // Valores com escapes ficam como estão (não vale a pena arriscar).
    if key.contains('\\') || value.contains('\\') || value.contains('"') {
        return None;
    }
    Some((key.to_string(), value.to_string()))
}
