//! Ambiente da ENGINE — orçamento de sombras de ponto, gestos idle de NPC
//! e a lanterna do viajante (extraído do `ambient.rs` do monólito, exit5/G-2).
//!
//! Ficaram no monólito: o emissor visual de chuva (partículas do monólito), o
//! plugin (que ordena contra `worldsys`/`sky`/`particles` do monólito) e a
//! fatia de ÁUDIO (viva no `bevy_ext_audio::{sfx, ambience}` — o
//! `SfxPlugin`). A névoa/bioma vive no `bevy_ext_postfx::ambient`.
//!
//! TypePath históricos preservados com `#[type_path = "viber::ambient"]`.

use std::collections::HashMap;

use bevy::animation::AnimationPlayer;
use bevy::animation::transition::AnimationTransitions;
use bevy::prelude::*;
use bevy_ext_core::animation::CharacterAnimator;
// Contratos: estados do mundo com TypePath congelado no postfx (`env`), o
// marcador das luzes autorais com sombra, o herói e o contrato de script.
use bevy_ext_core::Player;
use bevy_ext_postfx::env::{AuthoredShadowLight, TravellerLantern};
// Câmara de editor deste crate (referenciada por path inteiro no
// `light_budget_system`): quem orça as luzes é a câmara activa.

/// Número máximo de PointLights com SHADOW MAP (cube de 6 faces) em
/// simultâneo — as mais próximas da câmara (ver `light_budget_system`).
///
/// Cada luz com sombra é SEIS vistas para o renderer: seis frusta, seis
/// culls, seis filas de draw e mais 25 MB no array do shadow map (1024²).
/// `VIBER_SHADOW_LIGHTS=<n>` afina-o sem recompilar — `0` desliga as sombras
/// de ponto (a luz continua a iluminar).
pub const SHADOW_LIGHT_BUDGET: usize = 12;

/// Orçamento efectivo: `VIBER_SHADOW_LIGHTS` ou [`SHADOW_LIGHT_BUDGET`]
/// limitado pelo tier de qualidade ([`set_quality_shadow_cap`]).
pub fn shadow_light_budget() -> usize {
    std::env::var("VIBER_SHADOW_LIGHTS")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or_else(|| {
            SHADOW_LIGHT_BUDGET.min(QUALITY_SHADOW_CAP.load(std::sync::atomic::Ordering::Relaxed))
        })
}

/// Teto do orçamento de sombras de ponto imposto por `<AdaptiveQuality>`
/// (`usize::MAX` = sem teto). O env `VIBER_SHADOW_LIGHTS` ganha-lhe: é o A/B
/// explícito de quem mede.
static QUALITY_SHADOW_CAP: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Troca o teto do tier de qualidade (`None` = orçamento cheio).
pub fn set_quality_shadow_cap(cap: Option<usize>) {
    QUALITY_SHADOW_CAP.store(
        cap.unwrap_or(usize::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Distância máxima (m) a que uma PointLight ainda ganha shadow map, medida
/// da câmara à SUPERFÍCIE da esfera de influência (`distância − range`).
///
/// O orçamento antigo dava sombra às 12 mais próximas **fosse qual fosse a
/// distância**: no campo aberto do `simple-rpg` isso eram 12 lanternas a
/// centenas de metros, invisíveis, a pagar 6 vistas de render cada uma.
/// Medido no spawn: 44,5 ms de frame com as sombras de ponto, 22,5 ms sem
/// elas — metade do frame ia para sombras que não se viam.
pub const SHADOW_LIGHT_MAX_DISTANCE: f32 = 60.0;

/// Distância efectiva: `VIBER_SHADOW_LIGHT_DISTANCE` ou o default.
pub fn shadow_light_max_distance() -> f32 {
    std::env::var("VIBER_SHADOW_LIGHT_DISTANCE")
        .ok()
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|value| *value >= 0.0)
        .unwrap_or(SHADOW_LIGHT_MAX_DISTANCE)
}

/// Luz do dia acima da qual as PointLight perdem o shadow map.
///
/// Ao meio-dia a contribuição de uma lanterna é ordens de grandeza abaixo do
/// sol: a sombra dela não se lê em píxel nenhum, mas custa as mesmas 6 vistas
/// de render. O gate solta-as ao anoitecer, que é quando se veem.
/// `VIBER_SHADOW_LIGHT_DAYLIGHT=1` desliga o gate (sombras sempre).
pub const SHADOW_LIGHT_DAYLIGHT_MAX: f32 = 0.35;

/// Histerese do gate de luz do dia — sem ela o crepúsculo liga/desliga as
/// sombras a cada refresh do orçamento.
const SHADOW_LIGHT_DAYLIGHT_BAND: f32 = 0.1;

/// Limiar efectivo: `VIBER_SHADOW_LIGHT_DAYLIGHT` ou o default.
pub fn shadow_light_daylight_max() -> f32 {
    std::env::var("VIBER_SHADOW_LIGHT_DAYLIGHT")
        .ok()
        .and_then(|raw| raw.parse::<f32>().ok())
        .unwrap_or(SHADOW_LIGHT_DAYLIGHT_MAX)
}

/// Interruptor de QA das sombras de ponto (extra `point-shadows` do
/// profiler). Desligado, o `light_budget_system` tira a sombra a TODAS as
/// lanternas — o A/B mede o custo das 6 vistas por luz sem reiniciar a
/// engine.
#[derive(Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::ambient"]
pub struct PointShadowsEnabled(pub bool);

impl Default for PointShadowsEnabled {
    fn default() -> Self {
        Self(shadow_light_budget() > 0)
    }
}
/// Histerese de rank: uma luz só PERDE a sombra acima de
/// BUDGET+BAND e só a ganha abaixo de BUDGET. Limitada a um terço do
/// orçamento em [`wants_point_shadow`]: com a banda cheia, o teto 4 do tier
/// Medium mantinha as 8 sombras do High e o teto 0 do Low deixava 4 acesas.
const SHADOW_LIGHT_BAND: usize = 4;
/// Política antiga, só para A/B: `VIBER_LIGHT_BUDGET=12` apaga (Visibility
/// hidden) tudo além das N luzes mais próximas — o comportamento pré-ronda
/// "prettier".
pub const LIGHT_BUDGET: usize = 12;
/// Intervalo de refrescamento do orçamento de luzes (s).
pub const LIGHT_BUDGET_INTERVAL: f32 = 1.0;

/// Intervalo entre gestos idle de NPC (s, ±jitter).
pub const GESTURE_MIN_INTERVAL: f32 = 7.0;

/// Orçamento de SOMBRAS de PointLight — não de luz.
///
/// Política: TODAS as luzes iluminam. O cluster do Bevy aguenta 204 objetos
/// (`MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS`) e o `range` por omissão (20 m)
/// limita o custo por pixel às luzes que tocam o pixel; o que é caro é o cube
/// shadow map (6 faces de cena por luz), e é isso que fica orçamentado às
/// [`SHADOW_LIGHT_BUDGET`] mais próximas COM sombra autorada
/// ([`AuthoredShadowLight`]), com a banda [`SHADOW_LIGHT_BAND`] de histerese.
///
/// A política antiga — ESCONDER tudo além das 12 mais próximas — apagava o
/// resto da vila e fazia as lanternas piscar ON/OFF ao andar (refresh de 1 s).
/// Sombras a 20+ m não se leem; luz a 20 m lê-se. Mantém-se acessível por
/// `VIBER_LIGHT_BUDGET=12` para o A/B.
/// Decide se ESTA luz fica com shadow map — parte pura do orçamento
/// (`rank`/distância/histerese), para testar sem mundo.
///
/// Ganha a sombra abaixo do orçamento e dentro da distância; só a PERDE
/// acima de orçamento+banda ou distância+banda. Sem a histerese nos dois
/// eixos, a fronteira trocava a cada refresh (1 s) e a lanterna piscava ao
/// andar para a frente e para trás.
pub fn wants_point_shadow(
    rank: usize,
    gap_m: f32,
    budget: usize,
    max_distance: f32,
    already_on: bool,
) -> bool {
    let near =
        gap_m <= max_distance || (already_on && gap_m <= max_distance + SHADOW_LIGHT_BAND as f32);
    let band = SHADOW_LIGHT_BAND.min(budget / 3);
    let ranked_in = rank < budget || (already_on && rank < budget + band);
    near && ranked_in
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]

pub fn light_budget_system(
    mut throttle: Local<f32>,
    // REAL: com o tempo virtual em pausa (modo Edit) o throttle nunca baixava
    // e as sombras ficavam presas às 12 luzes do 1.º frame, mesmo movendo a
    // câmara do editor pelo mapa.
    time: Res<Time<bevy::time::Real>>,
    editor: Query<&GlobalTransform, With<crate::editor_camera::EditorCamera>>,
    cameras: crate::editor_camera::EyeCameraQuery,
    lights: Query<(Entity, &GlobalTransform), (With<PointLight>, Without<Camera3d>)>,
    shadow_lights: Query<(Entity, &GlobalTransform), (With<PointLight>, With<AuthoredShadowLight>)>,
    mut visibilities: Query<&mut Visibility, With<PointLight>>,
    mut shadow_state: Query<&mut PointLight, With<AuthoredShadowLight>>,
    enabled: Res<PointShadowsEnabled>,
    clock: Option<Res<crate::worldsys::DayCycleState>>,
) {
    *throttle -= time.delta_secs();
    if *throttle > 0.0 {
        return;
    }
    *throttle = LIGHT_BUDGET_INTERVAL;
    // `iter().next()` e não `single()`: com ≥2 câmaras o `single()` falhava
    // (mesma semântica do 1.º player do `music_driver`). Em modo editor a
    // câmara de editor é que orça — ver `active_camera_eye`.
    let Some(cam) = crate::editor_camera::active_camera_eye(&editor, &cameras) else {
        return;
    };
    let cam_pos = cam.translation();

    // Braço de controlo (QA): a política antiga — luzes APAGADAS além de N.
    if std::env::var("VIBER_LIGHT_BUDGET").as_deref() == Ok("12") {
        let mut by_distance: Vec<(Entity, f32)> = lights
            .iter()
            .map(|(entity, t)| (entity, t.translation().distance_squared(cam_pos)))
            .collect();
        by_distance.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (i, (entity, _)) in by_distance.iter().enumerate() {
            if let Ok(mut visibility) = visibilities.get_mut(*entity) {
                let wanted = if i < LIGHT_BUDGET {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
                if *visibility != wanted {
                    *visibility = wanted;
                }
            }
        }
        return;
    }

    // Ranking por distância à SUPERFÍCIE da esfera de influência (distância
    // menos `range`): uma lanterna de `range` 20 m a 300 m da câmara não
    // ilumina um único pixel, e o cube shadow map dela custava as mesmas 6
    // vistas que a do candeeiro em cima do herói.
    let mut ranked: Vec<(Entity, f32)> = Vec::with_capacity(shadow_lights.iter().len());
    for (entity, transform) in shadow_lights.iter() {
        let range = shadow_state.get(entity).map(|l| l.range).unwrap_or(0.0);
        let gap = (transform.translation().distance(cam_pos) - range).max(0.0);
        ranked.push((entity, gap));
    }
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1));
    let max_distance = shadow_light_max_distance();
    let budget = shadow_light_budget();
    // Gate de luz do dia: de dia a sombra da lanterna não se lê (ver
    // `SHADOW_LIGHT_DAYLIGHT_MAX`). A banda usa o estado ACTUAL de uma luz
    // qualquer — a decisão é global, por isso basta o limiar deslocar-se
    // conforme já estejam ou não ligadas.
    let daylight = clock
        .as_deref()
        .map(|clock| {
            // Minuto VISUAL (com night-cap): o sol/atmosfera já estão no
            // cap, e o gate não pode desligar sombras às 05:30 com o ecrã
            // ainda à noite.
            crate::worldsys::daylight_factor(
                clock.visual_minute(),
                clock.dawn_minute,
                clock.dusk_minute,
            )
        })
        .unwrap_or(0.0);
    let any_shadow_on = shadow_state.iter().any(|light| light.shadow_maps_enabled);
    let daylight_limit = shadow_light_daylight_max()
        + if any_shadow_on {
            SHADOW_LIGHT_DAYLIGHT_BAND
        } else {
            0.0
        };
    let dark_enough = daylight <= daylight_limit;
    for (rank, (entity, gap)) in ranked.iter().enumerate() {
        let Ok(mut light) = shadow_state.get_mut(*entity) else {
            continue;
        };
        let want = enabled.0
            && dark_enough
            && wants_point_shadow(rank, *gap, budget, max_distance, light.shadow_maps_enabled);
        if light.shadow_maps_enabled != want {
            light.shadow_maps_enabled = want;
            #[cfg(feature = "experimental_pbr_pcss")]
            {
                light.soft_shadows_enabled = want;
            }
        }
    }
}

// ── gestos idle de NPC ──────────────────────────────────────────────────

/// NPCs (sem script, sem player) tocam um clip de gesto periodicamente.
///
/// O gesto é uma ACÇÃO one-shot: toca uma vez, tranca o driver de locomoção
/// enquanto dura e devolve o rig no fim. Antes chamava `transitions.play` sem
/// `repeat` e sem qualquer regresso ao idle — o NPC congelava na última pose do
/// aceno para sempre.
#[allow(clippy::type_complexity)]

pub fn npc_gesture_system(
    mut throttle: Local<f32>,
    time: Res<Time>,
    mut npcs: Query<
        (Entity, &mut CharacterAnimator),
        (
            With<Name>,
            Without<Player>,
            Without<bevy_ext_core::LuaScriptRef>,
        ),
    >,
    mut animation_players: crate::animation::PlayerQuery,
    mut timers: Local<HashMap<Entity, f32>>,
) {
    *throttle -= time.delta_secs();
    if *throttle > 0.0 {
        return;
    }
    // Tempo REAL desde a última passada (1 s + overshoot do frame): o `-1.0`
    // fixo contava um segundo por passada mesmo com frames longos.
    let elapsed = 1.0 - *throttle;
    *throttle = 1.0;
    // Timers de NPCs despawnados saem do map — o Local crescia para sempre
    // (cada respawn de criatura/NPC leakava uma entrada).
    timers.retain(|entity, _| npcs.contains(*entity));
    for (entity, mut animator) in &mut npcs {
        let entry = timers.entry(entity).or_insert_with(|| {
            // primeiro gesto entre 5 e 12 s (pseudo-aleatório por entidade)
            5.0 + (entity.to_bits() % 7000) as f32 / 1000.0
        });
        *entry -= elapsed;
        if *entry > 0.0 {
            continue;
        }
        *entry = GESTURE_MIN_INTERVAL + (entity.to_bits() % 5000) as f32 / 1000.0;
        // Um NPC a andar (ou a meio de outra acção) não é interrompido — o
        // gesto é decoração de quem está parado.
        if animator.is_busy() || animator.state != Some(crate::animation::AnimState::Idle) {
            continue;
        }
        let Some(node) = animator
            .node_matching(|n| n.contains("talk") || n.contains("wave") || n.contains("call"))
        else {
            continue;
        };
        crate::animation::play_action(
            &mut animator,
            &mut animation_players,
            node,
            std::time::Duration::from_millis(250),
            false,
        );
    }
}

/// Intensidade da lanterna à noite (lm).
///
/// **600 000 (era 1 200 → 30 000).** A 1 200 lm a lanterna valia ~10 lux a
/// 3 m — 17× ABAIXO do ambiente noturno, e nem se via. A 30 000 lm o pool
/// ainda era ténue na engine viva: o default do Bevy é 1M lm e mesmo esse é
/// calibrado para uma exposição ~10 stops mais clara que a do jogo, e a
/// noite leva um net de −1,5 EV. 600k lm dão ~53 000 lux a 3 m — um pool
/// quente inequívoco à frente do herói, com o resto do mundo escuro.
/// `range` 12 mantém a poça antes do `range` default de 20 m das tochas.
pub const LANTERN_MAX_INTENSITY: f32 = 600_000.0;
/// Alcance da lanterna (m).
pub const LANTERN_RANGE: f32 = 12.0;
/// Altura da chama acima do chão do herói (m).
const LANTERN_HEIGHT: f32 = 2.2;

// Marker da lanterna do viajante (uma instância por mundo): contrato no
// `bevy_ext_postfx::env` (exit3/wave-c) — o `torch_pool_drive` do crate lê O
// MESMO marcador; importado no topo.

/// Intensidade da lanterna controlada pelo JOGO (`viber.lantern(intensity)`).
///
/// A tocha/lampião equipado acende a luz mesmo de DIA (a lanterna automática
/// só existe à noite) e reforça-a à noite. `0` (default) = só a automática.
/// Vive num recurso para o `lantern_driver` compor sem o Lua tocar em
/// componentes.
#[derive(Debug, Clone, Copy, Resource, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::ambient"]
pub struct TorchLightOverride {
    pub intensity: f32,
}

/// Spawna (uma vez) e conduz a lanterna: segue o herói, intensidade ∝ noite
/// com fade exponencial suave (~0.4 s de constante — sem piscar).
#[allow(clippy::type_complexity)]
pub fn lantern_driver(
    time: Res<Time>,
    atmosphere: Option<Res<bevy_ext_postfx::env::AtmosphereState>>,
    torch: Option<Res<TorchLightOverride>>,
    players: Query<&GlobalTransform, With<Player>>,
    mut lanterns: Query<(&mut Transform, &mut PointLight, &mut Visibility), With<TravellerLantern>>,
    mut commands: Commands,
) {
    // `iter().next()` e não `single()`: ≥2 players não pode matar a lanterna.
    let Some(player) = players.iter().next() else {
        return;
    };
    let anchor = player.translation() + Vec3::Y * LANTERN_HEIGHT;
    let night = atmosphere.map(|a| a.night.clamp(0.0, 1.0)).unwrap_or(0.0);
    let night_target = LANTERN_MAX_INTENSITY * night;
    let torch_target = torch.map(|t| t.intensity.max(0.0)).unwrap_or(0.0);
    let target = night_target.max(torch_target);
    let Ok((mut transform, mut light, mut visibility)) = lanterns.single_mut() else {
        commands.spawn((
            TravellerLantern,
            PointLight {
                color: Color::srgb(1.0, 0.69, 0.40), // #ffb066 quente
                intensity: 0.0,
                range: LANTERN_RANGE,
                ..Default::default()
            },
            Transform::from_translation(anchor),
            Visibility::Inherited,
            Name::new("fx:lanterna-viajante"),
        ));
        return;
    };
    transform.translation = anchor;
    // Fade exponencial: constante de tempo ~0.4 s em ambos os sentidos.
    let dt = time.delta_secs().clamp(0.0, 0.2);
    let blend = 1.0 - (-dt * 2.5).exp();
    light.intensity += (target - light.intensity) * blend;
    let wanted = if light.intensity < LANTERN_MAX_INTENSITY * 0.02 {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    if *visibility != wanted {
        *visibility = wanted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_shadow_needs_rank_and_distance() {
        // 1.ª do ranking, colada ao jogador: sim.
        assert!(wants_point_shadow(
            0,
            5.0,
            12,
            SHADOW_LIGHT_MAX_DISTANCE,
            false
        ));
        // 1.ª do ranking mas a 300 m da esfera de influência: não.
        assert!(!wants_point_shadow(
            0,
            300.0,
            12,
            SHADOW_LIGHT_MAX_DISTANCE,
            false
        ));
        // Perto, mas fora do orçamento: não.
        assert!(!wants_point_shadow(
            20,
            5.0,
            12,
            SHADOW_LIGHT_MAX_DISTANCE,
            false
        ));
        // Orçamento 0 (VIBER_SHADOW_LIGHTS=0) desliga tudo.
        assert!(!wants_point_shadow(
            0,
            1.0,
            0,
            SHADOW_LIGHT_MAX_DISTANCE,
            false
        ));
    }

    /// Histerese nos DOIS eixos: quem já tem sombra aguenta a banda antes de
    /// a perder (senão pisca ao andar na fronteira, refresh de 1 s).

    #[test]
    fn point_shadow_hysteresis_holds_the_border() {
        let max = SHADOW_LIGHT_MAX_DISTANCE;
        // Distância: 2 m para lá do limite.
        assert!(wants_point_shadow(0, max + 2.0, 12, max, true));
        assert!(!wants_point_shadow(0, max + 2.0, 12, max, false));
        // Distância: muito para lá da banda — perde de qualquer forma.
        assert!(!wants_point_shadow(0, max + 50.0, 12, max, true));
        // Rank: 13.ª mantém-se acesa, 17.ª (12 + banda) já não.
        assert!(wants_point_shadow(13, 5.0, 12, max, true));
        assert!(!wants_point_shadow(16, 5.0, 12, max, true));
    }

    /// Os tetos do `<AdaptiveQuality>` cortam mesmo: a banda de histerese
    /// não pode segurar as sombras que o tier anterior deixou ligadas.

    #[test]
    fn point_shadow_band_respects_quality_caps() {
        let max = SHADOW_LIGHT_MAX_DISTANCE;
        // High (8) → Medium (4): a 5.ª fica pela banda (1), a 6.ª apaga-se.
        assert!(wants_point_shadow(4, 5.0, 4, max, true));
        assert!(!wants_point_shadow(5, 5.0, 4, max, true));
        // Low (0): nenhuma sombra fica acesa.
        assert!(!wants_point_shadow(0, 0.0, 0, max, true));
    }

    /// O gate de luz do dia usa a mesma curva do tint (`daylight_factor`):
    /// ao meio-dia 1.0 (acima do limiar), à meia-noite 0.0.

    #[test]
    fn daylight_gate_closes_at_noon_and_opens_at_night() {
        let (dawn, dusk) = (330.0, 1170.0);
        assert!(crate::worldsys::daylight_factor(720.0, dawn, dusk) > SHADOW_LIGHT_DAYLIGHT_MAX);
        assert!(crate::worldsys::daylight_factor(1320.0, dawn, dusk) <= SHADOW_LIGHT_DAYLIGHT_MAX);
    }
}
