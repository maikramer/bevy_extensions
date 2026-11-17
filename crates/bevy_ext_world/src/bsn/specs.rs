//! Componentes tipados do `bevy_ext_scene` ↔ atributos do DSL.
//!
//! O documento pode trazer `ColliderSpec`/`BodySpec`/`Destructible`/
//! `Harvestable` em vez das strings `collider=`/`rigidbody=`/… no
//! `WorldNode`. O loader devolve-os ao parser do Viber como atributos
//! ([`apply_specs`]); a migração ([`lift_specs`]) move os atributos para os
//! componentes só quando a ida e volta é exata.

use std::any::TypeId;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, LazyLock, OnceLock, RwLock};

use bevy::reflect::{FromReflect, PartialReflect, Reflect, TypePath, TypeRegistry};
use bevy_ext_scene::{
    AttrSpec, AudioMixerSpec, AuthoredProps, BiomeRegionSpec, BodySpec, CameraSpec, ColliderSpec,
    Destructible, FloraStation, Harvestable, InteriorSceneSpec, ModelLod, MusicLayerSpec,
    NodeSpecs, ParticleSpec, PortalSpec, ScriptRef, SpawnExclusionSpec,
};

use crate::xml::NodeOrigin;
use jackdaw_bsn::SceneBsnAst;
use jackdaw_scene_types::{CustomProperties, PropertyValue};

fn registry() -> &'static TypeRegistry {
    static REGISTRY: OnceLock<TypeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = TypeRegistry::default();
        bevy_ext_scene::register_types(&mut registry);
        // As `prop-*` vivem no componente nativo do Jackdaw (o inspector do
        // editor tem painel próprio para ele).
        registry.register::<CustomProperties>();
        registry.register::<PropertyValue>();
        registry.register::<BTreeMap<String, PropertyValue>>();
        registry.register::<Cow<'static, str>>();
        registry
    })
}

fn patch_to_bsn_value(patch: &jackdaw_bsn::BsnPatch) -> Option<jackdaw_bsn::BsnValue> {
    match patch {
        jackdaw_bsn::BsnPatch::Struct(data) => {
            Some(jackdaw_bsn::BsnValue::Struct(data.clone()))
        }
        jackdaw_bsn::BsnPatch::TupleStruct(data) => {
            Some(jackdaw_bsn::BsnValue::TupleStruct(data.clone()))
        }
        jackdaw_bsn::BsnPatch::Type(tp) => Some(jackdaw_bsn::BsnValue::Type(tp.clone())),
        _ => None,
    }
}

fn read_from_patch<T>(patch: &jackdaw_bsn::BsnPatch) -> Option<T>
where
    T: Reflect + FromReflect + Default + TypePath,
{
    let value = patch_to_bsn_value(patch)?;
    let value = normalize_options(&value, TypeId::of::<T>(), registry());
    let partial = jackdaw_bsn::bsn_value_to_reflect(&value, TypeId::of::<T>(), registry(), None)?;
    let mut out = T::default();
    out.try_apply(partial.as_ref()).ok()?;
    Some(out)
}

/// Lê um componente tipado do nó (o caminho do load, por `type_path`).
fn read<T>(ast: &SceneBsnAst, entity: bevy::ecs::entity::Entity) -> Option<T>
where
    T: Reflect + FromReflect + Default + TypePath,
{
    let value = jackdaw_bsn::get_bsn_field(ast, entity, T::type_path(), "")?;
    let value = normalize_options(&value, TypeId::of::<T>(), registry());
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

#[inline]
fn origin_key(origin: &NodeOrigin) -> (Arc<Path>, Vec<u32>) {
    (origin.file.clone(), origin.path.clone())
}

/// Accumulates world-load spec/node/native records and flushes each lock once.
#[derive(Default)]
pub(crate) struct SpecsBatch {
    node_ids: Vec<((Arc<Path>, Vec<u32>), u64)>,
    specs: Vec<((Arc<Path>, Vec<u32>), Arc<NodeSpecs>)>,
    native: Vec<((Arc<Path>, Vec<u32>), Option<Arc<Vec<jackdaw_bsn::BsnPatch>>>)>,
    nodes: Vec<((Arc<Path>, Vec<u32>), Arc<RecordedNode>)>,
}

impl SpecsBatch {
    pub(crate) fn record_node_id(&mut self, origin: &NodeOrigin, id: u64) {
        self.node_ids.push((origin_key(origin), id));
    }

    pub(crate) fn apply_specs(
        &mut self,
        ast: &SceneBsnAst,
        entity: bevy::ecs::entity::Entity,
        origin: &NodeOrigin,
        attrs: &mut Vec<(String, String)>,
    ) {
        let specs = extract_node_specs(ast, entity);
        if specs.is_empty() {
            return;
        }
        write_spec_attrs(&specs, attrs);
        self.specs.push((origin_key(origin), Arc::new(specs)));
    }

    pub(crate) fn record_native(
        &mut self,
        ast: &SceneBsnAst,
        entity: bevy::ecs::entity::Entity,
        origin: &NodeOrigin,
    ) {
        let Some(patches) = collect_native_patches(ast, entity) else {
            return;
        };
        let value = if patches.is_empty() {
            None
        } else {
            Some(Arc::new(patches))
        };
        self.native.push((origin_key(origin), value));
    }

    pub(crate) fn record_node(&mut self, origin: &NodeOrigin, node: RecordedNode) {
        self.nodes.push((origin_key(origin), Arc::new(node)));
    }

    pub(crate) fn commit(self) {
        if !self.node_ids.is_empty()
            && let Ok(mut ids) = NODE_IDS.write()
        {
            ids.reserve(self.node_ids.len());
            for (k, v) in self.node_ids {
                ids.insert(k, v);
            }
        }
        if !self.specs.is_empty()
            && let Ok(mut index) = INDEX.write()
        {
            index.reserve(self.specs.len());
            for (k, v) in self.specs {
                index.insert(k, v);
            }
        }
        if !self.native.is_empty()
            && let Ok(mut index) = NATIVE.write()
        {
            for (k, v) in self.native {
                match v {
                    Some(list) => {
                        index.insert(k, list);
                    }
                    None if !index.is_empty() => {
                        index.remove(&k);
                    }
                    None => {}
                }
            }
        }
        if !self.nodes.is_empty()
            && let Ok(mut nodes) = NODES.write()
        {
            nodes.reserve(self.nodes.len());
            for (k, v) in self.nodes {
                nodes.insert(k, v);
            }
        }
    }
}

/// Specs lidos por nó (ficheiro + caminho do `NodeOrigin`), para o jogo os
/// anexar às entidades que o spawner do Viber cria (`SceneSource`).
static INDEX: LazyLock<RwLock<HashMap<(Arc<Path>, Vec<u32>), Arc<NodeSpecs>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// `SceneNodeId` (Jackdaw) de cada nó, pela origem — o último load ganha.
static NODE_IDS: LazyLock<RwLock<HashMap<(Arc<Path>, Vec<u32>), u64>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub(crate) fn record_node_id(origin: &NodeOrigin, id: u64) {
    if let Ok(mut ids) = NODE_IDS.write() {
        ids.insert(origin_key(origin), id);
    }
}

/// Patches de componentes que o loader não consome (o DSL não os conhece):
/// `AnimationSet`, camera rig, binds de UI, componentes do próprio jogo….
static NATIVE: LazyLock<RwLock<HashMap<(Arc<Path>, Vec<u32>), Arc<Vec<jackdaw_bsn::BsnPatch>>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Tipos que o loader já traduz para o DSL (ou que são estrutura do
/// documento): não passam como componentes nativos.
const CONSUMED_PREFIXES: &[&str] = &[
    "bevy_world_xml::",
    "bevy_transform::",
    "bevy_ecs::",
    "bevy_light::",
    "jackdaw::prefab::",
    "jackdaw_scene_types::node_id::",
    "jackdaw_scene_types::types::GltfSource",
    "jackdaw_scene_types::types::CustomProperties",
    "bevy_ext_scene",
    "bevy_ext_terrain",
];

fn collect_native_patches(
    ast: &SceneBsnAst,
    entity: bevy::ecs::entity::Entity,
) -> Option<Vec<jackdaw_bsn::BsnPatch>> {
    let patches = ast.get_patches(entity)?;
    Some(
        patches
            .0
            .iter()
            .filter_map(|&pe| ast.get_patch(pe))
            .filter(|patch| {
                jackdaw_bsn::patch_type_path(patch)
                    .is_some_and(|path| !CONSUMED_PREFIXES.iter().any(|p| path.starts_with(p)))
            })
            .cloned()
            .collect(),
    )
}

pub(crate) fn record_native(ast: &SceneBsnAst, entity: bevy::ecs::entity::Entity, origin: &NodeOrigin) {
    let Some(native) = collect_native_patches(ast, entity) else {
        return;
    };
    let key = origin_key(origin);
    if let Ok(mut index) = NATIVE.write() {
        if native.is_empty() {
            index.remove(&key);
        } else {
            index.insert(key, Arc::new(native));
        }
    }
}

/// Um nó tal como o loader o entregou à engine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordedNode {
    pub tag: String,
    /// Atributos autorados no `WorldNode` (antes de pose/specs/previews).
    pub raw_attrs: Vec<(String, String)>,
    /// Atributos finais (o que o parser da engine leu).
    pub attrs: Vec<(String, String)>,
    pub text: String,
    /// Tem nós filhos no DSL (um re-spawn isolado perdia-os).
    pub has_children: bool,
}

static NODES: LazyLock<RwLock<HashMap<(Arc<Path>, Vec<u32>), Arc<RecordedNode>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn record_node(origin: &NodeOrigin, node: RecordedNode) {
    if let Ok(mut nodes) = NODES.write() {
        nodes.insert(origin_key(origin), Arc::new(node));
    }
}

/// O nó (tag/atributos) de onde uma entidade veio.
pub fn node_for(origin: &NodeOrigin) -> Option<Arc<RecordedNode>> {
    NODES.read().ok()?.get(&origin_key(origin)).cloned()
}

/// Filhos directos registados de um nó (mesmo ficheiro, caminho + 1 índice),
/// por ordem. Um `<Include>`/prefab tem os filhos noutro ficheiro: não
/// aparecem aqui.
pub fn recorded_children(origin: &NodeOrigin) -> Vec<(NodeOrigin, Arc<RecordedNode>)> {
    let Ok(nodes) = NODES.read() else {
        return Vec::new();
    };
    let depth = origin.path.len();
    let mut out: Vec<(NodeOrigin, Arc<RecordedNode>)> = nodes
        .iter()
        .filter(|((file, path), _)| {
            file.as_ref() == origin.file.as_ref()
                && path.len() == depth + 1
                && path.starts_with(&origin.path)
        })
        .map(|((file, path), node)| {
            (
                NodeOrigin {
                    file: file.clone(),
                    path: path.clone(),
                },
                node.clone(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    out
}

/// A subárvore registada debaixo de `origin`, como a engine a leu, para
/// re-spawnar um nó com filhos. `None` quando um filho esperado não está
/// registado (vem de outro ficheiro): o re-spawn perdê-lo-ia.
pub fn recorded_subtree(origin: &NodeOrigin) -> Option<Vec<crate::xml::XmlNode>> {
    let mut out = Vec::new();
    for (child, node) in recorded_children(origin) {
        let children = if node.has_children {
            let kids = recorded_subtree(&child)?;
            if kids.is_empty() {
                return None;
            }
            kids
        } else {
            Vec::new()
        };
        out.push(crate::xml::XmlNode {
            tag: node.tag.clone(),
            attrs: node.attrs.clone(),
            text: node.text.clone(),
            children,
            origin: Some(Arc::new(child)),
        });
    }
    Some(out)
}

/// O `url` do modelo do nó de onde uma entidade veio (como no DSL:
/// `/assets/meshes/…glb`).
pub fn model_url_for(origin: &NodeOrigin) -> Option<String> {
    node_for(origin)?
        .attrs
        .iter()
        .find(|(key, _)| key == "url")
        .map(|(_, url)| url.clone())
}

/// Todas as chaves de atributo que os componentes tipados escrevem.
pub fn spec_attr_keys() -> &'static [&'static str] {
    static KEYS: OnceLock<Vec<&'static str>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys = Vec::new();
        keys.extend_from_slice(ColliderSpec::KEYS);
        keys.extend_from_slice(BodySpec::KEYS);
        keys.extend_from_slice(Destructible::KEYS);
        keys.extend_from_slice(Harvestable::KEYS);
        keys.extend_from_slice(ModelLod::KEYS);
        keys.extend_from_slice(FloraStation::KEYS);
        keys.extend_from_slice(ScriptRef::KEYS);
        keys.extend_from_slice(ParticleSpec::KEYS);
        keys.extend_from_slice(SpawnExclusionSpec::KEYS);
        keys.extend_from_slice(PortalSpec::KEYS);
        keys.extend_from_slice(CameraSpec::KEYS);
        keys.extend_from_slice(InteriorSceneSpec::KEYS);
        keys.extend_from_slice(AudioMixerSpec::KEYS);
        keys.extend_from_slice(MusicLayerSpec::KEYS);
        keys.extend_from_slice(BiomeRegionSpec::KEYS);
        keys
    })
}

/// Atributos de um nó editado ao vivo: os autorados que o editor enviou
/// (`raw`; sem eles, os do load), mais os derivados do load que o `raw` não
/// traz (nome, previews), depois os componentes tipados e a pose actual.
pub fn live_attrs(
    recorded: &RecordedNode,
    raw: Option<&[(String, String)]>,
    specs: &NodeSpecs,
    pose: Option<crate::bsn::Pose>,
) -> Vec<(String, String)> {
    // Atributos que vieram de um componente tipado (e não do `WorldNode`):
    // saem e voltam pelo `write_spec_attrs` — assim um spec REMOVIDO no
    // editor leva o seu atributo consigo.
    let from_spec = |key: &str| {
        !recorded.raw_attrs.iter().any(|(k, _)| k == key)
            && (spec_attr_keys().contains(&key) || key.starts_with("prop-"))
    };
    let mut attrs: Vec<(String, String)> = match raw {
        Some(raw) => {
            let mut attrs = raw.to_vec();
            for (key, value) in &recorded.attrs {
                let derived = !recorded.raw_attrs.iter().any(|(k, _)| k == key);
                if derived && !from_spec(key) && !attrs.iter().any(|(k, _)| k == key) {
                    attrs.push((key.clone(), value.clone()));
                }
            }
            attrs
        }
        None => recorded
            .attrs
            .iter()
            .filter(|(key, _)| !from_spec(key))
            .cloned()
            .collect(),
    };
    write_spec_attrs(specs, &mut attrs);
    if let Some(pose) = pose {
        crate::bsn::pose::write_pose_attrs(&mut attrs, pose);
    }
    attrs
}

/// Componentes autorados no editor que o DSL não transporta, para o jogo os
/// aplicar à entidade do nó (`jackdaw_bsn::apply_component_patch`).
pub fn native_components_for(origin: &NodeOrigin) -> Option<Arc<Vec<jackdaw_bsn::BsnPatch>>> {
    NATIVE.read().ok()?.get(&origin_key(origin)).cloned()
}

/// [`record_node_id`] para o jogo (nós criados no editor durante o Play).
pub fn record_node_id_pub(origin: &NodeOrigin, id: u64) {
    record_node_id(origin, id);
}

/// O `SceneNodeId` do nó de onde uma entidade veio (a sua `SceneSource`),
/// se o documento o tinha: é por ele que o editor endereça as edições do
/// Play (`ControlEvent::NodeComponents`).
pub fn scene_node_id_for(origin: &NodeOrigin) -> Option<u64> {
    NODE_IDS.read().ok()?.get(&origin_key(origin)).copied()
}

/// Os componentes tipados do nó de onde uma entidade veio, se o documento os
/// tinha (o último load de cada ficheiro ganha).
pub fn specs_for(origin: &NodeOrigin) -> Option<Arc<NodeSpecs>> {
    INDEX.read().ok()?.get(&origin_key(origin)).cloned()
}

fn record(origin: &NodeOrigin, specs: NodeSpecs) {
    if let Ok(mut index) = INDEX.write() {
        index.insert(origin_key(origin), Arc::new(specs));
    }
}

/// Lê os componentes tipados da entidade (sem registar nada).
pub(crate) fn extract_node_specs(
    ast: &SceneBsnAst,
    entity: bevy::ecs::entity::Entity,
) -> NodeSpecs {
    NodeSpecs {
        collider: read::<ColliderSpec>(ast, entity),
        body: read::<BodySpec>(ast, entity),
        destructible: read::<Destructible>(ast, entity),
        harvest: read::<Harvestable>(ast, entity),
        lod: read::<ModelLod>(ast, entity),
        flora: read::<FloraStation>(ast, entity),
        script: read::<ScriptRef>(ast, entity),
        props: merged_props(
            read::<AuthoredProps>(ast, entity),
            read::<CustomProperties>(ast, entity),
        ),
        particle: read::<ParticleSpec>(ast, entity),
        exclusion: read::<SpawnExclusionSpec>(ast, entity),
        portal: read::<PortalSpec>(ast, entity),
        camera: read::<CameraSpec>(ast, entity),
        interior: read::<InteriorSceneSpec>(ast, entity),
        mixer: read::<AudioMixerSpec>(ast, entity),
        music_layer: read::<MusicLayerSpec>(ast, entity),
        biome: read::<BiomeRegionSpec>(ast, entity),
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
    let specs = extract_node_specs(ast, entity);
    if specs.is_empty() {
        return;
    }
    write_spec_attrs(&specs, attrs);
    record(origin, specs);
}

/// Escreve nos `attrs` os atributos equivalentes aos componentes tipados
/// (o componente ganha a um atributo homónimo).
pub fn write_spec_attrs(specs: &NodeSpecs, attrs: &mut Vec<(String, String)>) {
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
    if let Some(spec) = &specs.camera {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.interior {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.mixer {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.music_layer {
        write_attrs(spec, attrs);
    }
    if let Some(spec) = &specs.biome {
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
}

/// Regista (ou substitui) os componentes tipados de um nó — o jogo chama-o
/// depois de re-spawnar um nó editado ao vivo.
pub fn set_node_specs(origin: &NodeOrigin, specs: NodeSpecs) {
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
    lift_specs_skipping(tag, attrs, &|_| false)
}

/// [`lift_specs`] sem os tipos para que `skip(type_path)` é verdadeiro.
///
/// A migração passa aqui os componentes que o nó **já tem**: um ficheiro
/// voltado a gravar pelo editor pode ter o componente e o atributo
/// homónimo ao mesmo tempo (o componente ganha no load), e voltar a levantá-lo
/// duplicava o componente no documento.
pub fn lift_specs_skipping(
    tag: &str,
    attrs: &[(String, String)],
    skip: &dyn Fn(&str) -> bool,
) -> LiftedSpecs {
    let mut out = LiftedSpecs::default();
    fn take<T: AttrSpec + PartialReflect + TypePath>(
        attrs: &[(String, String)],
        out: &mut LiftedSpecs,
        skip: &dyn Fn(&str) -> bool,
    ) {
        if skip(T::type_path()) {
            return;
        }
        if let Some((spec, keys)) = bevy_ext_scene::lift::<T>(attrs) {
            out.components.push(Box::new(spec));
            out.consumed.extend(keys.into_iter().map(str::to_string));
        }
    }
    take::<ColliderSpec>(attrs, &mut out, skip);
    take::<BodySpec>(attrs, &mut out, skip);
    take::<Destructible>(attrs, &mut out, skip);
    take::<Harvestable>(attrs, &mut out, skip);
    if matches!(tag, "GltfScene" | "GLTFLoader") {
        take::<ModelLod>(attrs, &mut out, skip);
    }
    take::<FloraStation>(attrs, &mut out, skip);
    take::<ScriptRef>(attrs, &mut out, skip);
    if !skip(CustomProperties::type_path())
        && let Some((props, keys)) = AuthoredProps::from_attrs(attrs)
    {
        out.components.push(Box::new(custom_from_authored(&props)));
        out.consumed.extend(keys);
    }
    match tag.to_ascii_lowercase().as_str() {
        "particlesystem" => take::<ParticleSpec>(attrs, &mut out, skip),
        "spawnexclusion" => take::<SpawnExclusionSpec>(attrs, &mut out, skip),
        "sceneportal" => take::<PortalSpec>(attrs, &mut out, skip),
        // Câmara e áudio: `at`/`size`/`id`/`height`/`target` repetem-se
        // noutras tags — só migram na sua.
        "orbitcamera" | "thirdpersoncamera" => take::<CameraSpec>(attrs, &mut out, skip),
        "interiorscene" => take::<InteriorSceneSpec>(attrs, &mut out, skip),
        "audiomixer" => take::<AudioMixerSpec>(attrs, &mut out, skip),
        "musiclayer" => take::<MusicLayerSpec>(attrs, &mut out, skip),
        "biomeregion" => take::<BiomeRegionSpec>(attrs, &mut out, skip),
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
            // Componentes que o nó já tem (o bloco da entidade começa depois
            // do separador `--`): não são levantados outra vez.
            let block_start = lines[..i]
                .iter()
                .rposition(|line| line.trim() == "--")
                .map_or(0, |at| at + 1);
            let already = |type_path: &str| {
                lines[block_start..i]
                    .iter()
                    .any(|line| line.trim_start().starts_with(type_path))
            };
            let lifted = lift_specs_skipping(tag, &attrs, &already);
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

/// Texto de um [`PropertyValue`] tal como o DSL o espera numa `prop-*`.
pub fn property_text(value: &PropertyValue) -> String {
    match value {
        PropertyValue::Bool(v) => v.to_string(),
        PropertyValue::Int(v) => v.to_string(),
        PropertyValue::Float(v) => v.to_string(),
        PropertyValue::String(v) => v.to_string(),
        PropertyValue::Vec2(v) => format!("{} {}", v.x, v.y),
        PropertyValue::Vec3(v) => format!("{} {} {}", v.x, v.y, v.z),
        PropertyValue::Color(c) => {
            let c = c.to_srgba();
            format!("{} {} {} {}", c.red, c.green, c.blue, c.alpha)
        }
        // Uma entidade não tem forma no DSL: o script não a pode ler.
        PropertyValue::Entity(_) => String::new(),
    }
}

/// As `prop-*` de um nó: as antigas ([`AuthoredProps`]) e as do componente
/// do Jackdaw ([`CustomProperties`]); numa chave repetida ganha o Jackdaw.
pub fn merged_props(
    authored: Option<AuthoredProps>,
    custom: Option<CustomProperties>,
) -> Option<AuthoredProps> {
    let Some(custom) = custom else {
        return authored;
    };
    let mut props = authored.unwrap_or_default();
    for (key, value) in custom.properties.iter() {
        if matches!(value, PropertyValue::Entity(_)) {
            continue;
        }
        let text = property_text(value);
        match props.props.iter_mut().find(|prop| prop.key == *key) {
            Some(prop) => prop.value = text,
            None => props.props.push(bevy_ext_scene::AuthoredProp {
                key: key.clone(),
                value: text,
            }),
        }
    }
    (!props.props.is_empty()).then_some(props)
}

/// [`AuthoredProps`] → [`CustomProperties`] com valores `String`: a ida e
/// volta pelo DSL é exata ("0.50" continua "0.50", "007" não vira 7).
pub fn custom_from_authored(props: &AuthoredProps) -> CustomProperties {
    let mut custom = CustomProperties::default();
    for prop in &props.props {
        custom.properties.insert(
            prop.key.clone(),
            PropertyValue::String(Cow::Owned(prop.value.clone())),
        );
    }
    custom
}

const AUTHORED_OPEN: &str = "bevy_ext_scene::AuthoredProps {";
const AUTHORED_PROP: &str = "bevy_ext_scene::AuthoredProp { key: \"";

/// Troca cada bloco `AuthoredProps { props: [...] }` de um `.bsn` pelo
/// [`CustomProperties`] equivalente, no próprio texto. Um bloco com escapes
/// ou chaves repetidas fica como está. Idempotente. Devolve quantos trocou.
pub fn migrate_props_text(text: &str) -> (String, usize) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut migrated = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() != AUTHORED_OPEN {
            out.push_str(line);
            i += 1;
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        let close = format!("{indent}}}");
        let Some(end) = (i + 1..lines.len()).find(|&j| lines[j].trim_end() == close) else {
            out.push_str(line);
            i += 1;
            continue;
        };
        let props: Option<Vec<(String, String)>> = lines[i + 1..end]
            .iter()
            .map(|l| l.trim())
            .filter(|l| !matches!(*l, "props: [" | "]," | "props: [],"))
            .map(parse_authored_prop)
            .collect();
        let unique = props.as_ref().is_some_and(|props| {
            let mut keys: Vec<&str> = props.iter().map(|(k, _)| k.as_str()).collect();
            keys.sort_unstable();
            keys.windows(2).all(|w| w[0] != w[1])
        });
        let Some(props) = props.filter(|_| unique) else {
            for l in &lines[i..=end] {
                out.push_str(l);
            }
            i = end + 1;
            continue;
        };
        let authored = AuthoredProps {
            props: props
                .into_iter()
                .map(|(key, value)| bevy_ext_scene::AuthoredProp { key, value })
                .collect(),
        };
        for part in component_text(&custom_from_authored(&authored)).lines() {
            out.push_str(indent);
            out.push_str(part);
            out.push('\n');
        }
        migrated += 1;
        i = end + 1;
    }
    (out, migrated)
}

fn parse_authored_prop(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix(AUTHORED_PROP)?;
    let rest = rest.strip_suffix(" },")?;
    // O emissor omite o campo com o valor por defeito: `{ key: "x" }` = "".
    if let Some(key) = rest.strip_suffix('"')
        && !key.contains('"')
        && !key.contains('\\')
    {
        return Some((key.to_string(), String::new()));
    }
    let (key, rest) = rest.split_once("\", value: \"")?;
    let value = rest.strip_suffix('"')?;
    if key.contains('\\') || value.contains('\\') || value.contains('"') {
        return None;
    }
    Some((key.to_string(), value.to_string()))
}

const OPTION_SOME: &str = "core::option::Option::Some";

/// O emissor do Jackdaw grava um `Option<T>` com valor como o valor nu
/// (`preset: "fire"`), mas o `bsn_value_to_reflect` só aceita
/// `core::option::Option::Some("fire")` — o nu lia como `None` (2.5k
/// partículas do mundo perdiam o preset). Percorre o valor contra o tipo e
/// embrulha em `Some(..)` todo o valor nu num campo `Option<T>`.
pub fn normalize_options(
    value: &jackdaw_bsn::BsnValue,
    type_id: TypeId,
    registry: &TypeRegistry,
) -> jackdaw_bsn::BsnValue {
    use bevy::reflect::{TypeInfo, enums::VariantInfo};
    use jackdaw_bsn::{BsnField, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue};
    let Some(info) = registry.get(type_id).map(|reg| reg.type_info()) else {
        return value.clone();
    };
    match info {
        TypeInfo::Enum(enum_info) if enum_info.type_path().starts_with("core::option::Option<") => {
            let inner = match enum_info.variant("Some") {
                Some(VariantInfo::Tuple(tuple)) => tuple.field_at(0).map(|f| f.type_id()),
                _ => None,
            };
            let explicit = match value {
                BsnValue::Type(path) => path.ends_with("None") || path.ends_with("Some"),
                BsnValue::TupleStruct(data) => data.type_path.ends_with("Some"),
                _ => false,
            };
            if explicit {
                if let (BsnValue::TupleStruct(data), Some(inner)) = (value, inner) {
                    return BsnValue::TupleStruct(BsnTupleStructData {
                        type_path: data.type_path.clone(),
                        values: data
                            .values
                            .iter()
                            .map(|v| normalize_options(v, inner, registry))
                            .collect(),
                    });
                }
                return value.clone();
            }
            let inner_value = match inner {
                Some(inner) => normalize_options(value, inner, registry),
                None => value.clone(),
            };
            BsnValue::TupleStruct(BsnTupleStructData {
                type_path: OPTION_SOME.to_string(),
                values: vec![inner_value],
            })
        }
        TypeInfo::Struct(struct_info) => match value {
            BsnValue::Struct(data) => BsnValue::Struct(BsnStructData {
                type_path: data.type_path.clone(),
                fields: BsnStructFields(
                    data.fields
                        .0
                        .iter()
                        .map(|field| BsnField {
                            name: field.name.clone(),
                            value: match struct_info.field(&field.name) {
                                Some(info) => normalize_options(&field.value, info.type_id(), registry),
                                None => field.value.clone(),
                            },
                        })
                        .collect(),
                ),
            }),
            _ => value.clone(),
        },
        TypeInfo::List(list) => match value {
            BsnValue::List(items) => BsnValue::List(
                items
                    .iter()
                    .map(|item| normalize_options(item, list.item_ty().id(), registry))
                    .collect(),
            ),
            _ => value.clone(),
        },
        TypeInfo::Enum(enum_info) => match value {
            // Variante tuple de um enum nosso (`ColliderShape::Mesh {..}` é
            // struct-variant; aqui só se desce nas tuple-variants).
            BsnValue::TupleStruct(data) => {
                let name = data.type_path.rsplit("::").next().unwrap_or_default();
                match enum_info.variant(name) {
                    Some(VariantInfo::Tuple(tuple)) => BsnValue::TupleStruct(BsnTupleStructData {
                        type_path: data.type_path.clone(),
                        values: data
                            .values
                            .iter()
                            .enumerate()
                            .map(|(i, v)| match tuple.field_at(i) {
                                Some(f) => normalize_options(v, f.type_id(), registry),
                                None => v.clone(),
                            })
                            .collect(),
                    }),
                    _ => value.clone(),
                }
            }
            BsnValue::Struct(data) => {
                let name = data.type_path.rsplit("::").next().unwrap_or_default();
                match enum_info.variant(name) {
                    Some(VariantInfo::Struct(variant)) => BsnValue::Struct(BsnStructData {
                        type_path: data.type_path.clone(),
                        fields: BsnStructFields(
                            data.fields
                                .0
                                .iter()
                                .map(|field| BsnField {
                                    name: field.name.clone(),
                                    value: match variant.field(&field.name) {
                                        Some(info) => {
                                            normalize_options(&field.value, info.type_id(), registry)
                                        }
                                        None => field.value.clone(),
                                    },
                                })
                                .collect(),
                        ),
                    }),
                    _ => value.clone(),
                }
            }
            _ => value.clone(),
        },
        _ => value.clone(),
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;

    fn read_text<T>(text: &str) -> Option<T>
    where
        T: Reflect + FromReflect + Default + TypePath,
    {
        let ast = jackdaw_bsn::parse_bsn_text(text).unwrap();
        read::<T>(&ast, ast.roots[0])
    }

    #[test]
    fn option_nu_le_como_some() {
        let spec: ParticleSpec =
            read_text("#\"p\"\nbevy_ext_scene::ParticleSpec {\n    preset: \"fire\",\n}\n").unwrap();
        assert_eq!(spec.preset.as_deref(), Some("fire"));
        let spec: ParticleSpec = read_text(
            "#\"p\"\nbevy_ext_scene::ParticleSpec {\n    preset: core::option::Option::Some(\"dust\"),\n}\n",
        )
        .unwrap();
        assert_eq!(spec.preset.as_deref(), Some("dust"));
        let spec: ParticleSpec = read_text(
            "#\"p\"\nbevy_ext_scene::ParticleSpec {\n    preset: core::option::Option::None,\n}\n",
        )
        .unwrap();
        assert_eq!(spec.preset, None);
    }

    #[test]
    fn option_dentro_de_option_de_struct() {
        // `emitter: Some(ParticleEmitter { start_color: "…" (nu) })`.
        let text = "#\"p\"\nbevy_ext_scene::ParticleSpec {\n    emitter: core::option::Option::Some(bevy_ext_scene::ParticleEmitter { preset: \"fire\", emission_rate: 18.0, start_color: \"#ff8800\" }),\n}\n";
        let spec: ParticleSpec = read_text(text).unwrap();
        let emitter = spec.emitter.expect("emissor");
        assert_eq!(emitter.start_color.as_deref(), Some("#ff8800"));
        assert_eq!(emitter.emission_rate, 18.0);
    }

    #[test]
    fn todos_os_specs_do_emissor_voltam_iguais() {
        // O que o `component_text` (= emissor do editor) grava tem de ler
        // de volta igual, para cada spec com `Option`.
        let particle = ParticleSpec {
            preset: Some("leaves".into()),
            emitter: None,
        };
        let text = format!("#\"p\"\n{}\n", component_text(&particle));
        assert_eq!(read_text::<ParticleSpec>(&text), Some(particle), "{text}");
        let lod = ModelLod {
            lod1: Some("/a_lod1.glb".into()),
            lod2: None,
            near: Some(50.0),
            mid: None,
        };
        let text = format!("#\"p\"\n{}\n", component_text(&lod));
        assert_eq!(read_text::<ModelLod>(&text), Some(lod), "{text}");
    }
}

const WORLD_NODE_LINE: &str = "bevy_world_xml::bsn::WorldNode {";

/// Fim do bloco que abre em `lines[open]` (contagem de chavetas fora de
/// strings).
fn block_end(lines: &[&str], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (index, line) in lines.iter().enumerate().skip(open) {
        let mut in_str = false;
        let mut escaped = false;
        for c in line.chars() {
            match c {
                _ if escaped => escaped = false,
                '\\' if in_str => escaped = true,
                '"' => in_str = !in_str,
                '{' | '[' | '(' if !in_str => depth += 1,
                '}' | ']' | ')' if !in_str => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 {
            return Some(index);
        }
    }
    None
}

fn starts_node_boundary(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("#\"") || t == "--" || t.starts_with(']') || t.starts_with("bevy_ecs::hierarchy::Children [")
}

/// Dá um `SceneNodeId` a cada nó com `WorldNode` que não o tem, inserindo
/// só a linha do id a seguir ao bloco `WorldNode` (o resto do ficheiro fica
/// byte a byte). Com ids, os nós de um módulo aberto no seu separador são
/// endereçáveis pelas edições ao vivo do Play. Idempotente. Devolve quantos
/// cunhou.
pub fn mint_node_ids_text(text: &str, mut mint: impl FnMut() -> u64) -> (String, usize) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut insert_after: Vec<(usize, String)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != WORLD_NODE_LINE {
            continue;
        }
        let Some(end) = block_end(&lines, index) else {
            continue;
        };
        // Componentes do próprio nó: do início do nó até à fronteira seguinte.
        let start = (0..index)
            .rev()
            .find(|&i| starts_node_boundary(lines[i]))
            .map_or(0, |i| i + 1);
        let stop = (end + 1..lines.len())
            .find(|&i| starts_node_boundary(lines[i]))
            .unwrap_or(lines.len());
        let own = lines[start..stop]
            .iter()
            .any(|l| l.trim_start().starts_with("jackdaw_scene_types::node_id::SceneNodeId("));
        if own {
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        insert_after.push((
            end,
            format!("{indent}jackdaw_scene_types::node_id::SceneNodeId({})\n", mint()),
        ));
    }
    let count = insert_after.len();
    let mut out = String::with_capacity(text.len() + count * 64);
    let mut pending = insert_after.into_iter().peekable();
    for (index, line) in lines.iter().enumerate() {
        out.push_str(line);
        while pending.peek().is_some_and(|(at, _)| *at == index) {
            if !line.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&pending.next().expect("peeked").1);
        }
    }
    (out, count)
}

#[cfg(test)]
mod node_id_tests {
    use super::*;

    const DOC: &str = "#\"world\"
bevy_world_xml::bsn::WorldNode {
    tag: \"world\",
    attrs: [],
    text: \"a { b } [c]\",
}
bevy_ecs::hierarchy::Children [
    #\"pedra\"
    bevy_world_xml::bsn::WorldNode {
        tag: \"GltfScene\",
        attrs: [
            bevy_world_xml::bsn::WorldProp { key: \"url\", value: \"/a.glb\" },
        ],
        text: \"\",
    }
    --
    #\"ja-tem\"
    jackdaw_scene_types::node_id::SceneNodeId(4294967999)
    bevy_world_xml::bsn::WorldNode {
        tag: \"Entity\",
        attrs: [],
        text: \"\",
    }
]
";

    #[test]
    fn cunha_so_quem_nao_tem_e_le_de_volta() {
        let mut next = 1u64 << 40;
        let (text, n) = mint_node_ids_text(DOC, || {
            next += 1;
            next
        });
        assert_eq!(n, 2, "{text}");
        let ast = jackdaw_bsn::parse_bsn_text(&text).expect("continua válido");
        let root = ast.roots[0];
        assert!(ast.stable_id_of(root).is_some());
        let (again, n2) = mint_node_ids_text(&text, || unreachable!());
        assert_eq!((again.as_str(), n2), (text.as_str(), 0), "idempotente");
        // O resto do ficheiro fica igual: só linhas acrescentadas.
        let removed: String = text
            .lines()
            .filter(|l| !l.contains("SceneNodeId(1099511627"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(removed, DOC);
    }
}

#[cfg(test)]
mod custom_props_tests {
    use super::*;

    fn read_back(text: &str) -> (Option<AuthoredProps>, Option<CustomProperties>) {
        let ast = jackdaw_bsn::parse_bsn_text(text).expect("bsn válido");
        let entity = ast.roots[0];
        (
            read::<AuthoredProps>(&ast, entity),
            read::<CustomProperties>(&ast, entity),
        )
    }

    const OLD: &str = "\
bevy_ext_scene::AuthoredProps {
    props: [
        bevy_ext_scene::AuthoredProp { key: \"cinematic\", value: \"campo-de-batalha\" },
        bevy_ext_scene::AuthoredProp { key: \"radius\", value: \"0.50\" },
        bevy_ext_scene::AuthoredProp { key: \"group\" },
    ],
}
";

    #[test]
    fn migra_e_le_de_volta_igual() {
        let (text, n) = migrate_props_text(OLD);
        assert_eq!(n, 1, "{text}");
        assert!(text.contains("CustomProperties"), "{text}");
        assert!(!text.contains("AuthoredProps"), "{text}");
        let (again, n2) = migrate_props_text(&text);
        assert_eq!((again.as_str(), n2), (text.as_str(), 0), "idempotente");

        let (authored, custom) = read_back(&text);
        assert!(authored.is_none());
        let merged = merged_props(None, custom).expect("props");
        assert_eq!(merged.get("cinematic"), Some("campo-de-batalha"));
        assert_eq!(merged.get("radius"), Some("0.50"), "texto exato");
        assert_eq!(merged.get("group"), Some(""), "valor omitido = vazio");
    }

    #[test]
    fn jackdaw_ganha_numa_chave_repetida() {
        let authored = AuthoredProps {
            props: vec![bevy_ext_scene::AuthoredProp {
                key: "radius".into(),
                value: "1".into(),
            }],
        };
        let mut custom = CustomProperties::default();
        custom
            .properties
            .insert("radius".into(), PropertyValue::Float(2.5));
        custom
            .properties
            .insert("on".into(), PropertyValue::Bool(true));
        let merged = merged_props(Some(authored), Some(custom)).unwrap();
        assert_eq!(merged.get("radius"), Some("2.5"));
        assert_eq!(merged.get("on"), Some("true"));
    }

    #[test]
    fn lift_emite_o_componente_do_jackdaw() {
        let attrs = vec![("prop-cinematic".to_string(), "x".to_string())];
        let lifted = lift_specs("Entity", &attrs);
        let names: Vec<String> = lifted
            .components
            .iter()
            .filter_map(|c| c.get_represented_type_info())
            .map(|i| i.type_path().to_string())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("CustomProperties")), "{names:?}");
    }
}

