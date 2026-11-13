//! Definições autoradas na cena: componentes `Reflect` em entidades `.bsn`
//! sem `WorldNode` (ver [`NativeEntity`]).
//!
//! O Jackdaw edita-as no inspector como qualquer componente; o jogo lê-as
//! daqui como valores tipados. [`SceneSettingsPlugin`] guarda-as num recurso
//! e relê os documentos quando mudam em disco, para uma gravação no editor
//! chegar ao jogo que está a correr.
//!
//! ```ignore
//! app.add_plugins(SceneSettingsPlugin::new("world.bsn"))
//!     .add_scene_setting::<ElementsSettings>();
//! // `Res<SceneSetting<ElementsSettings>>` — o valor da cena ou o `Default`.
//! ```

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use bevy::prelude::*;
use bevy::reflect::{FromReflect, GetTypeRegistration, TypePath, TypeRegistry};

use crate::bsn::NativeEntity;

/// O valor de uma definição `T` no documento, se existir.
///
/// A conversão usa o próprio `jackdaw_bsn` (mesma leitura do editor): os
/// campos que o documento não escreve ficam com o `Default` de `T`.
pub fn native_value<T>(natives: &[NativeEntity]) -> Option<T>
where
    T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath,
{
    native_values::<T>(natives).into_iter().next()
}

/// Todas as instâncias de `T` no documento (ex.: uma por espécie).
pub fn native_values<T>(natives: &[NativeEntity]) -> Vec<T>
where
    T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath,
{
    let mut registry = TypeRegistry::default();
    registry.register::<T>();
    natives
        .iter()
        .filter_map(|entity| entity.get(T::type_path()))
        .filter_map(|value| {
            let partial =
                jackdaw_bsn::bsn_value_to_reflect(value, TypeId::of::<T>(), &registry, None);
            let Some(partial) = partial else {
                warn!(
                    "definição `{}` ilegível no .bsn — fica o default",
                    T::type_path()
                );
                return None;
            };
            let mut out = T::default();
            if let Err(err) = out.try_apply(partial.as_ref()) {
                warn!("definição `{}`: {err} — fica o default", T::type_path());
                return None;
            }
            Some(out)
        })
        .collect()
}

/// Lê as definições de um mundo `.bsn` (com os módulos/prefabs que inclui).
pub fn read_natives(world: &Path) -> anyhow::Result<(Vec<NativeEntity>, Vec<PathBuf>)> {
    let loaded = crate::bsn::load_bsn_world(world)?;
    Ok((loaded.native, loaded.sources))
}

/// Definições lidas do mundo e os ficheiros de onde vieram.
#[derive(Resource, Debug, Default)]
pub struct SceneNatives {
    pub world: PathBuf,
    pub natives: Vec<NativeEntity>,
    sources: Vec<(PathBuf, Option<SystemTime>)>,
    /// Sobe a cada releitura; as definições tipadas comparam com isto.
    pub generation: u64,
    poll: Timer,
}

impl SceneNatives {
    fn load(world: PathBuf) -> Self {
        let mut out = Self {
            world,
            poll: Timer::new(Duration::from_secs(1), TimerMode::Repeating),
            ..default()
        };
        out.reload();
        out
    }

    fn reload(&mut self) {
        match read_natives(&self.world) {
            Ok((natives, sources)) => {
                self.natives = natives;
                self.sources = sources
                    .into_iter()
                    .map(|path| {
                        let modified = mtime(&path);
                        (path, modified)
                    })
                    .collect();
                self.generation += 1;
            }
            Err(err) => warn!("definições da cena: {err:#}"),
        }
    }

    fn changed_on_disk(&self) -> bool {
        self.sources
            .iter()
            .any(|(path, modified)| mtime(path) != *modified)
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Lê as definições do mundo no arranque e relê-as quando um dos documentos
/// muda (verificação de 1 s, barata: só `stat`).
pub struct SceneSettingsPlugin {
    pub world: PathBuf,
    /// Recarregar ao gravar. Desligado, só lê no arranque.
    pub watch: bool,
}

impl SceneSettingsPlugin {
    pub fn new(world: impl Into<PathBuf>) -> Self {
        Self {
            world: world.into(),
            watch: true,
        }
    }
}

impl Plugin for SceneSettingsPlugin {
    fn build(&self, app: &mut App) {
        if app.world().contains_resource::<SceneNatives>() {
            return;
        }
        let world = std::path::absolute(&self.world).unwrap_or_else(|_| self.world.clone());
        let natives = if world
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bsn"))
        {
            SceneNatives::load(world)
        } else {
            // Um mundo XML não tem definições nativas: ficam os defaults.
            SceneNatives { world, ..default() }
        };
        app.insert_resource(natives);
        if self.watch {
            app.add_systems(First, poll_scene_natives);
        }
    }
}

fn poll_scene_natives(time: Res<Time>, mut natives: ResMut<SceneNatives>) {
    if natives.sources.is_empty() || !natives.poll.tick(time.delta()).just_finished() {
        return;
    }
    if natives.bypass_change_detection().changed_on_disk() {
        info!("definições da cena: documento gravado — a reler");
        natives.reload();
    }
}

/// Valor tipado de uma definição da cena (o da cena ou o `Default`).
#[derive(Resource, Debug, Clone, Default, Deref)]
pub struct SceneSetting<T>(pub T);

/// Lista tipada (várias entidades com `T`, ex.: espécies de flora).
#[derive(Resource, Debug, Clone, Default, Deref)]
pub struct SceneSettingList<T>(pub Vec<T>);

/// Regista uma definição tipada: recurso no arranque e atualização quando o
/// documento muda.
pub trait SceneSettingsApp {
    fn add_scene_setting<T>(&mut self) -> &mut Self
    where
        T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath + Clone;

    fn add_scene_setting_list<T>(&mut self) -> &mut Self
    where
        T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath + Clone;
}

impl SceneSettingsApp for App {
    fn add_scene_setting<T>(&mut self) -> &mut Self
    where
        T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath + Clone,
    {
        let value = self
            .world()
            .get_resource::<SceneNatives>()
            .and_then(|natives| native_value::<T>(&natives.natives))
            .unwrap_or_default();
        self.insert_resource(SceneSetting(value))
            .add_systems(First, refresh_setting::<T>.after(poll_scene_natives))
    }

    fn add_scene_setting_list<T>(&mut self) -> &mut Self
    where
        T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath + Clone,
    {
        let values = self
            .world()
            .get_resource::<SceneNatives>()
            .map(|natives| native_values::<T>(&natives.natives))
            .unwrap_or_default();
        self.insert_resource(SceneSettingList(values))
            .add_systems(First, refresh_setting_list::<T>.after(poll_scene_natives))
    }
}

fn refresh_setting<T>(
    natives: Option<Res<SceneNatives>>,
    mut setting: ResMut<SceneSetting<T>>,
    mut seen: Local<Option<u64>>,
) where
    T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath + Clone,
{
    let Some(natives) = natives else {
        return;
    };
    let first = seen.is_none();
    if *seen == Some(natives.generation) {
        return;
    }
    *seen = Some(natives.generation);
    if first {
        // O valor do arranque já veio do `add_scene_setting`.
        return;
    }
    let next = native_value::<T>(&natives.natives).unwrap_or_default();
    // Só marca mudança quando o valor muda (outra definição pode ter sido a
    // gravada): quem reage a `is_changed` não refaz trabalho à toa.
    if setting.0.reflect_partial_eq(&next) != Some(true) {
        setting.0 = next;
    }
}

fn refresh_setting_list<T>(
    natives: Option<Res<SceneNatives>>,
    mut setting: ResMut<SceneSettingList<T>>,
    mut seen: Local<Option<u64>>,
) where
    T: Reflect + FromReflect + Default + GetTypeRegistration + TypePath + Clone,
{
    let Some(natives) = natives else {
        return;
    };
    let first = seen.is_none();
    if *seen == Some(natives.generation) {
        return;
    }
    *seen = Some(natives.generation);
    if first {
        return;
    }
    let next = native_values::<T>(&natives.natives);
    let same = setting.0.len() == next.len()
        && setting
            .0
            .iter()
            .zip(&next)
            .all(|(a, b)| a.reflect_partial_eq(b) == Some(true));
    if !same {
        setting.0 = next;
    }
}

/// Texto `.bsn` de uma entidade de definições: `#"nome"` e um patch por
/// componente (o mesmo emissor que o Jackdaw usa ao gravar). Os campos
/// iguais ao `Default` do tipo saem omitidos.
pub fn emit_native_entity(
    name: &str,
    components: &[&dyn PartialReflect],
    registry: &TypeRegistry,
) -> String {
    let mut ast = jackdaw_bsn::SceneBsnAst::default();
    let mut patches = vec![jackdaw_bsn::BsnPatch::Name(name.to_string())];
    patches.extend(
        components
            .iter()
            .map(|component| jackdaw_bsn::component_to_bsn_patch(*component, registry)),
    );
    let entity = ast.create_entity_node(patches);
    ast.add_to_roots(entity);
    jackdaw_bsn::emit_scene(&ast)
}

/// Várias entidades de definições como filhas de uma raiz `#"raiz"`.
pub fn emit_native_group(
    root: &str,
    root_components: &[&dyn PartialReflect],
    children: &[(String, Vec<&dyn PartialReflect>)],
    registry: &TypeRegistry,
) -> String {
    let mut ast = jackdaw_bsn::SceneBsnAst::default();
    let mut patches = vec![jackdaw_bsn::BsnPatch::Name(root.to_string())];
    patches.extend(
        root_components
            .iter()
            .map(|component| jackdaw_bsn::component_to_bsn_patch(*component, registry)),
    );
    let parent = ast.create_entity_node(patches);
    ast.add_to_roots(parent);
    for (name, components) in children {
        let mut patches = vec![jackdaw_bsn::BsnPatch::Name(name.clone())];
        patches.extend(
            components
                .iter()
                .map(|component| jackdaw_bsn::component_to_bsn_patch(*component, registry)),
        );
        let child = ast.create_entity_node(patches);
        ast.add_child_to_ast(parent, child);
    }
    jackdaw_bsn::emit_scene(&ast)
}
