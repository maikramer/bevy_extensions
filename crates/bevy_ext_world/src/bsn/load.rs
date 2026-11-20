//! Leitura de `.bsn` para o DSL de mundo do Viber.
//!
//! O documento não é aplicado como ECS do jogo: cada entidade traz um
//! [`WorldNode`] com a tag/attrs/texto originais. O loader reconstrói os
//! [`XmlNode`] e o `parse_world`/spawner existentes fazem o resto. As
//! componentes nativas do Jackdaw (`Transform`, `Name`, `GltfSource`, luzes)
//! são traduzidas de volta para atributos XML — é isso que faz o editor servir
//! para mover props e ajustar luzes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use bevy::math::{Quat, Vec3};
use jackdaw_bsn::{BsnPatch, BsnStructData, BsnValue, SceneBsnAst, parse_bsn_text};

use crate::bsn::components::{WorldNode, WorldProp, display_name};
use crate::bsn::pose::{Pose, format_f32, format_vec3, rotation_to_direction, write_pose_attrs};
use crate::xml::include::{
    INCLUDE_POSE_KEYS, LoadedWorld, MAX_INCLUDE_DEPTH, resolve_include_path,
};
use crate::xml::{NodeOrigin, XmlNode};

const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
const WORLD_ORIGIN: &str = "bevy_world_xml::bsn::WorldOrigin";
const ISA: &str = "jackdaw::prefab::components::IsA";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const GLTF_SOURCE: &str = "jackdaw_scene_types::types::GltfSource";
const POINT_LIGHT: &str = "bevy_light::point_light::PointLight";
const RECT_LIGHT: &str = "bevy_light::rect_light::RectLight";
const DIRECTIONAL_LIGHT: &str = "bevy_light::directional_light::DirectionalLight";

/// Lê um mundo `.bsn` a partir da raiz do projeto.
pub fn load_bsn_world(path: &Path) -> Result<LoadedWorld> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("world file not found: {}", path.display()))?;
    let root_dir = crate::project::root_of(&canonical);
    load_bsn_world_in(&canonical, &root_dir)
}

/// [`load_bsn_world`] com uma raiz explícita para os `<Include src="/…">`.
///
/// Memoizado no processo (só a última leitura): o arranque lê o MESMO mundo
/// duas vezes — as definições (`SceneSettingsPlugin`) e a cena da engine — e
/// cada resolução de prefabs custa ~1,4 s no `world.bsn` do Vael. A entrada
/// só serve enquanto nenhum dos documentos lidos mudar (mtime + tamanho);
/// gravar no editor invalida-a.
pub fn load_bsn_world_in(path: &Path, root_dir: &Path) -> Result<LoadedWorld> {
    let path = path
        .canonicalize()
        .with_context(|| format!("world file not found: {}", path.display()))?;
    let root_dir = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf());
    if let Some(hit) = memo::get(&path, &root_dir) {
        return Ok(hit);
    }
    let loaded = load_bsn_world_uncached(&path, &root_dir)?;
    memo::put(&path, &root_dir, &loaded);
    Ok(loaded)
}

/// Os documentos alcançáveis a partir de `path` — o próprio mais os documentos
/// que referencia (`IsA { source }`, `WorldProp { key: "src" }` e os
/// `<Include>` dos `.xml` legados), lidos como TEXTO — autoram alguma de
/// `tags`?
///
/// Existe para decidir se vale a pena **parsear** um mundo só para ler
/// configurações de shader: o parse com resolução de prefabs custa ~1,4 s no
/// `world.bsn` do Vael e, quando o documento não autora `<Sky>`/`<DayCycle>`/
/// `<Water>`, o resultado são os defaults. Aqui é uma busca de substring sobre
/// os bytes (milissegundos).
///
/// O `.bsn` é RON do Jackdaw: o tag aparece como `tag: "Sky"` e as referências
/// como `source: "…/x.bsn"` / `value: "…/x.bsn"`. Seguem-se **todas** as
/// strings terminadas em `.bsn`/`.xml` — de mais, nunca de menos: seguir um
/// caminho a mais só custa uns bytes lidos.
pub fn authors_tags(path: &Path, root_dir: &Path, tags: &[&str]) -> bool {
    /// O `world.bsn` do Vael alcança ~537 documentos (módulos, 158 prefabs e
    /// `origin_file`); a 2 048 o tecto só existe para um grafo patológico.
    const MAX_FILES: usize = 2048;
    /// Ler mais do que isto já custa mais do que o parse que se quer evitar.
    const MAX_BYTES: usize = 64 * 1024 * 1024;
    let mut pending = vec![path.to_path_buf()];
    let mut seen: std::collections::HashSet<std::path::PathBuf> = Default::default();
    let mut read = 0usize;
    while let Some(next) = pending.pop() {
        let canonical = next.canonicalize().unwrap_or(next);
        if !seen.insert(canonical.clone()) {
            continue; // ciclo de includes
        }
        if seen.len() > MAX_FILES {
            return true;
        }
        let Ok(bytes) = std::fs::read(&canonical) else {
            continue;
        };
        read += bytes.len();
        if read > MAX_BYTES {
            return true;
        }
        let dir = canonical.parent().unwrap_or(root_dir);
        if scan_document(&bytes, tags, &mut pending, dir, root_dir) {
            return true;
        }
    }
    false
}

/// Uma passagem sobre o documento: compara os valores de `tag:` com `tags` e
/// recolhe as referências a outros documentos.
///
/// Um só varrimento (e não um por tag) porque o Vael alcança ~64 MB de `.bsn`:
/// cinco passagens custavam ~1 s em debug, e o objectivo é ser mais barato do
/// que o parse que se quer evitar.
fn scan_document(
    bytes: &[u8],
    tags: &[&str],
    pending: &mut Vec<std::path::PathBuf>,
    dir: &Path,
    root_dir: &Path,
) -> bool {
    // Mundo `.xml` legado: aí o tag é `<Sky>` a sério (o `.bsn` é RON e não
    // tem tags XML). Só se paga a varredura quando o documento tem marcas XML.
    if find_from(bytes, 0, b"<world").is_some() || find_from(bytes, 0, b"<Include").is_some() {
        for tag in tags {
            let mut needle = Vec::with_capacity(tag.len() + 1);
            needle.push(b'<');
            needle.extend_from_slice(tag.as_bytes());
            if find_from(bytes, 0, &needle).is_some() {
                return true;
            }
        }
    }
    let mut at = 0;
    while let Some(offset) = bytes[at..].iter().position(|b| *b == b'"' || *b == b'\'') {
        let start = at + offset;
        let Some(value) = quoted(bytes, start) else {
            at = start + 1;
            continue;
        };
        // `tag: "X"` — o valor está precedido de `tag` + espaços + `:`.
        if tags.contains(&value) && preceded_by_tag_key(bytes, start) {
            return true;
        }
        if value.ends_with(".bsn") || value.ends_with(".xml") {
            pending.push(prefer_bsn(resolve_include_path(value, dir, root_dir)));
        }
        at = start + value.len() + 2;
    }
    // `.xml` legado: `<Include src="…"/>` (o `.bsn` não tem tags XML).
    if find_from(bytes, 0, b"<Include").is_some() {
        let mut cursor = 0;
        while let Some(offset) = find_from(bytes, cursor, b"<Include") {
            let end = bytes[offset..]
                .iter()
                .position(|b| *b == b'>')
                .map_or(bytes.len(), |at| offset + at);
            if let Some(src) = attribute_text(&bytes[offset..end], "src") {
                pending.push(prefer_bsn(resolve_include_path(&src, dir, root_dir)));
            }
            cursor = end.saturating_add(1).max(offset + 1);
        }
    }
    false
}

/// A string que começa em `at` é o valor de um `tag`? (para trás: espaços, `:`,
/// espaços, a palavra `tag`).
fn preceded_by_tag_key(bytes: &[u8], at: usize) -> bool {
    let mut cursor = at;
    while matches!(bytes.get(cursor.wrapping_sub(1)), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        cursor -= 1;
    }
    if bytes.get(cursor.wrapping_sub(1)) != Some(&b':') {
        return false;
    }
    cursor -= 1;
    while matches!(bytes.get(cursor.wrapping_sub(1)), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        cursor -= 1;
    }
    let key = b"tag";
    cursor >= key.len() && bytes.get(cursor - key.len()..cursor) == Some(&key[..])
}



/// A string entre aspas que começa em `at` (`"…"`, `'…'` ou `#"…"`).
fn quoted(bytes: &[u8], at: usize) -> Option<&str> {
    let at = if bytes.get(at) == Some(&b'#') { at + 1 } else { at };
    let quote = *bytes.get(at)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let body = &bytes[at + 1..];
    let end = body.iter().position(|b| *b == quote)?;
    std::str::from_utf8(&body[..end]).ok()
}

fn skip_spaces(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

fn find_from(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// Valor de `name="…"` (ou `'…'`) dentro de uma tag.
fn attribute_text(tag: &[u8], name: &str) -> Option<String> {
    let key = name.as_bytes();
    let mut at = 0;
    while let Some(offset) = tag.get(at..).and_then(|rest| {
        rest.windows(key.len()).position(|w| w == key)
    }) {
        let after_key = at + offset + key.len();
        let rest = &tag[after_key.min(tag.len())..];
        let rest = rest.strip_prefix(b" =").unwrap_or(rest);
        let rest = rest.strip_prefix(b"=").unwrap_or(rest).trim_ascii_start();
        if let Some(quote) = rest.first().copied().filter(|b| *b == b'"' || *b == b'\'') {
            let body = &rest[1..];
            if let Some(end) = body.iter().position(|b| *b == quote) {
                return std::str::from_utf8(&body[..end]).ok().map(str::to_string);
            }
        }
        at = after_key.max(at + 1);
    }
    None
}

/// Memo de [`load_bsn_world_in`]: as últimas [`CAPACITY`] leituras (o
/// mundo + os autoloads/cenas pequenas que se leem entre as duas leituras
/// dele), cada uma validada pelas suas fontes.
mod memo {
    use super::LoadedWorld;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::SystemTime;

    type Stamp = Option<(SystemTime, u64)>;

    struct Entry {
        path: PathBuf,
        root_dir: PathBuf,
        stamps: Vec<(PathBuf, Stamp)>,
        world: LoadedWorld,
    }

    const CAPACITY: usize = 8;

    static MEMO: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

    fn stamp(path: &Path) -> Stamp {
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    fn disabled() -> bool {
        std::env::var("VIBER_BSN_MEMO").is_ok_and(|v| v == "0")
    }

    pub(super) fn get(path: &Path, root_dir: &Path) -> Option<LoadedWorld> {
        if disabled() {
            return None;
        }
        let guard = MEMO.lock().ok()?;
        let Some(entry) = guard
            .iter()
            .find(|entry| entry.path == path && entry.root_dir == root_dir)
        else {
            bevy::log::debug!("bsn memo: primeira leitura de {}", path.display());
            return None;
        };
        if let Some((source, _)) = entry
            .stamps
            .iter()
            .find(|(source, old)| stamp(source) != *old)
        {
            bevy::log::debug!("bsn memo: {} mudou — a reler", source.display());
            return None;
        }
        bevy::log::debug!("bsn memo: reaproveita {}", path.display());
        Some(entry.world.clone())
    }

    pub(super) fn put(path: &Path, root_dir: &Path, world: &LoadedWorld) {
        if disabled() {
            return;
        }
        let stamps = world
            .sources
            .iter()
            .map(|source| (source.clone(), stamp(source)))
            .collect();
        if let Ok(mut guard) = MEMO.lock() {
            guard.retain(|entry| entry.path != path || entry.root_dir != root_dir);
            if guard.len() >= CAPACITY {
                guard.remove(0);
            }
            guard.push(Entry {
                path: path.to_path_buf(),
                root_dir: root_dir.to_path_buf(),
                stamps,
                world: world.clone(),
            });
        }
    }
}

fn load_bsn_world_uncached(path: &Path, root_dir: &Path) -> Result<LoadedWorld> {
    let path = path.to_path_buf();
    let root_dir = root_dir.to_path_buf();
    let mut loader = Loader {
        root_dir,
        stack: vec![path.clone()],
        unresolved: Default::default(),
        native: Vec::new(),
        sources: vec![path.clone()],
    };
    let (root_tag, root_attrs, nodes) = loader.load_document(&path)?;
    let _ = root_tag;
    Ok(LoadedWorld {
        root_attrs,
        nodes,
        native: loader.native,
        sources: loader.sources,
    })
}

struct Loader {
    root_dir: PathBuf,
    stack: Vec<PathBuf>,
    /// Fontes `IsA` sem ficheiro: esses `<Include>` usam o `src` original.
    unresolved: std::collections::HashSet<PathBuf>,
    native: Vec<NativeEntity>,
    sources: Vec<PathBuf>,
}

/// Uma entidade do `.bsn` sem `WorldNode` que traz componentes próprios.
#[derive(Debug, Clone)]
pub struct NativeEntity {
    /// Identidade da entidade de definições no documento resolvido (Play do Jackdaw).
    pub node_id: Option<u64>,
    pub name: Option<String>,
    /// `(type_path, valor)` de cada componente que não é da camada de
    /// edição (pose, visibilidade, ids do Jackdaw, previews).
    pub components: Vec<(String, BsnValue)>,
}

impl NativeEntity {
    pub fn get(&self, type_path: &str) -> Option<&BsnValue> {
        self.components
            .iter()
            .find(|(path, _)| path == type_path)
            .map(|(_, value)| value)
    }
}

/// Componentes da camada de edição: não são definições do jogo.
fn is_editor_component(type_path: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "bevy_transform::",
        "bevy_camera::visibility::",
        "bevy_ecs::",
        "bevy_light::",
        "jackdaw",
        "bevy_world_xml::",
        "bevy_ext_scene::",
    ];
    PREFIXES.iter().any(|prefix| type_path.starts_with(prefix))
}

/// Root tag/attrs of a `.bsn` document plus its converted children.
type DocumentParts = (String, Vec<(String, String)>, Vec<XmlNode>);

impl Loader {
    fn load_document(&mut self, path: &Path) -> Result<DocumentParts> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let ast = parse_bsn_text(&text).map_err(|err| anyhow!("{}: {err}", path.display()))?;
        let ast = self.resolve_prefabs(ast, path)?;
        if ast.roots.is_empty() {
            bail!("{}: documento .bsn sem entidades", path.display());
        }

        let mut root_tag = "world".to_string();
        let mut root_attrs = Vec::new();
        let mut has_world_root = false;
        let mut nodes = Vec::new();
        for (index, &root) in ast.roots.iter().enumerate() {
            let mut prefix = vec![index as u32];
            if let Some(world) = world_node(&ast, root)
                && (world.tag.eq_ignore_ascii_case("world")
                    || world.tag.eq_ignore_ascii_case("scene"))
            {
                root_tag = world.tag.clone();
                root_attrs = world
                    .attrs
                    .iter()
                    .map(|prop| (prop.key.clone(), prop.value.clone()))
                    .collect();
                has_world_root = true;
                nodes.extend(self.convert_children(&ast, root, path, &mut prefix)?);
                continue;
            }
            nodes.extend(self.convert_node(&ast, root, path, &mut prefix)?);
        }
        if !has_world_root && root_attrs.is_empty() {
            // Sem nó `<world>`: todos os roots são conteúdo do mundo.
            root_tag = "world".to_string();
        }
        Ok((root_tag, root_attrs, nodes))
    }

    /// Materializa as instâncias `IsA` (os `<Include>` do emissor e o que o
    /// editor gravou como overrides) com o mesmo resolver do Jackdaw.
    fn resolve_prefabs(&mut self, mut ast: SceneBsnAst, path: &Path) -> Result<SceneBsnAst> {
        if ast.entities_with_component(ISA).is_empty() {
            return Ok(ast);
        }
        let assets_root = assets_root_of(&self.root_dir);
        let doc_dir = path.parent().unwrap_or(&self.root_dir);
        jackdaw_prefab::absolutize_isa_sources(&mut ast, &assets_root, doc_dir);

        let mut cache: HashMap<PathBuf, SceneBsnAst> = HashMap::new();
        let mut pending = isa_sources(&ast);
        while let Some(source) = pending.pop() {
            if cache.contains_key(&source) {
                continue;
            }
            if !source.is_file() && jackdaw_bsn::existing_form(&source).is_none() {
                // Módulo ainda sem `.bsn`: o `<Include src>` cai no XML.
                self.unresolved.insert(source);
                continue;
            }
            let prefab = jackdaw_prefab::read_prefab_document(&source, &assets_root)
                .with_context(|| format!("{} (em {})", source.display(), path.display()))?;
            pending.extend(isa_sources(&prefab));
            if !self.sources.contains(&source) {
                self.sources.push(source.clone());
            }
            cache.insert(source, prefab);
        }
        jackdaw_prefab::resolve_scene(&ast, &|source: &Path| cache.get(source))
            .map_err(|err| anyhow!("{}: {err}", path.display()))
    }

    fn convert_children(
        &mut self,
        ast: &SceneBsnAst,
        patches_entity: bevy::ecs::entity::Entity,
        current_path: &Path,
        prefix: &mut Vec<u32>,
    ) -> Result<Vec<XmlNode>> {
        let children = ast.get_children_ast(patches_entity);
        let mut out = Vec::with_capacity(children.len());
        for (index, child) in children.into_iter().enumerate() {
            prefix.push(index as u32);
            out.extend(self.convert_node(ast, child, current_path, prefix)?);
            prefix.pop();
        }
        Ok(out)
    }

    fn convert_node(
        &mut self,
        ast: &SceneBsnAst,
        patches_entity: bevy::ecs::entity::Entity,
        current_path: &Path,
        prefix: &mut Vec<u32>,
    ) -> Result<Vec<XmlNode>> {
        let Some(world) = world_node(ast, patches_entity) else {
            return self.convert_folder(ast, patches_entity, current_path, prefix);
        };
        // Identidade fixada (instância de prefab, nó arrumado noutra pasta):
        // usada tal e qual — é a chave dos saves e não pode mudar.
        let pinned = struct_patch(ast, patches_entity, WORLD_ORIGIN).map(|own| NodeOrigin {
            file: Arc::from(self.root_dir.join(field_str(own, "file").unwrap_or_default()).as_path()),
            path: field(own, "path")
                .and_then(as_list)
                .map(|values| values.iter().filter_map(as_u32).collect())
                .unwrap_or_default(),
        });
        let tag = world.tag.clone();
        let mut attrs: Vec<(String, String)> = world
            .attrs
            .iter()
            .map(|prop| (prop.key.clone(), prop.value.clone()))
            .collect();
        let raw_attrs = attrs.clone();
        let origin = pinned.unwrap_or_else(|| self.origin_for(&world, current_path, prefix));
        // Identidade estável do nó no editor (Play do Jackdaw): o jogo põe-a
        // nas entidades deste nó para as edições ao vivo as encontrarem.
        if let Some(id) = ast.stable_id_of(patches_entity) {
            crate::bsn::specs::record_node_id(&origin, id);
        }

        if tag.eq_ignore_ascii_case("include") {
            let instance = struct_patch(ast, patches_entity, ISA)
                .and_then(|data| field_str(data, "source"))
                .map(PathBuf::from);
            if let Some(source) = instance
                && !self.unresolved.contains(&source)
            {
                return self.expand_instance(ast, patches_entity, &attrs, origin, prefix);
            }
            return self.expand_include(ast, &attrs, origin, current_path);
        }

        // Os componentes tipados são a forma canónica dos atributos: aplicam-se
        // antes de derivar o nome. Um `id` migrado para `BiomeRegionSpec`
        // mudava o nome derivado e o loader acrescentava um `name` que o
        // documento não tinha (o `bsn_equiv` acusava a migração).
        crate::bsn::specs::apply_specs(ast, patches_entity, &origin, &mut attrs);
        // O emissor dá sempre um `#Name` (derivado quando o XML não tinha
        // `name`); só um nome diferente do derivado é autoral.
        if let Some(name) = ast.get_name(patches_entity)
            && name != display_name(&tag, &attrs)
        {
            set_attr(&mut attrs, "name", name.to_string());
        }
        apply_transform_and_previews(ast, patches_entity, &tag, world.pose, &mut attrs);
        crate::bsn::specs::record_native(ast, patches_entity, &origin);
        #[cfg(feature = "terrain")]
        crate::bsn::terrain::apply_terrain(ast, patches_entity, &tag, &mut attrs);

        let children = self.convert_children(ast, patches_entity, current_path, prefix)?;

        // O editor anexa as features de terreno como FILHAS da âncora
        // (`VoxelTerrain`), mas o parser trata as features de solo como folhas
        // e ignora os filhos de `<Terrain>`. As features filhas da âncora
        // sobem a irmãs na vista — a posição entre irmãos é indiferente ao
        // `collect_terrain`.
        #[cfg(feature = "terrain")]
        if tag.eq_ignore_ascii_case("terrain") {
            let mut hoisted = Vec::new();
            let mut kept = Vec::new();
            for child in children {
                if matches!(
                    child.tag.to_ascii_lowercase().as_str(),
                    "cave" | "arch" | "bridge" | "cliff"
                ) {
                    hoisted.push(child);
                } else {
                    kept.push(child);
                }
            }
            let mut out = vec![XmlNode {
                tag,
                attrs,
                text: world.text.clone(),
                children: kept,
                origin: Some(Arc::new(origin)),
            }];
            out.extend(hoisted);
            return Ok(out);
        }

        crate::bsn::specs::record_node(
            &origin,
            crate::bsn::specs::RecordedNode {
                tag: tag.clone(),
                raw_attrs,
                attrs: attrs.clone(),
                text: world.text.clone(),
                has_children: !children.is_empty(),
            },
        );
        Ok(vec![XmlNode {
            tag,
            attrs,
            text: world.text.clone(),
            children,
            origin: Some(Arc::new(origin)),
        }])
    }

    /// Entidade sem `WorldNode`: guarda os componentes próprios (definições)
    /// e trata-a como **pasta transparente** — os filhos sobem ao nível dela.
    /// Uma pasta com pose vira um `<Group>` com essa pose.
    fn convert_folder(
        &mut self,
        ast: &SceneBsnAst,
        patches_entity: bevy::ecs::entity::Entity,
        current_path: &Path,
        prefix: &mut Vec<u32>,
    ) -> Result<Vec<XmlNode>> {
        // Feature de terreno (cave/arco/ponte como componente): a entidade
        // vira o nó DSL correspondente em vez de pasta transparente — o
        // componente nunca chega ao caminho `native`.
        #[cfg(feature = "terrain")]
        if crate::bsn::terrain::is_terrain_feature(ast, patches_entity) {
            let children = self.convert_children(ast, patches_entity, current_path, prefix)?;
            let node = crate::bsn::terrain::terrain_feature_node(
                ast,
                patches_entity,
                children,
                current_path,
                prefix,
            )
            .expect("is_terrain_feature implica um componente conhecido");
            return Ok(vec![node]);
        }
        let components: Vec<(String, BsnValue)> = ast
            .component_type_paths(patches_entity)
            .into_iter()
            .filter(|path| !is_editor_component(path))
            .filter_map(|path| {
                let value = jackdaw_bsn::get_bsn_field(ast, patches_entity, &path, "")?;
                Some((path, value))
            })
            .collect();
        if !components.is_empty() {
            self.native.push(NativeEntity {
                node_id: ast.stable_id_of(patches_entity),
                name: ast.get_name(patches_entity).map(str::to_owned),
                components,
            });
        }

        let children = self.convert_children(ast, patches_entity, current_path, prefix)?;
        if children.is_empty() {
            return Ok(children);
        }
        let pose = transform_patch(ast, patches_entity)
            .map(|(translation, rotation, scale)| Pose {
                translation,
                rotation,
                scale,
            })
            .unwrap_or_default();
        if pose == Pose::default() {
            return Ok(children);
        }
        let mut attrs = Vec::new();
        write_pose_attrs(&mut attrs, pose);
        if let Some(name) = ast.get_name(patches_entity) {
            set_attr(&mut attrs, "name", name.to_string());
        }
        Ok(vec![XmlNode {
            tag: "Group".to_string(),
            attrs,
            text: String::new(),
            children,
            origin: Some(Arc::new(NodeOrigin {
                file: Arc::from(current_path),
                path: prefix.to_vec(),
            })),
        }])
    }

    /// `<Include>` já materializado pelo resolver de prefabs: os filhos da
    /// instância são o conteúdo do módulo. Como no XML, só há `<Group>` de
    /// embrulho quando a instância tem pose.
    fn expand_instance(
        &mut self,
        ast: &SceneBsnAst,
        patches_entity: bevy::ecs::entity::Entity,
        attrs: &[(String, String)],
        origin: NodeOrigin,
        prefix: &mut Vec<u32>,
    ) -> Result<Vec<XmlNode>> {
        let module = struct_patch(ast, patches_entity, ISA)
            .and_then(|data| field_str(data, "source"))
            .map(PathBuf::from)
            .unwrap_or_else(|| origin.file.to_path_buf());
        let included = self.convert_children(ast, patches_entity, &module, prefix)?;

        let mut wrapper_attrs: Vec<(String, String)> = attrs
            .iter()
            .filter(|(key, _)| INCLUDE_POSE_KEYS.contains(&key.as_str()))
            .cloned()
            .collect();
        if let Some((translation, rotation, scale)) = transform_patch(ast, patches_entity) {
            wrapper_attrs.retain(|(key, _)| !INCLUDE_POSE_KEYS.contains(&key.as_str()));
            let pose = Pose {
                translation,
                rotation,
                scale,
            };
            if pose != Pose::default() {
                write_pose_attrs(&mut wrapper_attrs, pose);
            }
        }
        if wrapper_attrs.is_empty() {
            return Ok(included);
        }
        let derived = display_name("Include", attrs);
        let name = attr(attrs, "name").map(str::to_owned).or_else(|| {
            ast.get_name(patches_entity)
                .filter(|name| *name != derived)
                .map(str::to_owned)
        });
        if let Some(name) = name {
            set_attr(&mut wrapper_attrs, "name", name);
        }
        Ok(vec![XmlNode {
            tag: "Group".to_string(),
            attrs: wrapper_attrs,
            text: String::new(),
            children: included,
            origin: Some(Arc::new(origin)),
        }])
    }

    fn expand_include(
        &mut self,
        _ast: &SceneBsnAst,
        attrs: &[(String, String)],
        origin: NodeOrigin,
        current_path: &Path,
    ) -> Result<Vec<XmlNode>> {
        let src = attr(attrs, "src")
            .map(str::trim)
            .filter(|src| !src.is_empty())
            .ok_or_else(|| anyhow!("<Include> sem src em {}", origin.file.display()))?;
        let dir = current_path.parent().unwrap_or(&self.root_dir);
        let mut resolved = resolve_include_path(src, dir, &self.root_dir);
        resolved = prefer_bsn(resolved);

        let canonical = resolved
            .canonicalize()
            .with_context(|| format!("include inexistente: {}", resolved.display()))?;
        if self.stack.contains(&canonical) {
            bail!(
                "ciclo de includes: {}",
                self.stack
                    .iter()
                    .map(|path| path.display().to_string())
                    .chain(std::iter::once(canonical.display().to_string()))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            );
        }
        if self.stack.len() > MAX_INCLUDE_DEPTH {
            bail!(
                "include depth exceeds {MAX_INCLUDE_DEPTH}: {}",
                canonical.display()
            );
        }

        self.stack.push(canonical.clone());
        let included = if is_bsn(&canonical) {
            self.load_document(&canonical)?.2
        } else {
            crate::xml::include::load_world_in(&canonical, &self.root_dir)?.nodes
        };
        self.stack.pop();

        // Mesma regra do XML: só há wrapper quando o include tem pose; o
        // `name` acompanha o wrapper.
        let mut wrapper_attrs: Vec<(String, String)> = attrs
            .iter()
            .filter(|(key, _)| INCLUDE_POSE_KEYS.contains(&key.as_str()))
            .cloned()
            .collect();
        if wrapper_attrs.is_empty() {
            return Ok(included);
        }
        if !wrapper_attrs.iter().any(|(key, _)| key == "name")
            && let Some(name) = attr(attrs, "name")
        {
            wrapper_attrs.push(("name".into(), name.to_owned()));
        }
        let wrapper = XmlNode {
            tag: "Group".to_string(),
            attrs: wrapper_attrs,
            text: String::new(),
            children: included,
            origin: Some(Arc::new(origin)),
        };
        Ok(vec![wrapper])
    }

    fn origin_for(&self, world: &WorldNode, current_path: &Path, prefix: &[u32]) -> NodeOrigin {
        if !world.origin_file.is_empty() {
            let path = self.root_dir.join(&world.origin_file);
            let path = path.canonicalize().unwrap_or(path);
            return NodeOrigin {
                file: Arc::from(path.as_path()),
                path: world.origin_path.clone(),
            };
        }
        NodeOrigin {
            file: Arc::from(current_path),
            path: prefix.to_vec(),
        }
    }
}

/// `IsA.source` de cada instância do documento.
fn isa_sources(ast: &SceneBsnAst) -> Vec<PathBuf> {
    ast.entities_with_component(ISA)
        .into_iter()
        .filter_map(|node| jackdaw_prefab::read_isa_source(ast, node))
        .collect()
}

/// Pasta de assets do projeto, como o Jackdaw a vê (`assets_dir` do
/// `jackdaw.toml`; `assets/` por omissão). Só desempata `IsA.source` que o
/// editor gravou relativos a ela.
fn assets_root_of(root_dir: &Path) -> PathBuf {
    let configured = std::fs::read_to_string(root_dir.join("jackdaw.toml"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "assets_dir").then(|| value.trim().trim_matches('"').to_string())
            })
        });
    root_dir.join(configured.as_deref().unwrap_or("assets"))
}

fn prefer_bsn(path: PathBuf) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("bsn") => path,
        Some("xml") => {
            let bsn = path.with_extension("bsn");
            if bsn.is_file() { bsn } else { path }
        }
        _ => {
            let bsn = path.with_extension("bsn");
            if bsn.is_file() {
                bsn
            } else {
                let xml = path.with_extension("xml");
                if xml.is_file() { xml } else { path }
            }
        }
    }
}

fn is_bsn(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bsn"))
}

pub(crate) fn world_node(
    ast: &SceneBsnAst,
    patches_entity: bevy::ecs::entity::Entity,
) -> Option<WorldNode> {
    let data = struct_patch(ast, patches_entity, WORLD_NODE)?;
    let tag = field_str(data, "tag")?.to_string();
    let attrs = match field(data, "attrs") {
        Some(BsnValue::List(values)) => values
            .iter()
            .filter_map(|value| {
                let prop = as_struct(value)?;
                Some(WorldProp {
                    key: field_str(prop, "key")?.to_string(),
                    value: field_str(prop, "value").unwrap_or_default().to_string(),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(WorldNode {
        tag,
        attrs,
        text: field_str(data, "text").unwrap_or_default().to_string(),
        origin_file: field_str(data, "origin_file")
            .unwrap_or_default()
            .to_string(),
        origin_path: field(data, "origin_path")
            .and_then(as_list)
            .map(|values| values.iter().filter_map(as_u32).collect())
            .unwrap_or_default(),
        pose: field(data, "pose").and_then(as_bool).unwrap_or(false),
    })
}

fn apply_transform_and_previews(
    ast: &SceneBsnAst,
    patches_entity: bevy::ecs::entity::Entity,
    tag: &str,
    pose_authoritative: bool,
    attrs: &mut Vec<(String, String)>,
) {
    let transform = transform_patch(ast, patches_entity);
    if tag.eq_ignore_ascii_case("directionallight") {
        if let Some((_, rotation, _)) = transform {
            let direction = rotation_to_direction(rotation);
            set_attr(attrs, "direction", format_vec3(direction));
        }
    } else if pose_authoritative && let Some((translation, rotation, scale)) = transform {
        write_pose_attrs(
            attrs,
            Pose {
                translation,
                rotation,
                scale,
            },
        );
    }

    if let Some(data) = struct_patch(ast, patches_entity, GLTF_SOURCE)
        && let Some(path) = field_str(data, "path")
    {
        let normalized = path.trim_start_matches('/');
        let current = attr(attrs, "url").map(str::to_string);
        let same = current
            .as_deref()
            .map(|url| url.trim_start_matches('/') == normalized)
            .unwrap_or(false);
        if !same {
            let url = if current.as_deref().is_some_and(|url| url.starts_with('/')) {
                format!("/{normalized}")
            } else {
                normalized.to_string()
            };
            set_attr(attrs, "url", url);
        }
    }

    if let Some(data) = struct_patch(ast, patches_entity, POINT_LIGHT) {
        if let Some(color) = field(data, "color").and_then(color_rgb) {
            set_attr(attrs, "color", hex_color(color));
        }
        if let Some(value) = field_f64(data, "intensity") {
            set_attr(attrs, "intensity", format_f32(value as f32));
        }
        if let Some(value) = field_f64(data, "radius") {
            set_attr(attrs, "radius", format_f32(value as f32));
        }
        if let Some(value) = field_bool(data, "shadow_maps_enabled") {
            set_attr(attrs, "shadows", value.to_string());
        }
    }

    if let Some(data) = struct_patch(ast, patches_entity, RECT_LIGHT) {
        if let Some(color) = field(data, "color").and_then(color_rgb) {
            set_attr(attrs, "color", hex_color(color));
        }
        for (field_name, attr_name) in [
            ("intensity", "intensity"),
            ("range", "range"),
            ("width", "width"),
            ("height", "height"),
        ] {
            if let Some(value) = field_f64(data, field_name) {
                set_attr(attrs, attr_name, format_f32(value as f32));
            }
        }
    }

    if let Some(data) = struct_patch(ast, patches_entity, DIRECTIONAL_LIGHT) {
        if let Some(color) = field(data, "color").and_then(color_rgb) {
            set_attr(attrs, "color", hex_color(color));
        }
        if let Some(value) = field_f64(data, "illuminance") {
            set_attr(attrs, "illuminance", format_f32(value as f32));
        }
        if let Some(value) = field_bool(data, "shadow_maps_enabled") {
            set_attr(attrs, "shadows", value.to_string());
        }
    }
}

pub(crate) fn transform_patch(
    ast: &SceneBsnAst,
    patches_entity: bevy::ecs::entity::Entity,
) -> Option<(Vec3, Quat, Vec3)> {
    let data = struct_patch(ast, patches_entity, TRANSFORM)?;
    let translation = field(data, "translation")
        .and_then(as_vec3)
        .unwrap_or(Vec3::ZERO);
    let rotation = field(data, "rotation")
        .and_then(as_quat)
        .unwrap_or(Quat::IDENTITY);
    let scale = field(data, "scale").and_then(as_vec3).unwrap_or(Vec3::ONE);
    Some((translation, rotation, scale))
}

fn struct_patch<'a>(
    ast: &'a SceneBsnAst,
    patches_entity: bevy::ecs::entity::Entity,
    type_path: &str,
) -> Option<&'a BsnStructData> {
    let patch = ast.find_patch_by_type_path(patches_entity, type_path)?;
    match ast.get_patch(patch)? {
        BsnPatch::Struct(data) => Some(data),
        _ => None,
    }
}

fn field<'a>(data: &'a BsnStructData, name: &str) -> Option<&'a BsnValue> {
    data.fields
        .0
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
}

fn field_str<'a>(data: &'a BsnStructData, name: &str) -> Option<&'a str> {
    field(data, name).and_then(as_str)
}

fn field_f64(data: &BsnStructData, name: &str) -> Option<f64> {
    field(data, name).and_then(as_f64)
}

fn field_bool(data: &BsnStructData, name: &str) -> Option<bool> {
    field(data, name).and_then(as_bool)
}

fn as_struct(value: &BsnValue) -> Option<&BsnStructData> {
    match value {
        BsnValue::Struct(data) => Some(data),
        _ => None,
    }
}

fn as_str(value: &BsnValue) -> Option<&str> {
    match value {
        BsnValue::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn as_f64(value: &BsnValue) -> Option<f64> {
    match value {
        BsnValue::Float(value) => Some(*value),
        BsnValue::Int(value) => Some(*value as f64),
        _ => None,
    }
}

fn as_bool(value: &BsnValue) -> Option<bool> {
    match value {
        BsnValue::Bool(value) => Some(*value),
        _ => None,
    }
}

fn as_u32(value: &BsnValue) -> Option<u32> {
    match value {
        BsnValue::Int(value) => u32::try_from(*value).ok(),
        _ => None,
    }
}

fn as_list(value: &BsnValue) -> Option<&Vec<BsnValue>> {
    match value {
        BsnValue::List(values) => Some(values),
        _ => None,
    }
}

fn as_vec3(value: &BsnValue) -> Option<Vec3> {
    let data = as_struct(value)?;
    Some(Vec3::new(
        field_f64(data, "x")? as f32,
        field_f64(data, "y")? as f32,
        field_f64(data, "z")? as f32,
    ))
}

fn as_quat(value: &BsnValue) -> Option<Quat> {
    let data = as_struct(value)?;
    Some(Quat::from_xyzw(
        field_f64(data, "x")? as f32,
        field_f64(data, "y")? as f32,
        field_f64(data, "z")? as f32,
        field_f64(data, "w")? as f32,
    ))
}

fn color_rgb(value: &BsnValue) -> Option<[f32; 3]> {
    match value {
        BsnValue::Struct(data)
            if data.type_path.ends_with("Srgba") || data.type_path.ends_with("Srgb") =>
        {
            Some([
                field_f64(data, "red")? as f32,
                field_f64(data, "green")? as f32,
                field_f64(data, "blue")? as f32,
            ])
        }
        // `Color::Srgba(Srgba { .. })`, a forma que o Bevy reflete.
        BsnValue::TupleStruct(data) if data.values.len() == 1 => color_rgb(&data.values[0]),
        BsnValue::Type(path) if path.ends_with("WHITE") => Some([1.0, 1.0, 1.0]),
        BsnValue::Type(path) if path.ends_with("BLACK") => Some([0.0, 0.0, 0.0]),
        _ => None,
    }
}

fn hex_color([red, green, blue]: [f32; 3]) -> String {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(red),
        channel(green),
        channel(blue)
    )
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

pub(crate) fn set_attr(attrs: &mut Vec<(String, String)>, key: &str, value: String) {
    if let Some((_, current)) = attrs.iter_mut().find(|(name, _)| name == key) {
        *current = value;
    } else {
        attrs.push((key.to_string(), value));
    }
}

#[cfg(test)]
mod tag_scan_tests {
    use super::*;

    /// O `.bsn` é RON: o tag é `tag: "X"` e as referências são strings
    /// terminadas em `.bsn`. O teste cobre o documento, um include e o
    /// `IsA { source }` de um prefab.
    #[test]
    fn finds_tags_in_the_document_and_through_references() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let root = dir.path();
        std::fs::write(
            root.join("main.bsn"),
            r#"WorldNode { tag: "world", attrs: [] }
Children [
    IsA { source: "world/cities/a.bsn", deleted: [] }
]
WorldNode { tag: "Include", attrs: [WorldProp { key: "src", value: "world/b.bsn" }] }"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("world/cities")).unwrap();
        std::fs::write(
            root.join("world/cities/a.bsn"),
            r#"WorldNode { tag: "Water", attrs: [] }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("world/b.bsn"),
            r#"WorldNode { tag: "DayCycle", attrs: [] }"#,
        )
        .unwrap();

        assert!(
            authors_tags(&root.join("main.bsn"), root, &["Water"]),
            "o tag vem do prefab referenciado por IsA"
        );
        assert!(
            authors_tags(&root.join("main.bsn"), root, &["DayCycle"]),
            "o tag vem do include"
        );
        assert!(
            !authors_tags(&root.join("main.bsn"), root, &["Sky"]),
            "nenhum documento autora Sky"
        );
    }

    #[test]
    fn a_name_alone_does_not_count() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let root = dir.path();
        // `SkyModel` é um valor de atributo, não um nó `tag: "Sky"`.
        std::fs::write(
            root.join("w.bsn"),
            r#"WorldNode { tag: "world", attrs: [WorldProp { key: "model", value: "SkyModel" }] }"#,
        )
        .unwrap();
        assert!(!authors_tags(&root.join("w.bsn"), root, &["Sky"]));
    }

    #[test]
    fn cycles_and_missing_files_do_not_hang() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let root = dir.path();
        std::fs::write(root.join("a.bsn"), r#"IsA { source: "b.bsn" }"#).unwrap();
        std::fs::write(root.join("b.bsn"), r#"IsA { source: "a.bsn" }"#).unwrap();
        assert!(!authors_tags(&root.join("a.bsn"), root, &["Water"]));
        assert!(!authors_tags(&root.join("missing.bsn"), root, &["Water"]));
    }

    /// O `.xml` legado continua legível: `<Include src>` é seguido e o `src`
    /// sai com os dois estilos de aspas.
    #[test]
    fn legacy_xml_includes_are_followed() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let root = dir.path();
        std::fs::write(
            root.join("old.xml"),
            r#"<world><Include  src = 'nested.xml' /></world>"#,
        )
        .unwrap();
        std::fs::write(root.join("nested.xml"), r#"<world><Water level="3"/></world>"#).unwrap();
        // O `.xml` não tem `tag: "Water"`; o tag XML só é visto no include… e o
        // scanner procura RON. O que se garante aqui é que o `src` é lido.
        let tag = br#"<Include src="cities/a.bsn"/>"#;
        assert_eq!(attribute_text(tag, "src").as_deref(), Some("cities/a.bsn"));
        assert_eq!(attribute_text(br#"<Include  src = 'x/y.xml' />"#, "src").as_deref(), Some("x/y.xml"));
        assert_eq!(attribute_text(br#"<Include/>"#, "src"), None);
        assert!(
            authors_tags(&root.join("old.xml"), root, &["Water"]),
            "o mundo XML legado autoram `<Water>` no include"
        );
        assert!(!authors_tags(&root.join("old.xml"), root, &["Sky"]));
    }
}

#[cfg(test)]
mod world_scan_tests {
    use super::*;

    /// Guarda do contrato: se alguém voltar a autorar `<Sky>`/`<DayCycle>`/
    /// `<Water>`/`<Layer>` no mundo, este teste falha e o parse da cena
    /// principal (1,4 s) tem de voltar a correr. Corre com
    /// `VAEL_ROOT=<checkout do Vael>`.
    #[test]
    fn the_vael_world_authors_no_shader_tags() {
        let Some(root) = std::env::var_os("VAEL_ROOT").map(std::path::PathBuf::from) else {
            eprintln!("VAEL_ROOT ausente — a saltar");
            return;
        };
        let world = root.join("world.bsn");
        if !world.is_file() {
            eprintln!("{} ausente — a saltar", world.display());
            return;
        }
        let tags = ["Sky", "DayCycle", "Water", "Layer"];
        assert!(
            !authors_tags(&world, &root, &tags),
            "o mundo voltou a autorar um tag de shader: o parse condicional da \
             cena principal deixa de ser seguro (ver src/app.rs)"
        );
        // Sentinela: o scanner VÊ tags num documento real (senão o teste acima
        // passaria por o scanner estar cego).
        assert!(
            authors_tags(&root.join("scenes/menu.bsn"), &root, &["SceneEntry"]),
            "o scanner não encontra um tag que existe mesmo — está cego"
        );
    }
}
