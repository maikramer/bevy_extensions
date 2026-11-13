//! Loops de ambiente: água (lago/rio), cachoeiras e chuva — a fatia de áudio
//! do `src/ambient.rs` do monólito.
//!
//! Todos nascem no bus de AMBIENTE ([`AmbienceLoop`]) e o volume move-se por
//! fade ([`fade_step`]) para o alvo da distância — a água é ambiente: obedece
//! ao `<AudioMixer ambience>`/duck, não ao SFX.

use bevy::prelude::*;
use bevy_ext_core::{InteriorSceneConfig, Player};
use bevy_kira_audio::{AudioInstance, AudioTween};

use crate::host::{AudioPaths, AudioWorldState, HostWaterBodies, WaterKind};
use crate::music::{
    AmbienceLoop, AudioLoopPending, LoopInstance, LoopVolume, fade_step, linear_to_db,
};

// ── Água ambiente ───────────────────────────────────────────────────────
//
// Os clips `water_lake.ogg` / `water_flow.ogg` existiam no pool e não tinham
// consumidor: a margem era cega e surda. Dois loops permanentes (um por tipo
// de corpo) nascem quando o host publica [`HostWaterBodies`]; o driver move
// o volume deles para a distância À LINHA DE ÁGUA (a mesma métrica do
// splatter/spawner) — fade out a `WATER_AUDIBLE_RADIUS` m.

/// Loop de água ativo (lago OU rio).
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::ambient"]
pub struct WaterAmbienceLoop {
    /// `true` = o loop de rio (water_flow); `false` = lago (water_lake).
    pub river: bool,
}

/// Distância (m) além da qual o loop está em silêncio.
const WATER_AUDIBLE_RADIUS: f32 = 26.0;
/// Ganho máximo junto à água (antes dos buses do mixer).
const WATER_MAX_GAIN: f32 = 0.5;

/// Spawna UMA vez os dois loops quando o host publica os corpos de água. Os
/// paths resolvem contra o `sfx_dir` ([`AudioPaths`]).
pub fn setup_water_ambience(
    mut commands: Commands,
    water: Res<HostWaterBodies>,
    spawned: Query<(), With<WaterAmbienceLoop>>,
    paths: Option<Res<AudioPaths>>,
) {
    if water.0.is_none() {
        return;
    }
    if !spawned.is_empty() {
        return;
    }
    let Some(paths) = paths else {
        return;
    };
    for (river, clip) in [
        (false, "world/water_lake.ogg"),
        (true, "world/water_flow.ogg"),
    ] {
        commands.spawn((
            WaterAmbienceLoop { river },
            // Água é ambiente: obedece ao `<AudioMixer ambience>`/duck, não ao SFX.
            AmbienceLoop,
            AudioLoopPending {
                url: paths.sfx_path(clip),
                music: false,
            },
        ));
    }
}

/// Move o volume dos loops para o alvo da distância à linha de água.
pub fn water_ambience_driver(
    time: Res<Time>,
    water: Res<HostWaterBodies>,
    players: Query<&GlobalTransform, With<Player>>,
    mut loops: Query<(&WaterAmbienceLoop, &LoopInstance, &mut LoopVolume)>,
    mut instances: ResMut<Assets<AudioInstance>>,
) {
    // TODO(viber-exit): sem a publicação do `WaterBodies` (host sem terreno)
    // o driver não mexe — igual ao `TerrainRuntime` ausente do monólito.
    let Some(water) = water.0.as_deref() else {
        return;
    };
    // `iter().next()` e não `single()`: ≥2 players não pode matar o áudio.
    let Some(player) = players.iter().next() else {
        return;
    };
    let pos = player.translation();
    // [lake, river] — `None` = nenhum corpo desse tipo: sem água não há som.
    let best = [
        water
            .min_distance_to_waterline(WaterKind::Lake, pos.x, pos.z)
            .unwrap_or(f32::MAX),
        water
            .min_distance_to_waterline(WaterKind::River, pos.x, pos.z)
            .unwrap_or(f32::MAX),
    ];
    let dt = time.delta_secs().clamp(0.0, 0.2);
    for (tag, instance, mut volume) in &mut loops {
        let d = if tag.river { best[1] } else { best[0] };
        let falloff = (1.0 - d / WATER_AUDIBLE_RADIUS).clamp(0.0, 1.0);
        // Curva quadrática — perto da água o som cresce depressa. O bus
        // (sfx×master) é do canal kira, não entra aqui.
        let target = WATER_MAX_GAIN * falloff * falloff;
        let next = fade_step(volume.0, target, dt, 0.5);
        if next != volume.0 {
            let Some(mut snd) = instances.get_mut(&instance.0) else {
                continue;
            };
            snd.set_decibels(
                linear_to_db(next),
                AudioTween::linear(std::time::Duration::from_millis(60)),
            );
            volume.0 = next;
        }
    }
}

// ── Cachoeiras (loops posicionais) ──────────────────────────────────────
//
// Diferente dos loops globais de água: UMA entidade por queda grande
// (`WaterBody.cascades` com `waterfall`), volume por distância ao
// caldeirão, raio audível e ganho crescendo com a queda — água grande
// ouve-se de longe.

/// Loop posicional de cachoeira (uma entidade por queda).
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::ambient"]
pub struct WaterfallLoop {
    /// Base da queda (world XZ) — o caldeirão.
    pub at: Vec2,
    /// Queda (m) — escala o raio audível e o ganho.
    pub drop: f32,
}

/// Distância (m) base além da qual a cachoeira está em silêncio; cresce
/// `3 m` por metro de queda.
const WATERFALL_AUDIBLE_RADIUS: f32 = 34.0;
/// Ganho junto a uma queda de 3 m, antes dos buses do mixer (cresce com a
/// queda, teto de segurança).
const WATERFALL_MAX_GAIN: f32 = 0.55;

/// Spawna UMA vez um loop por cachoeira publicada no [`HostWaterBodies`]
/// (no Viber, as `cascades` com `waterfall` dos corpos de rio).
pub fn setup_waterfall_ambience(
    mut commands: Commands,
    water: Res<HostWaterBodies>,
    spawned: Query<(), With<WaterfallLoop>>,
    paths: Option<Res<AudioPaths>>,
) {
    let Some(water) = water.0.as_deref() else {
        return;
    };
    if !spawned.is_empty() {
        return;
    }
    for spot in water.waterfall_spots() {
        commands.spawn((
            WaterfallLoop {
                at: Vec2::new(spot.at[0], spot.at[1]),
                drop: spot.drop,
            },
            // Cachoeira é ambiente (ver `AmbienceLoop`).
            AmbienceLoop,
            AudioLoopPending {
                url: paths
                    .as_deref()
                    .map(|p| p.sfx_path("world/water_waterfall.ogg"))
                    .unwrap_or_else(|| "world/water_waterfall.ogg".to_string()),
                music: false,
            },
        ));
    }
}

/// Move o volume de cada loop de cachoeira para o alvo da distância ao
/// caldeirão (mesma curva quadrática dos loops globais de água).
pub fn waterfall_ambience_driver(
    time: Res<Time>,
    players: Query<&GlobalTransform, With<Player>>,
    mut loops: Query<(&WaterfallLoop, &LoopInstance, &mut LoopVolume)>,
    mut instances: ResMut<Assets<AudioInstance>>,
) {
    if loops.is_empty() {
        return;
    }
    // `iter().next()` e não `single()`: ≥2 players não pode matar o áudio.
    let Some(player) = players.iter().next() else {
        return;
    };
    let pos = player.translation();
    let dt = time.delta_secs().clamp(0.0, 0.2);
    for (tag, instance, mut volume) in &mut loops {
        let d = (Vec2::new(pos.x, pos.z) - tag.at).length();
        let radius = WATERFALL_AUDIBLE_RADIUS + tag.drop * 3.0;
        let falloff = (1.0 - d / radius).clamp(0.0, 1.0);
        let gain = (WATERFALL_MAX_GAIN * (0.5 + tag.drop * 0.12)).min(0.95);
        // Curva quadrática — perto da queda o som cresce depressa. O bus
        // (sfx×master) é do canal kira, não entra aqui.
        let target = gain * falloff * falloff;
        let next = fade_step(volume.0, target, dt, 0.5);
        if next != volume.0 {
            let Some(mut snd) = instances.get_mut(&instance.0) else {
                continue;
            };
            snd.set_decibels(
                linear_to_db(next),
                AudioTween::linear(std::time::Duration::from_millis(60)),
            );
            volume.0 = next;
        }
    }
}

// ── Chuva (áudio) ───────────────────────────────────────────────────────
//
// O loop SFX `ambient/rain_loop.ogg` (8 s costura-sem-clique) com volume ∝
// intensidade. O EMISSOR de gotas e as ondulações são visuais e ficaram na
// engine (no Vael o `bevy_elements` desenha a chuva; o `EngineRainVisuals`
// do core é o interruptor deles) — aqui vive só o áudio.

/// Ganho máximo do loop de chuva (antes dos buses do mixer).
const RAIN_MAX_GAIN: f32 = 0.4;

/// Marker do loop de áudio da chuva.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::ambient"]
pub struct RainAmbienceLoop;

/// Spawna UMA vez o loop de áudio da chuva (volume 0 até o driver o levantar).
///
/// O gate é o host ter publicado o [`AudioWorldState`] (no monólito era o
/// `WeatherState` existir) — apps sem clima não nascem com o loop.
pub fn setup_rain_ambience(
    mut commands: Commands,
    world: Option<Res<AudioWorldState>>,
    spawned: Query<(), With<RainAmbienceLoop>>,
    paths: Option<Res<AudioPaths>>,
) {
    if world.is_none() || !spawned.is_empty() {
        return;
    }
    commands.spawn((
        RainAmbienceLoop,
        // Chuva é ambiente (ver `AmbienceLoop`).
        AmbienceLoop,
        AudioLoopPending {
            url: paths
                .as_deref()
                .map(|p| p.sfx_path("ambient/rain_loop.ogg"))
                .unwrap_or_else(|| "ambient/rain_loop.ogg".to_string()),
            music: false,
        },
    ));
}

/// Volume do loop de chuva ∝ intensidade (fade lento — a chuva não liga/desliga).
pub fn rain_ambience_driver(
    time: Res<Time>,
    world: Option<Res<AudioWorldState>>,
    scene: Option<Res<InteriorSceneConfig>>,
    players: Query<&GlobalTransform, With<Player>>,
    mut loops: Query<(&LoopInstance, &mut LoopVolume), With<RainAmbienceLoop>>,
    mut instances: ResMut<Assets<AudioInstance>>,
) {
    let Some(world) = world else {
        return;
    };
    // Dentro da bolsa de interior não chove: o emissor já se calava, mas o
    // loop de áudio e as ondulações continuavam.
    let indoors = scene.as_deref().is_some_and(|scene| {
        players
            .iter()
            .next()
            .is_some_and(|player| scene.contains(player.translation().x, player.translation().z))
    });
    let intensity = if indoors {
        0.0
    } else {
        world.rain.clamp(0.0, 1.0)
    };
    // O bus (sfx×master) é do canal kira, não entra aqui.
    let target = RAIN_MAX_GAIN * intensity;
    let dt = time.delta_secs().clamp(0.0, 0.2);
    for (instance, mut volume) in &mut loops {
        let next = fade_step(volume.0, target, dt, 0.35);
        if next != volume.0 {
            let Some(mut snd) = instances.get_mut(&instance.0) else {
                continue;
            };
            snd.set_decibels(
                linear_to_db(next),
                AudioTween::linear(std::time::Duration::from_millis(60)),
            );
            volume.0 = next;
        }
    }
}
