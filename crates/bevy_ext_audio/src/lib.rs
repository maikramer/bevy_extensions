//! Runtime audio for Bevy games — the Viber audio stack extracted as a
//! package (`src/ambient.rs` + `src/music.rs` do monólito).
//!
//! Módulos (os nomes mantêm-se para o host reusar os caminhos históricos):
//!
//! - [`sfx`] — handles/preload dos clips ([`SfxClip`]), playback de
//!   [`SfxEvent`]/[`SfxLoudEvent`] com atenuação por distância e jitter de
//!   pitch (a fatia de áudio do `AmbientPlugin`).
//! - [`ambience`] — loops de água/cachoeira/chuva com fade quadrático por
//!   distância, todos no bus de ambiente.
//! - [`music`] — buses tipados kira, `mixer_sync`, ducking por bus, reverb,
//!   layers de BGM com crossfade por zona e música de combate (o antigo
//!   `music.rs` da engine).
//! - [`names`] — registry nome→clip partilhado com a camada de scripts.
//! - [`host`] — os contratos que o host publica ([`AudioPaths`],
//!   [`AudioWorldState`], [`HostWaterBodies`], [`AudioEar`]).
//!
//! Os tipos partilhados ([`SfxClip`], [`SfxEvent`], [`SfxLoudEvent`],
//! [`EngineRainVisuals`], [`AMBIENT_LOOP_FILES`], [`AudioMixerSettings`])
//! vivem no `bevy_ext_core` e são só reexportados aqui — contrato único, sem
//! duplicação.
//!
//! # Plugins
//!
//! [`AudioPlugin`] regista tudo; [`SfxPlugin`] e [`MusicPlugin`] são as
//! metades (a de SFX é a fatia de áudio do `AmbientPlugin` do Viber; a de
//! música é o que o `app.rs` registava). **Nunca adicione [`AudioPlugin`] e
//! um sub-plugin ao mesmo tempo** — o Bevy não deduplica `add_systems` e o
//! sistema correria 2× por frame.
//!
//! # Wiring do host (backend kira)
//!
//! O plugin não traz o backend: quem o adiciona configura os canais kira com
//! os buses tipados e cria os reverbs dos tracks de SFX/voz (igual ao
//! `app.rs` do Viber):
//!
//! ```ignore
//! app.add_plugins(bevy_kira_audio::AudioPlugin);
//! app.add_audio_channel::<bevy_ext_audio::MusicBus>();
//! let mut sfx_track = bevy_kira_audio::AudioTrack::new();
//! let sfx_reverb = sfx_track.add_effect(
//!     bevy_kira_audio::prelude::ReverbBuilder::new()
//!         .mix(bevy_kira_audio::prelude::Mix(0.0)),
//! );
//! app.add_audio_channel_with_track::<bevy_ext_audio::SfxBus>(sfx_track);
//! let mut voice_track = bevy_kira_audio::AudioTrack::new();
//! let voice_reverb = voice_track.add_effect(
//!     bevy_kira_audio::prelude::ReverbBuilder::new()
//!         .mix(bevy_kira_audio::prelude::Mix(0.0)),
//! );
//! app.add_audio_channel_with_track::<bevy_ext_audio::VoiceBus>(voice_track);
//! app.add_audio_channel::<bevy_ext_audio::AmbienceBus>();
//! app.insert_resource(bevy_ext_audio::ReverbHandles {
//!     sfx: Some(sfx_reverb),
//!     voice: Some(voice_reverb),
//! });
//! ```
//!
//! Sem canais kira (apps de teste headless) os sistemas são no-ops
//! silenciosos — nenhum pânico.

pub mod ambience;
pub mod audio_space;
pub mod host;
pub mod music;
pub mod names;
pub mod sfx;

use bevy::prelude::*;

// ── tipos partilhados (reexportados do core — não duplicar) ─────────────

pub use ambience::{
    RainAmbienceLoop, WaterAmbienceLoop, WaterfallLoop, rain_ambience_driver, setup_rain_ambience,
    setup_water_ambience, setup_waterfall_ambience, water_ambience_driver,
    waterfall_ambience_driver,
};
pub use bevy_ext_core::{
    AMBIENT_LOOP_FILES, AudioMixerSettings, EngineRainVisuals, Player, SFX_CLIPS_ALL, SfxClip,
    SfxEvent, SfxLoudEvent,
};
pub use host::{
    AudioEar, AudioPaths, AudioWorldState, HostWaterBodies, MAIN_SCENE, WaterBodies, WaterKind,
    WaterfallSpot, ear_position,
};
pub use music::{
    AmbienceLoop, AudioBus, AudioDuck, AudioLoopPending, AudioReverb, COMBAT_MUSIC_HOLD,
    CombatMusicState, DuckLevel, LoopInstance, LoopVolume, MusicBus, MusicLayerOverride,
    MusicLayerTag, ReverbHandles, ReverbPreset, ScriptLoopParams, ScriptLoopPlayback,
    ScriptSoundLoops, SfxBus, VoiceBus, apply_reverb, audio_loop_starter, auto_zone, bgm_zone,
    fade_step, layer_target, linear_to_db, mixer_sync, music_driver, scene_zone,
    script_sound_loops,
};
pub use names::{SFX_NAME_REGISTRY, sfx_clip_from_str};
pub use sfx::{
    SfxHandles, load_sfx_assets, pitch_jitter, sfx_loud_player_system, sfx_player_system,
};

// ── plugins ──────────────────────────────────────────────────────────────

/// Plugin completo de áudio ([`SfxPlugin`] + [`MusicPlugin`]). O análogo do
/// par `AmbientPlugin` (fatia de áudio) / registo de música do `app.rs` do
/// Viber, unificado num só nome.
pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((SfxPlugin, MusicPlugin));
    }
}

/// Fatia de SFX + loops de ambiente — o que o `AmbientPlugin` do Viber
/// registava de áudio (o resto do `AmbientPlugin` — névoa, orçamento de
/// luzes, lanterna, emissor de chuva — é visual e fica na engine).
pub struct SfxPlugin;

impl Plugin for SfxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioPaths>()
            .init_resource::<HostWaterBodies>()
            .add_message::<SfxEvent>()
            .add_message::<SfxLoudEvent>()
            .add_systems(PostStartup, load_sfx_assets)
            .add_systems(
                Update,
                (
                    sfx_player_system,
                    sfx_loud_player_system,
                    setup_water_ambience,
                    water_ambience_driver,
                    setup_waterfall_ambience,
                    waterfall_ambience_driver,
                    setup_rain_ambience,
                    rain_ambience_driver,
                ),
            );
    }
}

/// Música, buses, ducking, reverb e loops de script — os sistemas que o
/// `app.rs` do Viber registava com `timed(Group::World, …)`.
pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioDuck>()
            .init_resource::<AudioReverb>()
            .init_resource::<ReverbHandles>()
            .init_resource::<MusicLayerOverride>()
            .init_resource::<CombatMusicState>()
            .init_resource::<ScriptSoundLoops>()
            .add_systems(
                Update,
                (
                    audio_loop_starter,
                    mixer_sync,
                    apply_reverb,
                    music_driver,
                    script_sound_loops,
                ),
            );
    }
}
