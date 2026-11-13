//! SFX one-shots: handles/preload dos clips, playback de [`SfxEvent`]/[
//! `SfxLoudEvent`] com volume por distância (hit/whoosh/harvest/ui) —
//! `assets/audio/sfx/`.
//!
//! É a fatia de áudio do `src/ambient.rs` do monólito (o resto do ficheiro —
//! névoa, orçamento de luzes, lanterna, emissor de chuva — é visual e ficou
//! na engine).

use std::collections::HashMap;

use bevy::camera::Camera3d;
use bevy::prelude::*;
use bevy_ext_core::{AudioMixerSettings, SFX_CLIPS_ALL, SfxClip, SfxEvent, SfxLoudEvent};
use bevy_kira_audio::AudioChannel;
use bevy_kira_audio::AudioControl;

use crate::host::{AudioEar, AudioPaths, ear_position};
use crate::music::{SfxBus, linear_to_db};

// ── SFX ─────────────────────────────────────────────────────────────────

/// Handles pré-carregados dos clips — um map `SfxClip → handle` construído
/// de [`SFX_CLIPS_ALL`]; um clip novo no enum entra no preload sozinho. O
/// asset type é o [`bevy_kira_audio::AudioSource`] (decode symphonia), NÃO
/// o `bevy_audio::AudioSource` do backend nativo.
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::ambient"]
pub struct SfxHandles {
    clips: HashMap<SfxClip, Handle<bevy_kira_audio::AudioSource>>,
}

impl SfxHandles {
    /// Pré-carrega TODOS os clips (PostStartup) — o `.ogg` falhado só
    /// produz warn de load e o play é no-op silencioso. Os paths vêm do
    /// `sfx_dir` do config.yaml do jogo ([`AudioPaths`]).
    pub fn load(server: &AssetServer, paths: &AudioPaths) -> Self {
        Self {
            clips: SFX_CLIPS_ALL
                .iter()
                .map(|clip| (*clip, server.load(paths.sfx_path(clip.file()))))
                .collect(),
        }
    }

    pub fn get(&self, clip: SfxClip) -> Option<Handle<bevy_kira_audio::AudioSource>> {
        self.clips.get(&clip).cloned()
    }
}

pub fn load_sfx_assets(mut commands: Commands, server: Res<AssetServer>, paths: Res<AudioPaths>) {
    commands.insert_resource(SfxHandles::load(&server, &paths));
}

/// Toca o clip com volume por distância (Ativos de áudio são globais; a
/// atenuação é calculada no momento do evento). Os buses `sfx` e `master`
/// do `AudioMixerSettings` multiplicam o resultado — sem eles os sliders eram
/// no-op audível para os efeitos.
/// Clips tocados em rajada (cada golpe/step) levam um jitter de pitch
/// ±8 % — mata a monotonia da repetição sem samples extra. LCG local
/// (numerical recipes): barato, sem depender de crate de RNG.
pub fn pitch_jitter(clip: SfxClip, rng: &mut u32) -> f32 {
    const JITTERED: &[SfxClip] = &[
        SfxClip::Hit,
        SfxClip::Whoosh,
        SfxClip::EnemyHurt,
        SfxClip::Footstep,
        SfxClip::FootstepWater,
        SfxClip::Coin,
    ];
    if !JITTERED.contains(&clip) {
        return 1.0;
    }
    *rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    0.92 + ((*rng >> 16) as f32 / u16::MAX as f32) * 0.16
}

/// Reproduz um clip no bus de SFX com um ganho linear já decidido.
fn play_sfx(
    sfx: &AudioChannel<SfxBus>,
    handles: &SfxHandles,
    clip: SfxClip,
    volume: f32,
    rng: &mut u32,
) {
    let Some(handle) = handles.get(clip) else {
        return;
    };
    sfx.play(handle)
        .with_volume(linear_to_db(volume))
        .with_playback_rate(pitch_jitter(clip, rng) as f64);
}

pub fn sfx_player_system(
    mut events: MessageReader<SfxEvent>,
    handles: Option<Res<SfxHandles>>,
    ear: Query<&GlobalTransform, With<AudioEar>>,
    listeners: Query<&GlobalTransform, With<Camera3d>>,
    mixer: Option<Res<AudioMixerSettings>>,
    sfx: Option<Res<AudioChannel<SfxBus>>>,
    mut rng: Local<u32>,
) {
    let (Some(handles), Some(sfx)) = (handles, sfx) else {
        return;
    };
    // Mudo total nem toca (o bus estaria a −60 dB na mesma — poupança de CPU).
    if let Some(mixer) = mixer
        && (mixer.sfx * mixer.master) <= 0.0
    {
        return;
    }
    for event in events.read() {
        // Atenuação por distância no momento do evento; os buses sfx×master
        // são aplicados pelo kira (crate::music::mixer_sync) — um som a meio
        // responde a mudanças de volume, coisa que o modelo antigo (bus
        // multiplicado no spawn) não fazia.
        let volume = match event.position {
            Some(pos) => {
                let distance = ear_position(&ear, &listeners)
                    .map(|cam| cam.translation().distance(pos))
                    .unwrap_or(0.0);
                (1.0 - distance / 40.0).clamp(0.05, 1.0)
            }
            None => 0.6,
        };
        play_sfx(&sfx, &handles, event.clip, volume, &mut rng);
    }
}

/// O irmão de [`sfx_player_system`] para sons com ganho já calculado
/// (trovão, eventos a longa distância): sem a atenuação de 40 m.
pub fn sfx_loud_player_system(
    mut events: MessageReader<SfxLoudEvent>,
    handles: Option<Res<SfxHandles>>,
    mixer: Option<Res<AudioMixerSettings>>,
    sfx: Option<Res<AudioChannel<SfxBus>>>,
    mut rng: Local<u32>,
) {
    let (Some(handles), Some(sfx)) = (handles, sfx) else {
        return;
    };
    if let Some(mixer) = mixer
        && (mixer.sfx * mixer.master) <= 0.0
    {
        return;
    }
    for event in events.read() {
        play_sfx(
            &sfx,
            &handles,
            event.clip,
            event.volume.clamp(0.0, 1.0),
            &mut rng,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::SFX_NAME_REGISTRY;

    #[test]
    fn test_sfx_clip_files() {
        assert!(SfxClip::Hit.file().starts_with(""));
        // core 4: os stubs .wav sintéticos foram substituídos por clips
        // Text2Sound (.ogg) — nada na engine pode apontar a .wav.
        assert!(SfxClip::Ui.file().ends_with("ui.ogg"));
        for clip in SFX_CLIPS_ALL {
            assert!(clip.file().ends_with(".ogg"), "{} não é .ogg", clip.file());
        }
        // colheita nativa: 4 clips .ogg partilhados com a referência
        for clip in [
            SfxClip::ChopHit,
            SfxClip::ChopBreak,
            SfxClip::MineHit,
            SfxClip::MineBreak,
        ] {
            assert!(clip.file().contains("combat/"), "{}", clip.file());
        }
        // passe de juice (WS-A): os clips novos têm de ser .ogg e apontar a
        // pastas reais da árvore de assets (ui/, combat/, world/, ambient/).
        let juice = [
            (SfxClip::LevelUp, "ui/levelup.ogg"),
            (SfxClip::QuestDone, "ui/quest_complete.ogg"),
            (SfxClip::Travel, "combat/swing.ogg"),
            (SfxClip::Footstep, "ambient/footstep.ogg"),
            (SfxClip::FootstepWater, "ambient/footstep_water.ogg"),
            (SfxClip::Loot, "world/chest_open.ogg"),
        ];
        for (clip, suffix) in juice {
            assert!(clip.file().ends_with(suffix), "{}", clip.file());
        }
    }

    /// O registry de nomes tem de cobrir TODAS as variantes — um clip sem
    /// nome seria inalcançável por script e invisível para o audit.
    #[test]
    fn test_lua_registry_covers_all_clips() {
        for clip in SFX_CLIPS_ALL {
            assert!(
                SFX_NAME_REGISTRY.iter().any(|(_, c)| c == clip),
                "clip {:?} sem nome no SFX_NAME_REGISTRY",
                clip
            );
        }
        // e o mapeamento inverso resolve (case-insensitive).
        for (name, clip) in SFX_NAME_REGISTRY {
            assert_eq!(crate::names::sfx_clip_from_str(name), Some(*clip));
        }
    }

    /// O jitter de pitch só se aplica aos clips de rajada e vive dentro do
    /// ±8 % (a monotonia de rajada é o alvo; nunca é um clip novo).
    #[test]
    fn test_pitch_jitter_scope_and_bounds() {
        let mut rng = 0x5EED_1E55u32;
        for clip in SFX_CLIPS_ALL {
            let factor = pitch_jitter(*clip, &mut rng);
            if matches!(
                clip,
                SfxClip::Hit
                    | SfxClip::Whoosh
                    | SfxClip::EnemyHurt
                    | SfxClip::Footstep
                    | SfxClip::FootstepWater
                    | SfxClip::Coin
            ) {
                assert!(
                    (0.92..=1.08).contains(&factor),
                    "{clip:?} fora do ±8 %: {factor}"
                );
            } else {
                assert_eq!(factor, 1.0, "{clip:?} não é de rajada");
            }
        }
    }
}
