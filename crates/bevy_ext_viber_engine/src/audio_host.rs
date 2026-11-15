//! Adaptador de HOST do `bevy_ext_audio` — o que no crate entra por recursos
//! ([`AudioPaths`], [`AudioWorldState`], [`AudioEar`]) e aqui sai dos
//! recursos da engine (`config::GameConfig`, `scene_loader::CurrentScene`,
//! `ambient::CurrentBiome`, `worldsys::WeatherState`, `editor_camera::EditorCamera`).
//!
//! O `music.rs` do monólito saiu para o crate (exit4/G1) e os sistemas
//! (`music_driver`, `script_sound_loops`) passaram a ler os contratos do
//! host — este plugin publica o espelho por frame. Sem ele os sistemas são
//! no-ops (sem paths de clips, sem zona de cena/bioma) — o jogo compila e
//! corre, mas em silêncio.
//!
//! O análogo no Vael: `app.add_plugins(viber::audio_host::AudioHostPlugin);`
//! junto do registo dos sistemas `viber::music::*`.

use bevy::prelude::*;

use bevy_ext_audio::{AudioEar, AudioPaths, AudioWorldState};

/// Publica os contratos de host do [`bevy_ext_audio`] a partir da engine.
///
/// - `AudioPaths` semeado do `GameConfig.assets.sfx_dir` (Startup — o config
///   é insertado no build da App e nunca muda a correr);
/// - [`publish_audio_world_state`] espelha `WeatherState.rain`,
///   `CurrentScene.id` e `CurrentBiome.id` por frame (Update);
/// - `AudioEar` inserido na câmara de editor (o `ear_position` do crate
///   prefere-a à primeira `Camera3d`, replicando o `active_camera_eye`
///   histórico).
pub struct AudioHostPlugin;

impl Plugin for AudioHostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioPaths>()
            .init_resource::<AudioWorldState>()
            .add_systems(Startup, sync_audio_paths)
            .add_systems(
                Update,
                (publish_audio_world_state, mark_audio_ear),
            );
    }
}

/// Semeia o [`AudioPaths`] do crate com o `assets.sfx_dir` do config. A
/// conversão é a mesma do `GameConfig::asset_string` (trim da barra final;
/// vazio = sem prefixo) — os paths de clips têm de ficar IDÊNTICOS aos que
/// o `resolve_loop_clip` histórico produzia.
pub fn sync_audio_paths(
    config: Option<Res<crate::config::GameConfig>>,
    mut paths: ResMut<AudioPaths>,
) {
    let Some(config) = config else {
        return;
    };
    let dir = config.assets.sfx_dir.to_string_lossy();
    paths.sfx_dir = dir.trim_end_matches('/').to_string();
}

/// Espelho por frame: o que era `Option<Res<CurrentScene>>`/`CurrentBiome`/
/// `WeatherState` nos sistemas da engine entra no [`AudioWorldState`] do
/// crate. `None` do recurso OU `id: None` viram `None` no espelho — a
/// ausência significa o mesmo ("fora de projeto" / vale central).
pub fn publish_audio_world_state(
    weather: Option<Res<crate::worldsys::WeatherState>>,
    scene: Option<Res<crate::scene_loader::CurrentScene>>,
    biome: Option<Res<crate::ambient::CurrentBiome>>,
    mut mirror: ResMut<AudioWorldState>,
) {
    mirror.rain = weather.map(|w| w.rain).unwrap_or(0.0);
    mirror.scene_id = scene.as_ref().and_then(|s| s.id.clone());
    mirror.biome_id = biome.as_ref().and_then(|b| b.id.clone());
}

/// A câmara de editor é o OUVIDO preferido do crate ([`AudioEar`]) — sem o
/// marker, `ear_position` devolve a primeira `Camera3d` (arbitrária com duas
/// câmaras no `--camera free`) e os loops de script atenuavam à câmara errada.
pub fn mark_audio_ear(
    mut commands: Commands,
    editors: Query<Entity, Added<crate::editor_camera::EditorCamera>>,
) {
    for entity in &editors {
        commands.entity(entity).insert(AudioEar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless_app() -> App {
        let mut app = App::new();
        // Só o adaptador: os plugins do crate (SfxPlugin/MusicPlugin) pedem
        // `Res<AssetServer>` (AssetPlugin) — o contrato deles é testado lá.
        app.add_plugins(MinimalPlugins).add_plugins(AudioHostPlugin);
        app
    }

    /// Sem config, o sfx_dir fica no default histórico do crate (os sistemas
    /// são no-ops, nenhum pânico — apps de teste headless).
    #[test]
    fn audio_host_without_config_keeps_default_paths() {
        let mut app = headless_app();
        app.update();
        let paths = app.world().resource::<AudioPaths>();
        assert_eq!(paths.sfx_path("combat/swing.ogg"), "assets/audio/sfx/combat/swing.ogg");
    }

    /// O espelho do mundo segue os recursos da engine: cena/bioma/chuva
    /// publicados, e `None` dos recursos → `None` no espelho.
    #[test]
    fn audio_world_state_mirrors_engine_resources() {
        let mut app = headless_app();
        app.insert_resource(crate::scene_loader::CurrentScene {
            root: std::path::PathBuf::from("/w"),
            id: Some("interiors/chapel".into()),
            path: std::path::PathBuf::from("/w/chapel.bsn"),
            title: None,
            entry: None,
            came_from: None,
            return_pose: None,
        });
        app.insert_resource(crate::ambient::CurrentBiome {
            id: Some("picos".into()),
        });
        app.insert_resource(crate::worldsys::WeatherState {
            wind: [0.0, 0.0],
            wind_strength: 0.0,
            clouds: 0.0,
            rain: 0.4,
            cycle: false,
        });
        app.update();
        let mirror = app.world().resource::<AudioWorldState>();
        assert_eq!(mirror.scene_id.as_deref(), Some("interiors/chapel"));
        assert_eq!(mirror.biome_id.as_deref(), Some("picos"));
        assert!((mirror.rain - 0.4).abs() < 1e-6);

        // Mundo fora de projeto: os recursos saem, o espelho limpa-se.
        app.world_mut().remove_resource::<crate::scene_loader::CurrentScene>();
        app.world_mut().remove_resource::<crate::ambient::CurrentBiome>();
        app.world_mut().remove_resource::<crate::worldsys::WeatherState>();
        app.update();
        let mirror = app.world().resource::<AudioWorldState>();
        assert_eq!(mirror.scene_id, None);
        assert_eq!(mirror.biome_id, None);
        assert_eq!(mirror.rain, 0.0);
    }

    /// O `AudioEar` chega à câmara de editor (o ouvido preferido do crate).
    #[test]
    fn editor_camera_becomes_audio_ear() {
        let mut app = headless_app();
        let editor = app
            .world_mut()
            .spawn(crate::editor_camera::EditorCamera)
            .id();
        app.update();
        assert!(app.world().entity(editor).contains::<AudioEar>());
    }
}
