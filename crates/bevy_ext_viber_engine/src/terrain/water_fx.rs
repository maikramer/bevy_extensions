//! Water FX — splash bursts and expanding surface ripples for anything that
//! walks into, through or out of a water body.
//!
//! The water surface itself is a static mesh with a specialized shader (see
//! [`super::water_material`]): the Bevy 0.19 slot-1 storage promotion never
//! re-uploads a custom material uniform, so **the shader cannot be told where
//! the hero is**. The wake therefore lives in the ECS instead: every contact
//! spawns short-lived entities that the normal transparent pass draws over
//! the mirror.
//!
//! Three contacts, three effects:
//!
//! * **Enter** (dry → submerged past [`WADE_ENTER_DEPTH`]; back out only
//!   below [`WADE_EXIT_DEPTH`], so the shoreline does not chatter) — a
//!   `splash` burst scaled by impact speed and submersion, plus one wide
//!   ripple.
//! * **Wade** (moving while submerged) — a `wade` burst and a ripple every
//!   [`RIPPLE_INTERVAL`] of *distance travelled*, so the cadence follows the
//!   stride instead of the frame rate, and stopping stops the wake.
//! * **Exit** — a small `splash` at the last wet position.
//!
//! Everything is driven off [`super::runtime::TerrainRuntime`]'s registry
//! ([`super::water::WaterBody::surface_y_at`]), so it works for lakes and
//! rivers alike. A caixa XZ de cada corpo ([`body_bounds`]) filtra os
//! nadadores em terra antes da query: sem ela, cada um dos ~170 nadadores
//! pagava o `nearest_on_path` do rio — O(estações) — todos os frames.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};

use super::runtime::TerrainRuntime;
use super::water_waves::{WaterKinematics, WaterWaveWorld, WaveImpulseRequest};
use crate::particles::spawn_burst;
use crate::recipes::ParticleSpec;

/// Fallback horizontal interaction radius (meters) when a swimmer lacks [`WaterKinematics`].
pub const DEFAULT_SWIMMER_RADIUS: f32 = 0.42;

/// Submersion (meters of water over the entity's feet) that flips *dry →
/// wet*. Deliberately above [`WADE_EXIT_DEPTH`]: without the hysteresis a
/// hero walking the shoreline crosses a single threshold back and forth on
/// terrain-sampling noise and machine-guns entry splashes.
pub const WADE_ENTER_DEPTH: f32 = 0.22;
/// Submersion below which a wet entity counts as *out* again.
pub const WADE_EXIT_DEPTH: f32 = 0.10;
/// Distance walked (meters) between two wake ripples.
pub const RIPPLE_INTERVAL: f32 = 0.9;
/// Speed (m/s) below which a submerged entity stops making a wake.
pub const WAKE_MIN_SPEED: f32 = 0.35;
/// Ripple lifetime (s).
pub const RIPPLE_LIFE: f32 = 1.5;
/// Ripple radius at birth / at death (meters).
pub const RIPPLE_START_RADIUS: f32 = 0.35;
pub const RIPPLE_END_RADIUS: f32 = 2.6;
/// Ripple lift over the mirror (meters) — enough to sort in front of the
/// water surface without reading as a floating disc.
pub const RIPPLE_LIFT: f32 = 0.045;
/// Concurrent ripples allowed; beyond this the oldest are left to expire and
/// new ones are skipped (a crowd wading a ford must not flood the pass).
pub const MAX_RIPPLES: usize = 48;
/// Ring segments of the ripple mesh.
const RIPPLE_SEGMENTS: usize = 40;

// Invariantes das constantes acima, verificadas em COMPILE TIME (um `assert!`
// sobre consts em teste é sempre verdade e o clippy rejeita-o).
const _: () = {
    assert!(RIPPLE_START_RADIUS > 0.0);
    assert!(RIPPLE_START_RADIUS < RIPPLE_END_RADIUS);
    // O anel de entrada nasce com um multiplicador de 1.35 — mesmo assim tem
    // de ficar um efeito local, não um disco a cobrir a lagoa.
    assert!(RIPPLE_END_RADIUS * 1.35 < 4.0);
    // Histerese: sair exige menos submersão do que entrar.
    assert!(WADE_EXIT_DEPTH > 0.0);
    assert!(WADE_EXIT_DEPTH < WADE_ENTER_DEPTH);
    assert!(WADE_ENTER_DEPTH < 0.5);
};

// ── componentes ─────────────────────────────────────────────────────────

/// Anything that should splash. Inserted by [`tag_swimmers`] on the hero and
/// on every AI creature; also fine to add by hand.
#[derive(Debug, Clone, Component, Default, Reflect)]
#[reflect(Component)]
pub struct WaterContact {
    /// Submerged on the previous tick.
    pub wet: bool,
    /// Distance walked since the last ripple (meters).
    pub since_ripple: f32,
    /// Previous world position (for the speed estimate).
    pub last_pos: Option<Vec3>,
}

/// One expanding surface ring.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
pub struct Ripple {
    pub age: f32,
    pub life: f32,
    pub start_radius: f32,
    pub end_radius: f32,
}

/// Shared ring mesh + a cheap way to make ripple materials.
#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct RippleAssets {
    pub mesh: Handle<Mesh>,
    /// Materiais dos anéis RECICLADOS. Cada anel anima o SEU alpha (o fade é
    /// por idade, ver `ripple_system`), portanto não podem partilhar um — mas
    /// podem voltar a usar-se quando o anel anterior morre. Antes disto cada
    /// anel nascia com um `materials.add` novo e o material ficava residente
    /// para sempre (o Bevy não faz GC de assets): com a chuva eram ~14
    /// anéis/s a acumular sem limite.
    pub materials: Vec<Handle<StandardMaterial>>,
}

// ── lógica pura (testada) ───────────────────────────────────────────────

/// Ripple radius at age `t` — fast at birth, easing out (a real ring loses
/// speed as it spreads), so `t=0` is the start radius and `t=life` the end.
pub fn ripple_radius(age: f32, life: f32, start: f32, end: f32) -> f32 {
    let t = (age / life.max(1e-4)).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    start + (end - start) * eased
}

/// Ripple alpha at age `t`: a short fade-in so the ring is not born at full
/// strength, then a linear fade to nothing.
pub fn ripple_alpha(age: f32, life: f32) -> f32 {
    let t = (age / life.max(1e-4)).clamp(0.0, 1.0);
    let rise = (t / 0.12).clamp(0.0, 1.0);
    rise * (1.0 - t) * (1.0 - t)
}

/// Parameterized wave/foam impulse derived from entry speed and body footprint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactImpulse {
    pub radius: f32,
    pub strength: f32,
    pub foam: f32,
    pub total_speed: f32,
}

/// Computes world XZ crossing where an entity's feet descended past
/// `surface_y - depth_thresh` along `[previous, current]`.
pub fn swept_entry_xz(previous: Vec3, current: Vec3, surface_y: f32, depth_thresh: f32) -> Vec2 {
    let dy = current.y - previous.y;
    let trigger_y = surface_y - depth_thresh;
    if dy < -1e-4 && previous.y > trigger_y && current.y <= trigger_y {
        let t = ((trigger_y - previous.y) / dy).clamp(0.0, 1.0);
        Vec2::new(
            previous.x + (current.x - previous.x) * t,
            previous.z + (current.z - previous.z) * t,
        )
    } else {
        Vec2::new(current.x, current.z)
    }
}

/// Bernoulli-scaled splash crater and foam impulse on jumping/falling into water.
/// Threshold (m) between shallow ankle/shin shoals and waist wading water.
pub const WADING_MID_DEPTH: f32 = 0.38;
/// Threshold (m) at which wading transitions into deep swimming water.
pub const DEEP_SWIM_DEPTH: f32 = 0.88;

/// Depth stratification for shore/shallows vs wading vs deep water effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaterDepthZone {
    /// Ankle/shin margin (`< 0.38 m`): crisp light rings, rapid bed friction.
    Shallow,
    /// Knee-to-chest wading (`0.38 .. 0.88 m`): heavy turbulent drag, frothy wakes.
    Wading,
    /// Deep swimming basin (`>= 0.88 m`): booming plunge crater, rising bubbles & wide wake.
    Deep,
}

/// Classifies local water depth into [`WaterDepthZone`].
pub fn classify_depth_zone(depth_m: f32) -> WaterDepthZone {
    if depth_m < WADING_MID_DEPTH {
        WaterDepthZone::Shallow
    } else if depth_m < DEEP_SWIM_DEPTH {
        WaterDepthZone::Wading
    } else {
        WaterDepthZone::Deep
    }
}

pub fn compute_entry_impulse(
    fall_speed: f32,
    planar_speed: f32,
    body_radius: f32,
    submersion: f32,
) -> ImpactImpulse {
    let v_down = fall_speed.max(0.0);
    let v_horiz = planar_speed.max(0.0);
    let total_speed = (v_down + 0.40 * v_horiz).clamp(0.4, 28.0);
    let depth_gain = (submersion / 0.65).clamp(0.35, 1.0);
    let (r_mul, s_mul, f_mul) = match classify_depth_zone(submersion) {
        WaterDepthZone::Shallow => (0.82, 0.78, 0.82),
        WaterDepthZone::Wading => (1.0, 1.0, 1.0),
        WaterDepthZone::Deep => (1.18, 1.24, 1.16),
    };
    let radius = (body_radius.max(0.25) * (1.25 + 0.05 * v_down + 0.03 * v_horiz) * r_mul)
        .clamp(0.45, 2.25);
    let strength =
        ((0.032 + 0.022 * v_down + 0.010 * v_horiz) * depth_gain * s_mul).clamp(0.02, 0.42);
    let foam =
        ((0.20 + 0.045 * v_down + 0.028 * v_horiz) * depth_gain * f_mul).clamp(0.14, 0.95);
    ImpactImpulse {
        radius,
        strength,
        foam,
        total_speed,
    }
}

/// Continuous Kelvin bow-stern pressure dipole while wading or swimming.
pub fn compute_wake_impulse(
    planar_speed: f32,
    body_radius: f32,
    submersion: f32,
) -> ImpactImpulse {
    let speed = planar_speed.max(0.0);
    let speed_norm = ((speed - WAKE_MIN_SPEED) / 4.5).clamp(0.15, 1.45);
    let depth_gain = (submersion / 0.70).clamp(0.25, 1.0);
    let (r_mul, s_mul, f_mul) = match classify_depth_zone(submersion) {
        WaterDepthZone::Shallow => (0.78, 0.78, 0.75),
        WaterDepthZone::Wading => (1.0, 1.14, 1.12),
        WaterDepthZone::Deep => (1.26, 0.94, 1.05),
    };
    let radius = (body_radius.max(0.25) * (1.15 + 0.35 * speed_norm) * r_mul).clamp(0.42, 1.50);
    let strength = ((0.010 + 0.018 * speed_norm) * depth_gain * s_mul).clamp(0.004, 0.042);
    let foam = ((0.14 + 0.36 * speed_norm) * depth_gain * f_mul).clamp(0.05, 0.65);
    ImpactImpulse {
        radius,
        strength,
        foam,
        total_speed: speed,
    }
}

/// Particle count of an entry splash: the vertical impact speed sets the
/// crown, the submersion scales it down so stepping into ankle-deep water is
/// a plop and jumping into a bowl is a proper splash.
pub fn splash_count(impact_speed: f32, submersion: f32) -> usize {
    let depth_gain = (submersion / 0.8).clamp(0.25, 1.0);
    ((6.0 + impact_speed * 4.5 + submersion * 8.0) * depth_gain).clamp(4.0, 46.0) as usize
}

// ── malha do anel ───────────────────────────────────────────────────────

/// Unit ring (outer radius 1) whose vertex alpha peaks on the crest — the
/// entity's `Transform.scale` is what animates the radius, so one mesh serves
/// every ripple.
pub fn ripple_mesh() -> Mesh {
    // Três anéis: interior transparente → crista opaca → exterior transparente.
    let rings: [(f32, f32); 3] = [(0.70, 0.0), (0.88, 1.0), (1.0, 0.0)];
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(rings.len() * RIPPLE_SEGMENTS);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(rings.len() * RIPPLE_SEGMENTS);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(rings.len() * RIPPLE_SEGMENTS);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(rings.len() * RIPPLE_SEGMENTS);
    for (radius, alpha) in rings {
        for i in 0..RIPPLE_SEGMENTS {
            let theta = i as f32 / RIPPLE_SEGMENTS as f32 * std::f32::consts::TAU;
            positions.push([theta.cos() * radius, 0.0, theta.sin() * radius]);
            normals.push([0.0, 1.0, 0.0]);
            uvs.push([theta / std::f32::consts::TAU, radius]);
            colors.push([1.0, 1.0, 1.0, alpha]);
        }
    }
    let seg = RIPPLE_SEGMENTS as u32;
    let mut indices: Vec<u32> = Vec::with_capacity(RIPPLE_SEGMENTS * 12);
    for band in 0..2u32 {
        let a0 = band * seg;
        let b0 = (band + 1) * seg;
        for i in 0..seg {
            let j = (i + 1) % seg;
            indices.extend_from_slice(&[a0 + i, a0 + j, b0 + i, a0 + j, b0 + j, b0 + i]);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn burst_spec(preset: &str) -> ParticleSpec {
    ParticleSpec {
        preset: preset.to_string(),
        emission_rate: None,
        life: None,
        speed: None,
        size: None,
        color: None,
        shape_radius: None,
        looping: false,
        world_space: false,
    }
}

fn burst_spec_with(
    preset: &str,
    life: Option<(f32, f32)>,
    speed: Option<(f32, f32)>,
    size: Option<(f32, f32)>,
    shape_radius: Option<f32>,
) -> ParticleSpec {
    ParticleSpec {
        life,
        speed,
        size,
        shape_radius,
        ..burst_spec(preset)
    }
}

fn spawn_depth_entry_bursts(
    commands: &mut Commands,
    at: Vec3,
    impact_speed: f32,
    effective_depth: f32,
) {
    let count = splash_count(impact_speed, effective_depth);
    match classify_depth_zone(effective_depth) {
        WaterDepthZone::Shallow => {
            spawn_burst(
                commands,
                &burst_spec_with(
                    "splash",
                    Some((0.25, 0.50)),
                    Some((1.4, 3.0)),
                    Some((0.07, 0.16)),
                    Some(0.22),
                ),
                at,
                (count * 3 / 4).max(4),
            );
            spawn_burst(
                commands,
                &burst_spec_with("rain_ripple", None, None, None, Some(0.32)),
                at + Vec3::Y * 0.02,
                (count / 2).clamp(3, 8),
            );
        }
        WaterDepthZone::Wading => {
            spawn_burst(commands, &burst_spec("splash"), at, count);
            if impact_speed > 3.2 {
                spawn_burst(
                    commands,
                    &burst_spec("wade"),
                    at + Vec3::Y * 0.04,
                    (count / 2).clamp(4, 14),
                );
            }
            spawn_burst(
                commands,
                &burst_spec_with(
                    "foam",
                    Some((0.55, 1.05)),
                    Some((0.05, 0.22)),
                    Some((0.14, 0.28)),
                    Some(0.34),
                ),
                at + Vec3::Y * 0.02,
                (count / 3).clamp(3, 10),
            );
        }
        WaterDepthZone::Deep => {
            spawn_burst(
                commands,
                &burst_spec_with(
                    "splash",
                    None,
                    Some((2.8, 6.4)),
                    Some((0.12, 0.28)),
                    Some(0.45),
                ),
                at,
                (count + 4).min(48),
            );
            spawn_burst(
                commands,
                &burst_spec_with(
                    "foam",
                    Some((0.80, 1.50)),
                    Some((0.06, 0.25)),
                    Some((0.18, 0.36)),
                    Some(0.48),
                ),
                at + Vec3::Y * 0.03,
                (count / 2).clamp(6, 16),
            );
            if impact_speed > 5.0 {
                spawn_burst(
                    commands,
                    &burst_spec_with(
                        "mist",
                        Some((0.65, 1.20)),
                        Some((0.18, 0.55)),
                        Some((0.26, 0.52)),
                        Some(0.42),
                    ),
                    at + Vec3::Y * 0.08,
                    (count / 3).clamp(4, 12),
                );
            }
        }
    }
}

fn spawn_depth_wake_bursts(
    commands: &mut Commands,
    at: Vec3,
    planar_speed: f32,
    effective_depth: f32,
) {
    let base_count = (4.0 + planar_speed * 2.0) as usize;
    match classify_depth_zone(effective_depth) {
        WaterDepthZone::Shallow => {
            spawn_burst(
                commands,
                &burst_spec_with(
                    "wade",
                    Some((0.20, 0.40)),
                    Some((0.6, 1.5)),
                    Some((0.05, 0.11)),
                    Some(0.18),
                ),
                at,
                base_count.clamp(3, 8),
            );
        }
        WaterDepthZone::Wading => {
            spawn_burst(commands, &burst_spec("wade"), at, base_count.clamp(4, 14));
            spawn_burst(
                commands,
                &burst_spec_with(
                    "foam",
                    Some((0.50, 0.95)),
                    Some((0.04, 0.16)),
                    Some((0.12, 0.24)),
                    Some(0.26),
                ),
                at + Vec3::Y * 0.02,
                (base_count / 2).clamp(2, 6),
            );
        }
        WaterDepthZone::Deep => {
            spawn_burst(
                commands,
                &burst_spec_with(
                    "foam",
                    Some((0.65, 1.30)),
                    Some((0.05, 0.18)),
                    Some((0.15, 0.30)),
                    Some(0.28),
                ),
                at + Vec3::Y * 0.02,
                (base_count * 2 / 3).clamp(3, 8),
            );
            spawn_burst(
                commands,
                &burst_spec_with(
                    "wade",
                    None,
                    Some((0.8, 1.8)),
                    Some((0.06, 0.13)),
                    Some(0.25),
                ),
                at,
                (base_count / 2).clamp(2, 6),
            );
        }
    }
}

// ── sistemas ────────────────────────────────────────────────────────────

fn setup_ripple_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(RippleAssets {
        mesh: meshes.add(ripple_mesh()),
        materials: Vec::new(),
    });
}

/// Caixa XZ CONSERVADORA de um corpo de água — a broad-phase das queries de
/// água por nadador.
///
/// O `WaterBody` de um RIO resolve TODAS as suas queries
/// (`contains`/`is_near`/`surface_y_at`/`distance_to_waterline`) com
/// `nearest_on_path` sobre o vetor inteiro de estações — O(estações) cada
/// uma. O `water_contact_system` perguntava a CADA nadador por TODOS os
/// corpos: com as ~170 criaturas do `simple-rpg` são dezenas de milhares de
/// testes de segmento por frame, para 99 % de bichos que estão em terra firme.
///
/// Medir a caixa UMA vez por corpo por frame custa `corpos × estações` (≈13 ×
/// estações) e corta as queries ao que interessa. A caixa é deliberadamente
/// GENEROSA — pico do contorno orgânico × reach do espelho nos lagos, maior
/// meia-largura de estação nos rios — para nunca excluir um ponto que o corpo
/// molhe de facto.
fn body_bounds(body: &super::water::WaterBody) -> (Vec2, Vec2) {
    match body.kind {
        super::WaterKind::Lake => {
            let reach = body
                .carve_radius
                .max(body.radius * super::water::CONTOUR_PEAK * body.mirror_reach);
            (body.at - Vec2::splat(reach), body.at + Vec2::splat(reach))
        }
        super::WaterKind::River => {
            if body.stations.is_empty() {
                return (body.at, body.at);
            }
            // Quadrado do CASCO (`WaterBody::hull` = distância máxima das
            // estações ao centroide + maior meia-largura) + carve: contém a
            // caixa de estações expandida e custa O(1) por corpo por frame —
            // a versão antiga voltava a caminhar TODAS as estações de TODOS
            // os rios TODOS os frames para medir a mesma caixa imutável.
            // Corpo construído à mão sem casco (`hull <= 0`): cai no cálculo
            // exato em vez de encolher a caixa.
            if body.hull > 0.0 {
                let reach = body.hull + body.carve_radius;
                (body.at - Vec2::splat(reach), body.at + Vec2::splat(reach))
            } else {
                let half = body
                    .half_width
                    .iter()
                    .copied()
                    .fold(body.carve_radius, f32::max);
                let mut min = body.stations[0];
                let mut max = body.stations[0];
                for station in &body.stations {
                    min = min.min(*station);
                    max = max.max(*station);
                }
                (min - Vec2::splat(half), max + Vec2::splat(half))
            }
        }
    }
}

/// Marca herói e criaturas como candidatos a salpicar (idempotente).
#[allow(clippy::type_complexity)]
fn tag_swimmers(
    mut commands: Commands,
    hero: Query<
        Entity,
        (
            With<crate::player::Player>,
            Without<WaterContact>,
            With<Transform>,
        ),
    >,
    creatures: Query<
        Entity,
        (
            With<crate::ai::EnemyCreature>,
            Without<WaterContact>,
            With<Transform>,
        ),
    >,
) {
    for entity in hero.iter().chain(creatures.iter()) {
        commands.entity(entity).insert(WaterContact::default());
    }
}

/// Deteta entrada/saída/vadeagem e emite impulsos físicos de onda no solver
/// GPU (`WaterWaveWorld`) além de partículas de salpico/espuma.
#[allow(clippy::too_many_arguments)]
fn water_contact_system(
    time: Res<Time>,
    terrain: Option<Res<TerrainRuntime>>,
    mut waves: Option<ResMut<WaterWaveWorld>>,
    mut commands: Commands,
    mut swimmers: Query<(&GlobalTransform, &mut WaterContact, Option<&WaterKinematics>)>,
    mut bounds: Local<Vec<(Vec2, Vec2)>>,
) {
    let Some(terrain) = terrain else {
        return;
    };
    let dt = time.delta_secs().max(1e-4);
    // Broad-phase: uma caixa por corpo por frame (`body_bounds`), para a query
    // por nadador não pagar o `nearest_on_path` do rio em todos eles.
    bounds.clear();
    bounds.extend(terrain.water.iter().map(body_bounds));
    for (transform, mut contact, kinematics) in &mut swimmers {
        let pos = transform.translation();
        let previous = contact.last_pos.unwrap_or(pos);
        contact.last_pos = Some(pos);
        let step = pos - previous;
        let step_planar_vel = Vec2::new(step.x, step.z) / dt;
        let step_fall_speed = (-step.y / dt).max(0.0);

        let (planar_vel, fall_speed, body_radius) = if let Some(kin) = kinematics {
            let k_planar = Vec2::new(kin.velocity.x, kin.velocity.z);
            let k_vel = if k_planar.length_squared() > step_planar_vel.length_squared() {
                k_planar
            } else {
                step_planar_vel
            };
            (
                k_vel,
                (-kin.velocity.y).max(step_fall_speed),
                if kin.body_radius > 0.05 {
                    kin.body_radius
                } else {
                    DEFAULT_SWIMMER_RADIUS
                },
            )
        } else {
            (step_planar_vel, step_fall_speed, DEFAULT_SWIMMER_RADIUS)
        };
        let planar_speed = planar_vel.length();

        let p = Vec2::new(pos.x, pos.z);
        let surface = terrain
            .water
            .iter()
            .zip(bounds.iter())
            .filter(|(_, (min, max))| p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y)
            .filter_map(|(body, _)| body.surface_y_at(p))
            .fold(None::<f32>, |acc, y| Some(acc.map_or(y, |a| a.max(y))));
        let Some(surface_y) = surface else {
            if contact.wet {
                contact.wet = false;
                contact.since_ripple = 0.0;
            }
            continue;
        };
        // `pos.y` é a base da entidade (o pivô dos GLB está nos pés).
        let submersion = surface_y - pos.y;
        let bed_depth = if terrain.in_field(pos.x, pos.z) {
            (surface_y - terrain.sample(pos.x, pos.z)).max(0.0)
        } else {
            0.0
        };
        let effective_depth = bed_depth.max(submersion);
        let zone = classify_depth_zone(effective_depth);
        let wet = if contact.wet {
            submersion > WADE_EXIT_DEPTH
        } else {
            submersion > WADE_ENTER_DEPTH
        };

        if wet && !contact.wet {
            let hit_xz = swept_entry_xz(previous, pos, surface_y, WADE_ENTER_DEPTH);
            let at = Vec3::new(hit_xz.x, surface_y, hit_xz.y);
            let imp = compute_entry_impulse(fall_speed, planar_speed, body_radius, effective_depth);
            if let Some(ref mut wave_world) = waves {
                wave_world.push_impulse(WaveImpulseRequest {
                    pos: hit_xz,
                    vel: planar_vel,
                    radius: imp.radius,
                    strength: imp.strength,
                    foam: imp.foam,
                    is_wake: false,
                });
            }
            spawn_depth_entry_bursts(&mut commands, at, imp.total_speed, effective_depth);
            contact.since_ripple = 0.0;
        } else if !wet && contact.wet {
            let at = Vec3::new(pos.x, surface_y, pos.z);
            let exit_scale = match zone {
                WaterDepthZone::Shallow => 0.72,
                WaterDepthZone::Wading => 1.0,
                WaterDepthZone::Deep => 1.15,
            };
            if let Some(ref mut wave_world) = waves {
                wave_world.push_impulse(WaveImpulseRequest {
                    pos: p,
                    vel: planar_vel,
                    radius: ((body_radius * 1.2) * exit_scale).max(0.40),
                    strength: ((0.06 + planar_speed * 0.012) * exit_scale).min(0.24),
                    foam: 0.24 * exit_scale,
                    is_wake: false,
                });
            }
            match zone {
                WaterDepthZone::Shallow => {
                    spawn_burst(&mut commands, &burst_spec("wade"), at, 6);
                }
                _ => {
                    spawn_burst(&mut commands, &burst_spec("splash"), at, 10);
                }
            }
        } else if wet && planar_speed > WAKE_MIN_SPEED {
            let wake = compute_wake_impulse(planar_speed, body_radius, effective_depth);
            if let Some(ref mut wave_world) = waves {
                wave_world.push_impulse(WaveImpulseRequest {
                    pos: p,
                    vel: planar_vel,
                    radius: wake.radius,
                    strength: wake.strength,
                    foam: wake.foam,
                    is_wake: true,
                });
            }
            let ripple_interval = match zone {
                WaterDepthZone::Shallow => RIPPLE_INTERVAL * 1.15,
                WaterDepthZone::Wading => RIPPLE_INTERVAL * 0.80,
                WaterDepthZone::Deep => RIPPLE_INTERVAL * 0.92,
            };
            contact.since_ripple += planar_speed * dt;
            if contact.since_ripple >= ripple_interval {
                contact.since_ripple = 0.0;
                let at = Vec3::new(pos.x, surface_y, pos.z);
                spawn_depth_wake_bursts(&mut commands, at, planar_speed, effective_depth);
            }
        }
        contact.wet = wet;
    }
}

/// O material do anel: constante (o fade anima-se no alpha DESTE material,
/// por isso é um por anel — reciclado pelo pool de [`RippleAssets`]).
fn ripple_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgba(0.92, 0.97, 1.0, 0.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        // A onda e a lâmina de água são ambas transparentes: sem o bias a
        // ordenação por distância troca-as consoante o ângulo da câmara e o
        // anel pisca ao passar por cima do espelho.
        depth_bias: 8.0,
        ..StandardMaterial::default()
    }
}

#[allow(dead_code)]
fn spawn_ripple(
    commands: &mut Commands,
    assets: &mut RippleAssets,
    materials: &mut Assets<StandardMaterial>,
    at: Vec3,
    scale: f32,
) {
    let material = assets
        .materials
        .pop()
        .unwrap_or_else(|| materials.add(ripple_material()));
    commands.spawn((
        Name::new("fx:ripple"),
        Transform::from_translation(Vec3::new(at.x, at.y + RIPPLE_LIFT, at.z))
            .with_scale(Vec3::splat(RIPPLE_START_RADIUS)),
        Visibility::Inherited,
        Mesh3d(assets.mesh.clone()),
        MeshMaterial3d(material),
        NotShadowCaster,
        Ripple {
            age: 0.0,
            life: RIPPLE_LIFE,
            start_radius: RIPPLE_START_RADIUS,
            end_radius: RIPPLE_END_RADIUS * scale,
        },
    ));
}

/// Expande, desvanece e despawna os anéis.
fn ripple_system(
    time: Res<Time>,
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut assets: ResMut<RippleAssets>,
    mut ripples: Query<(
        Entity,
        &mut Transform,
        &mut Ripple,
        &MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut ripple, material) in &mut ripples {
        ripple.age += dt;
        if ripple.age >= ripple.life {
            // O material volta ao pool: o próximo anel herda-o (o alpha é
            // reescrito a partir da idade, o estado anterior não importa).
            assets.materials.push(material.0.clone());
            commands.entity(entity).try_despawn();
            continue;
        }
        let radius = ripple_radius(
            ripple.age,
            ripple.life,
            ripple.start_radius,
            ripple.end_radius,
        );
        transform.scale = Vec3::splat(radius);
        if let Some(mut material) = materials.get_mut(&material.0) {
            material
                .base_color
                .set_alpha(ripple_alpha(ripple.age, ripple.life) * 0.45);
        }
    }
}

/// Registo dos FX de água (chamado pelo arranque da engine).
pub struct WaterFxPlugin;

impl bevy::app::Plugin for WaterFxPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_systems(bevy::app::Startup, setup_ripple_assets)
            .add_systems(
                bevy::app::Update,
                (tag_swimmers, water_contact_system, ripple_system),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ripple_radius_spans_start_to_end_and_eases() {
        let (s, e, life) = (0.35_f32, 2.6_f32, 1.5_f32);
        assert!((ripple_radius(0.0, life, s, e) - s).abs() < 1e-5);
        assert!((ripple_radius(life, life, s, e) - e).abs() < 1e-5);
        assert!(
            (ripple_radius(life * 2.0, life, s, e) - e).abs() < 1e-5,
            "clamped"
        );
        // Ease-out: metade da vida já cobriu mais de metade do raio.
        let half = ripple_radius(life * 0.5, life, s, e);
        assert!(half > s + (e - s) * 0.5, "eases out: {half}");
        // Monótono.
        let mut previous = f32::NEG_INFINITY;
        for i in 0..=20 {
            let r = ripple_radius(life * i as f32 / 20.0, life, s, e);
            assert!(r >= previous, "monotonic at {i}: {r} < {previous}");
            previous = r;
        }
    }

    #[test]
    fn test_ripple_alpha_rises_then_fades_to_zero() {
        let life = 1.5_f32;
        assert!(ripple_alpha(0.0, life) < 1e-6, "born invisible");
        assert!(ripple_alpha(life, life) < 1e-6, "dies invisible");
        assert!(ripple_alpha(life * 2.0, life) < 1e-6, "clamped");
        let peak = ripple_alpha(life * 0.12, life);
        assert!(peak > 0.5, "reaches full strength after the rise: {peak}");
        assert!(
            ripple_alpha(life * 0.8, life) < ripple_alpha(life * 0.4, life),
            "fades out"
        );
    }

    #[test]
    fn test_splash_count_scales_with_impact_depth_and_clamps() {
        assert!(splash_count(0.0, 1.0) >= 4, "walking in still splashes");
        assert!(
            splash_count(4.0, 1.0) > splash_count(1.0, 1.0),
            "impact scales"
        );
        assert!(
            splash_count(2.0, 0.25) < splash_count(2.0, 1.5),
            "ankle-deep is a plop, waist-deep a crown"
        );
        assert!(splash_count(1000.0, 10.0) <= 46, "clamped");
        assert!(splash_count(0.0, 0.0) >= 4, "floor");
    }

    /// O anel tem os três aros e a crista é o único opaco — é isso que dá a
    /// linha de onda em vez de um disco.
    #[test]
    fn test_ripple_mesh_is_a_faded_ring() {
        let mesh = ripple_mesh();
        assert_eq!(mesh.count_vertices(), RIPPLE_SEGMENTS * 3);
        let colors = match mesh
            .attribute(Mesh::ATTRIBUTE_COLOR)
            .expect("vertex colors")
        {
            bevy::render::mesh::VertexAttributeValues::Float32x4(v) => v,
            other => panic!("expected rgba colors, got {other:?}"),
        };
        assert!(colors[0][3] < 1e-6, "inner ring transparent");
        assert!(
            (colors[RIPPLE_SEGMENTS][3] - 1.0).abs() < 1e-6,
            "crest opaque"
        );
        assert!(
            colors[RIPPLE_SEGMENTS * 2][3] < 1e-6,
            "outer ring transparent"
        );
        let indices = mesh.indices().expect("indexed");
        assert_eq!(indices.len(), RIPPLE_SEGMENTS * 12, "two bands");
        assert!(
            indices.iter().all(|i| i < RIPPLE_SEGMENTS * 3),
            "indices in range"
        );
    }

    #[test]
    fn test_swept_entry_xz_interpolates_crossing_and_impulses_scale() {
        let hit = swept_entry_xz(
            Vec3::new(10.0, 4.0, 2.0),
            Vec3::new(14.0, 0.0, 6.0),
            2.22,
            WADE_ENTER_DEPTH,
        );
        // trigger_y = 2.0 -> midpoint t = 0.5 -> (12.0, 4.0)
        assert!((hit - Vec2::new(12.0, 4.0)).length() < 1e-4, "hit={hit:?}");

        let step_in = compute_entry_impulse(0.2, 3.0, 0.42, 0.35);
        let jump_in = compute_entry_impulse(11.0, 4.0, 0.42, 1.2);
        assert!(jump_in.strength > step_in.strength * 2.0);
        assert!(jump_in.radius > step_in.radius);
        assert!(jump_in.foam > step_in.foam);

        let slow_wake = compute_wake_impulse(1.0, 0.42, 0.5);
        let fast_wake = compute_wake_impulse(5.5, 0.42, 0.8);
        assert!(fast_wake.strength > slow_wake.strength);
        assert!(fast_wake.foam > slow_wake.foam);
    }

    #[test]
    fn test_water_depth_zones_stratify_entry_and_wake_impulses() {
        assert_eq!(classify_depth_zone(0.18), WaterDepthZone::Shallow);
        assert_eq!(classify_depth_zone(0.55), WaterDepthZone::Wading);
        assert_eq!(classify_depth_zone(1.35), WaterDepthZone::Deep);

        let shallow_entry = compute_entry_impulse(6.0, 3.0, 0.42, 0.25);
        let wade_entry = compute_entry_impulse(6.0, 3.0, 0.42, 0.65);
        let deep_entry = compute_entry_impulse(6.0, 3.0, 0.42, 1.35);
        assert!(shallow_entry.radius < wade_entry.radius);
        assert!(wade_entry.radius < deep_entry.radius);
        assert!(shallow_entry.strength < deep_entry.strength);

        let shallow_wake = compute_wake_impulse(3.8, 0.42, 0.25);
        let wade_wake = compute_wake_impulse(3.8, 0.42, 0.60);
        let swim_wake = compute_wake_impulse(3.8, 0.42, 1.20);
        assert!(shallow_wake.strength < wade_wake.strength);
        assert!(swim_wake.radius > shallow_wake.radius);
    }
}
