//! Enemy AI — deterministic wander + player chase over the ground source
//! the app plugs in ([`crate::ground::Ground`]).
//!
//! Creatures carry [`EnemyCreature`] (authored stats + current state); the
//! patrol bookkeeping lives in [`WanderState`], auto-inserted by [`enemy_ai`]
//! on the first tick. Heights come from the ground sampler
//! ([`crate::ground::GroundQuery::surface_below`]) every tick (no physics
//! bodies) and facing reuses the hero's +Z-forward convention via
//! [`crate::locomotion::facing_rotation`].
//!
//! Respawn é regra do JOGO: quem quiser trazer uma criatura de volta spawna-a
//! com `viber.spawn_prototype` ou pelo spawner. A engine não mantém fila de
//! respawn nem política de distância ao player.
//!
//! Wiring: [`AiPlugin`] (serviço + FSM simples), só [`AiServicePlugin`] (quem
//! traz o seu comportamento) ou [`SimpleAiPlugin`] sobre o serviço.

#[cfg(test)]
use bevy::app::PostUpdate;

use bevy::math::Vec3;
use bevy::prelude::*;

use bevy_ext_core::{GROUND_PROBE, InteriorSceneConfig, Player, Rng, SfxEvent};

use crate::corpse::Corpse;
use crate::ground::Ground;
use crate::locomotion::{LocomotionProfile, facing_rotation, facing_slerp_factor};
use crate::profiler::{Group, timed};
use crate::senses::sense_ai;
use crate::vitals::Health;

/// Wander speed as a fraction of the authored chase speed.
pub const WANDER_SPEED_FRACTION: f32 = 0.4;
/// Chase keeps aggro until this multiple of `aggro_radius` (hysteresis band).
pub const DEAGGRO_HYSTERESIS: f32 = 1.8;
/// Seconds a wanderer keeps the same patrol target before picking a new one.
pub const WANDER_RETARGET_SECS: f32 = 4.0;
/// Patrol targets are picked inside this radius around `home`.
pub const WANDER_RADIUS: f32 = 8.0;
/// "Arrived" distance for a patrol target (triggers an early retarget).
pub const WANDER_ARRIVE_DIST: f32 = 0.5;

/// Below this planar speed (m/s) a character counts as standing still, and an
/// explicit look-at (`viber.face_player`) is allowed to own the facing.
///
/// Above it the body always points where it is *going*. This one rule is what
/// kills the "anda de lado": the scripts call `face_player()` and
/// `move_towards(patrol_point)` in the same frame, and the look-at used to win
/// unconditionally — body aimed at the hero, translation going sideways.
pub const FACING_MOVE_SPEED: f32 = 0.25;

/// Velocity below which a decelerating character is simply stopped, so it does
/// not creep forever on a vanishing remainder.
pub const STOP_EPSILON: f32 = 0.02;

// Contratos movidos ao `bevy_ext_core` (com o consumidor `bevy_ext_nav`, cuja
// ponte lê e escreve O MESMO componente que o FSM e os scripts produzem); os
// TypePaths `viber::ai` ficam congelados no core.
pub use bevy_ext_core::{AiLocomotion, AiLocomotionSystems, EnemyCreature, EnemyState, enemy_next_state};

/// Patrol bookkeeping for one wanderer, auto-inserted by [`enemy_ai`].
#[derive(Debug, Clone, Component, Default, Reflect)]
#[reflect(Component, Default)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::ai"]
pub struct WanderState {
    /// Current patrol target (XZ).
    pub target: Vec2,
    /// `Time.elapsed_secs()` when a new target must be picked.
    pub next_pick_at: f32,
    /// Retarget count — feeds the deterministic per-pick seed.
    pub picks: u64,
}

/// Deterministic seed for one retarget of one entity: a hash of the entity
/// index and the pick counter (which advances with arrival/time triggers).
pub fn enemy_seed(entity_index: u32, picks: u64) -> u64 {
    (entity_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ picks.wrapping_mul(0xBF58_476D_1CE4_E5B9)
}

/// Patrol target: uniform point in the disc `home ± radius`, via the shared
/// SplitMix64 RNG ([`Rng`]) — same seed, same target.
pub fn wander_target(home: Vec2, radius: f32, rng_seed: u64) -> Vec2 {
    let mut rng = Rng::new(rng_seed);
    home + rng.unit_disc() * radius
}

/// Clamp an XZ position to the world disc of radius `limit` (Y passes
/// through — heights come from the ground sampler, not the border).
pub fn clamp_to_world(pos: Vec3, limit: f32) -> Vec3 {
    let dist_sq = pos.x * pos.x + pos.z * pos.z;
    if dist_sq > limit * limit {
        let scale = limit / dist_sq.sqrt();
        Vec3::new(pos.x * scale, pos.y, pos.z * scale)
    } else {
        pos
    }
}

/// Criaturas sem script: visão, audição e a FSM Alert/Chase/Search.
/// Sem estímulo ficam no sítio (não há perseguição por distância plana).
pub fn enemy_ai(world: &mut World) {
    sense_ai(world);
}

/// Que IA o host regista — o `AiMode` do bootstrap da engine (`app.rs`).
///
/// O [`Default`] é [`Self::Full`] (o de sempre); [`Self::Service`] é para
/// quem traz o seu próprio comportamento (scripts Luau, regras em Rust) e
/// só quer os sentidos, os vitais e o consumidor do [`AiLocomotion`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AiMode {
    /// Nada: nem sentidos nem a FSM simples.
    Off,
    /// Só o serviço ([`AiServicePlugin`]): sentidos, vitais das criaturas e
    /// o consumidor do [`AiLocomotion`]. O comportamento é de quem monta a
    /// App — a FSM simples da engine não corre.
    Service,
    /// Serviço + FSM simples ([`AiPlugin`]) — o de sempre.
    #[default]
    Full,
}

impl AiMode {
    /// Regista na App os plugins do modo (não duplica mensagens nem sistemas
    /// partilhados).
    pub fn apply(self, app: &mut App) {
        match self {
            Self::Full => {
                app.add_plugins(AiPlugin);
            }
            Self::Service => {
                app.add_plugins(AiServicePlugin);
            }
            Self::Off => {}
        }
    }
}

fn is_character_descendant(
    mut entity: Entity,
    mover: Entity,
    parents: &Query<&ChildOf>,
    character_roots: &Query<(), Or<(With<AiLocomotion>, With<Player>, With<Corpse>)>>,
) -> bool {
    for _ in 0..8 {
        if entity == mover {
            return true;
        }
        let Ok(parent) = parents.get(entity) else {
            return false;
        };
        let p = parent.parent();
        if p == mover || character_roots.contains(p) {
            return true;
        }
        entity = p;
    }
    false
}

/// The single consumer of [`AiLocomotion`]: ramps velocity, steps the
/// transform, snaps Y to the carved surface and turns the yaw.
///
/// Runs in `PostUpdate` before `TransformSystems::Propagate`, so every producer
/// in `Update` (the FSM, the Luau command queue) has already had its say and
/// the movement lands in the same frame's `GlobalTransform`.
///
/// Three things it does that the old direct-write path did not:
///
/// * **Acceleration.** Velocity chases the desire at `profile.accel` instead of
///   jumping to it. The animation driver measures displacement through a
///   `SPEED_SMOOTH_TAU` low-pass; a step function into that filter is what made
///   the clip lag behind the body on every start and stop.
/// * **Turn rate.** Yaw slerps toward the target at `profile.turn_rate`
///   (reusing the hero's [`facing_slerp_factor`]) instead of
///   snapping. A wander retarget no longer spins the creature in one frame.
/// * **Facing follows motion.** Above [`FACING_MOVE_SPEED`] the body points
///   along its own velocity; an explicit look-at only wins when the character
///   is effectively stopped.
#[allow(clippy::type_complexity)]
pub fn apply_ai_locomotion(
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    interior: Option<Res<InteriorSceneConfig>>,
    obstacles: Query<
        (
            Entity,
            &GlobalTransform,
            &avian3d::prelude::Collider,
            Option<&avian3d::prelude::RigidBody>,
        ),
        (
            Without<bevy_ext_core::VoxelCollider>,
            Without<AiLocomotion>,
            Without<Player>,
            Without<avian3d::prelude::Sensor>,
            Without<bevy_ext_core::RagdollBodyOf>,
            Without<bevy_ext_core::CharacterBodyCollider>,
        ),
    >,
    parents: Query<&ChildOf>,
    character_roots: Query<(), Or<(With<AiLocomotion>, With<Player>, With<Corpse>)>>,
    mut movers: Query<
        (
            Entity,
            &mut Transform,
            &GlobalTransform,
            &mut AiLocomotion,
            Option<&LocomotionProfile>,
        ),
        Without<Corpse>,
    >,
) {
    const AI_BODY_RADIUS: f32 = 0.35;
    const AI_WAIST_HEIGHT: f32 = 0.45;
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let source = ground.as_deref().and_then(Ground::get);
    let world_limit = source.and_then(|source| source.world_radius());
    for (mover_entity, mut transform, global, mut loco, profile) in &mut movers {
        let profile = profile.copied().unwrap_or_default();
        // O passo e o terreno são MUNDO; o `Transform` é LOCAL. `offset` é o
        // que o pai acrescenta — um NPC dentro do grupo `city` tem local y=0 e
        // world y≈38,7. Grupos de cena são translações puras; com um pai
        // rodado ou escalado isto seria uma aproximação (a mesma que o antigo
        // aplicador de `MoveBy` fazia).
        let offset = global.translation() - transform.translation;
        // Nobody drove this frame: coast to a stop rather than hold the last
        // velocity forever (a script whose `on_update` was skipped by the AI
        // LOD must not keep walking).
        if !loco.is_driven() {
            loco.halt();
        }
        let delta = loco.desired() - loco.velocity;
        let max_change = profile.accel * dt;
        loco.velocity += delta.clamp_length_max(max_change);
        if loco.desired() == Vec2::ZERO && loco.velocity.length() < STOP_EPSILON {
            loco.velocity = Vec2::ZERO;
        }

        let step = loco.velocity * dt;
        if step.length_squared() > 1e-12 {
            let world = transform.translation + offset;
            let mut x = world.x + step.x;
            let mut z = world.z + step.y;
            for (obs_entity, obs_global, collider, rb) in &obstacles {
                // Corpos dinâmicos/cinemáticos (ex.: membros `PuppetBody` ou
                // props empurráveis) não são paredes estáticas — empurrar um
                // NPC 0,7 m/frame a partir deles gera colisões violentas.
                if matches!(
                    rb,
                    Some(
                        avian3d::prelude::RigidBody::Dynamic
                            | avian3d::prelude::RigidBody::Kinematic
                    )
                ) {
                    continue;
                }
                // Ignora colisores filhos do próprio mover ou de qualquer outro
                // personagem (`AiLocomotion`/`Player`/`Corpse`): caso contrário
                // o NPC encontra a própria cápsula de corpo no primeiro passo e
                // dispara a ~40 m/s pela vila.
                if is_character_descendant(obs_entity, mover_entity, &parents, &character_roots) {
                    continue;
                }
                let obs = obs_global.compute_transform();
                if (obs.translation.x - x).abs() > 16.0
                    || (obs.translation.z - z).abs() > 16.0
                    || (obs.translation.y - world.y).abs() > 16.0
                {
                    continue;
                }
                let probe = Vec3::new(x, world.y + AI_WAIST_HEIGHT, z);
                let (surface_pt, is_inside) =
                    collider.project_point(obs.translation, obs.rotation, probe, false);
                if !is_inside && (surface_pt.y - probe.y).abs() > 0.25 {
                    continue;
                }
                let delta_xz = Vec2::new(x - surface_pt.x, z - surface_pt.z);
                let dist_xz = delta_xz.length();
                if is_inside {
                    let outward = (-delta_xz)
                        .try_normalize()
                        .or_else(|| {
                            Vec2::new(world.x - obs.translation.x, world.z - obs.translation.z)
                                .try_normalize()
                        })
                        .unwrap_or(Vec2::X);
                    let pushed = Vec2::new(surface_pt.x, surface_pt.z) + outward * AI_BODY_RADIUS;
                    x = pushed.x;
                    z = pushed.y;
                } else if dist_xz < AI_BODY_RADIUS {
                    let outward = delta_xz
                        .try_normalize()
                        .or_else(|| {
                            Vec2::new(world.x - surface_pt.x, world.z - surface_pt.z).try_normalize()
                        })
                        .unwrap_or(Vec2::X);
                    let pushed = Vec2::new(surface_pt.x, surface_pt.z) + outward * AI_BODY_RADIUS;
                    x = pushed.x;
                    z = pushed.y;
                }
            }
            // A bolsa de interior vive DECLARADAMENTE fora do disco do mundo
            // (ver `InteriorSceneConfig`): clampar ali teleportava os NPCs da
            // sala para a borda do vale — a mesma exceção (pela posição ATUAL)
            // que o knockback já faz. `limit > 0` cobre um world size
            // degenerado, que colapsava todo o movimento na origem.
            let in_interior = interior
                .as_deref()
                .is_some_and(|scene| scene.contains(world.x, world.z));
            if let Some(limit) = world_limit.filter(|limit| !in_interior && *limit > 0.0) {
                let clamped = clamp_to_world(Vec3::new(x, world.y, z), limit);
                x = clamped.x;
                z = clamped.z;
            }
            // PISO SOB A CRIATURA (Y conhecido → surface_below): sob um
            // cliff/arco fica SOB a rocha em vez de ser teletransportada para o
            // topo do mundo. Sem fonte de chão (testes headless sem bootstrap)
            // o Y fica como está. FORA do heightfield (interior, cena sem
            // terreno) os samplers saturam para a borda — não é o chão da
            // criatura.
            let y = match source {
                Some(source) if source.in_field(x, z) => {
                    // `None` = não há piso sólido na coluna: o destino está
                    // enterrado em rocha que desce até ao fundo (falésia,
                    // encosta). O fallback antigo (o TOPO do mundo)
                    // teleportava a criatura para cima do penhasco a cada
                    // passo contra a parede. Sem collider próprio, recusa-se
                    // o passo — a criatura encosta, como o herói com o
                    // character controller.
                    source.surface_below(x, z, world.y + GROUND_PROBE)
                }
                _ => Some(world.y),
            };
            if let Some(y) = y {
                transform.translation = Vec3::new(x, y, z) - offset;
            }
        }

        let target_dir = if loco.speed() > FACING_MOVE_SPEED {
            Some(loco.velocity.normalize())
        } else {
            loco.facing()
        };
        if let Some(dir) = target_dir {
            let target = facing_rotation(Vec3::new(dir.x, 0.0, dir.y));
            let t = facing_slerp_factor(transform.rotation, target, profile.turn_rate, dt);
            transform.rotation = transform.rotation.slerp(target, t);
        }

        // One-frame contract: the desire has been consumed.
        loco.consume();
    }
}

/// HP default para as criaturas da FSM Rust (spawner dinâmico SEM script
/// Luau): nasciam sem `Health` e ficavam imunes ao strike/bomba — e o
/// respawn nunca disparava. Scriptadas recebem vitais via
/// [`ensure_creature_vitals`] (hostis apenas).
#[allow(clippy::type_complexity)]
pub fn ensure_fsm_vitals(
    mut commands: Commands,
    creatures: Query<(Entity, &EnemyCreature), (Without<Health>, Without<Player>)>,
) {
    for (entity, _) in &creatures {
        commands.entity(entity).insert(Health::default());
    }
}

/// Serviço de IA genérico, sem a FSM simples da engine: sentidos
/// ([`crate::senses::SensesPlugin`]), vitais das criaturas e o consumidor
/// único do [`AiLocomotion`].
///
/// É o que quem traz o seu próprio comportamento (scripts Luau, regras em
/// Rust) precisa: os scripts movem as criaturas via `viber.move_towards` e
/// recebem os sentidos, mas o `enemy_ai` da engine não corre.
pub struct AiServicePlugin;

impl Plugin for AiServicePlugin {
    fn build(&self, app: &mut App) {
        // Sem este plugin o `hear_combat` nunca corria (inimigos surdos) e
        // os `NoisePing` dos impactos ficavam para sempre no mundo.
        app.add_plugins(crate::senses::SensesPlugin);
        app.add_message::<SfxEvent>()
            .add_systems(Update, ensure_fsm_vitals)
            // Em [`AiMode::Service`] não há `sense_ai`: sem este dreno o
            // `SenseBus` crescia sem limite. Em `Full` corre depois do
            // `sense_ai`, que já consumiu os eventos do frame.
            .add_systems(
                Update,
                crate::senses::drain_sense_bus
                    .after(crate::senses::hear_combat)
                    .after(sense_ai),
            )
            // O consumidor único do [`AiLocomotion`] corre DEPOIS de todos os
            // produtores do `Update` (a FSM no [`SimpleAiPlugin`], a fila de
            // comandos Luau no host de scripts) e ANTES da propagação de
            // transforms, para o passo do frame chegar ao `GlobalTransform`
            // do mesmo frame.
            .add_systems(
                bevy::app::PostUpdate,
                timed(Group::Ai, apply_ai_locomotion)
                    .in_set(AiLocomotionSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

/// FSM simples da engine (wander/alert/chase/search). Precisa do
/// [`AiServicePlugin`] para os sentidos, os vitais e o movimento.
pub struct SimpleAiPlugin;

impl Plugin for SimpleAiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, timed(Group::Ai, enemy_ai));
    }
}

/// Plugin completo de sempre: serviço + FSM simples.
///
/// [`AiPlugin`] = [`AiServicePlugin`] + [`SimpleAiPlugin`]; não duplica
/// mensagens nem sistemas partilhados.
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((AiServicePlugin, SimpleAiPlugin));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ground::GroundQuery;

    /// Minimal headless app with just the locomotion consumer and a hand-driven
    /// clock. No ground source: the integrator leaves Y alone, which is what we
    /// want when the subject is XZ velocity and yaw.
    fn locomotion_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build().disable::<bevy::time::TimePlugin>())
            .add_plugins(bevy::transform::TransformPlugin);
        app.init_resource::<Time>();
        app.add_systems(
            PostUpdate,
            apply_ai_locomotion.before(bevy::transform::TransformSystems::Propagate),
        );
        app
    }

    fn tick(app: &mut App, millis: u64) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(millis));
        app.update();
    }

    /// Yaw never moves faster than the profile allows. Before this, every
    /// wander retarget span the creature through 180° inside one frame.
    #[test]
    fn test_yaw_never_turns_faster_than_the_profile_allows() {
        let mut app = locomotion_app();
        let profile = LocomotionProfile::authored(1.5, 4.0);
        // Already at speed, heading -Z; the desire is a full 180° reversal.
        let mut loco = AiLocomotion::default();
        loco.velocity = Vec2::new(0.0, -4.0);
        let entity = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 0.0), loco, profile))
            .id();

        let dt = 0.05_f32;
        let mut previous = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .rotation;
        for _ in 0..40 {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(0.0, 4.0));
            tick(&mut app, (dt * 1000.0) as u64);
            let now = app
                .world()
                .entity(entity)
                .get::<Transform>()
                .unwrap()
                .rotation;
            let swept = previous.angle_between(now);
            assert!(
                swept <= profile.turn_rate * dt + 1e-3,
                "yaw swept {swept:.3} rad in one frame, cap is {:.3}",
                profile.turn_rate * dt
            );
            previous = now;
        }
        // And it does get there: a capped turn is still a turn.
        let facing = previous * Vec3::Z;
        assert!(
            facing.z > 0.9,
            "after 2 s the body faces +Z, got {facing:?}"
        );
    }

    /// Velocity ramps at the profile's acceleration instead of jumping to the
    /// desire on the first frame — the step function into the animation's
    /// own low-pass is what made clips lag the body on every start.
    #[test]
    fn test_velocity_ramps_instead_of_jumping_to_the_desire() {
        let mut app = locomotion_app();
        let profile = LocomotionProfile::authored(1.5, 4.0);
        let entity = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                AiLocomotion::default(),
                profile,
            ))
            .id();

        let dt = 0.05_f32;
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<AiLocomotion>()
            .unwrap()
            .drive(Vec2::new(4.0, 0.0));
        tick(&mut app, 50);
        let speed = app
            .world()
            .entity(entity)
            .get::<AiLocomotion>()
            .unwrap()
            .speed();
        assert!(
            (speed - profile.accel * dt).abs() < 1e-3,
            "first frame reaches accel·dt = {:.2} m/s, got {speed:.2}",
            profile.accel * dt
        );

        // Keep driving: it converges on the desire and stays there.
        for _ in 0..20 {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(4.0, 0.0));
            tick(&mut app, 50);
        }
        let speed = app
            .world()
            .entity(entity)
            .get::<AiLocomotion>()
            .unwrap()
            .speed();
        assert!(
            (speed - 4.0).abs() < 1e-3,
            "settles at 4 m/s, got {speed:.2}"
        );

        // Stop driving: it coasts down instead of freezing mid-stride.
        for _ in 0..20 {
            tick(&mut app, 50);
        }
        let speed = app
            .world()
            .entity(entity)
            .get::<AiLocomotion>()
            .unwrap()
            .speed();
        assert_eq!(speed, 0.0, "an undriven character comes to a stop");
    }

    /// The rule that kills "anda de lado": a look-at issued while the body is
    /// moving does not steal the facing. Stopped, it does.
    #[test]
    fn test_a_look_at_only_wins_when_the_body_is_stopped() {
        let mut app = locomotion_app();
        let profile = LocomotionProfile::authored(1.5, 4.0);
        let entity = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                AiLocomotion::default(),
                profile,
            ))
            .id();

        // Walking along +X while being told to look along +Z, every frame —
        // exactly what the scripts do with move_towards + face_player.
        for _ in 0..60 {
            let mut entity_mut = app.world_mut().entity_mut(entity);
            let mut loco = entity_mut.get_mut::<AiLocomotion>().unwrap();
            loco.drive(Vec2::new(3.0, 0.0));
            loco.look_at(Vec2::new(0.0, 1.0));
            tick(&mut app, 50);
        }
        let facing = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .rotation
            * Vec3::Z;
        assert!(
            facing.x > 0.9,
            "a moving body faces where it goes (+X), got {facing:?}"
        );

        // Now stop and keep asking to look along +Z: the look-at takes over.
        for _ in 0..60 {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .look_at(Vec2::new(0.0, 1.0));
            tick(&mut app, 50);
        }
        let facing = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .rotation
            * Vec3::Z;
        assert!(
            facing.z > 0.9,
            "a stopped body honours the look-at (+Z), got {facing:?}"
        );
    }

    /// A producer that knows where it is going says so, and the stated goal is
    /// what the navigation bridge paths to. Without it the only thing a
    /// pathfinder could aim at is a point on the straight line — which is the
    /// line the wall is on.
    #[test]
    fn a_stated_destination_survives_into_the_goal() {
        let mut loco = AiLocomotion::default();
        assert_eq!(loco.goal(), None, "nada pedido, nada declarado");

        loco.drive_to(Vec2::ZERO, Vec2::new(30.0, 0.0), 4.0, 0.05);
        assert_eq!(loco.goal(), Some(Vec2::new(30.0, 0.0)));
        assert!(
            (loco.desired().length() - 4.0).abs() < 1e-4,
            "pede a velocidade pedida"
        );
        assert!(loco.desired().x > 0.0, "aponta ao destino");

        // Arrival: never overshoot, and the goal stays stated so the bridge
        // knows the character is still trying to get there.
        loco.drive_to(Vec2::new(29.95, 0.0), Vec2::new(30.0, 0.0), 4.0, 0.05);
        assert!(
            loco.desired().length() <= 4.0 + 1e-4,
            "a chegada nunca pede mais do que fecha o intervalo"
        );

        // A bare velocity clears it — `viber.move_by` has no destination to
        // state, and inventing one would path to a lie.
        loco.drive(Vec2::new(1.0, 0.0));
        assert_eq!(loco.goal(), None);
    }

    /// A child of a translated group moves in WORLD space and keeps its local
    /// transform consistent — the townsfolk live under `<Group name="city">`.
    #[test]
    fn test_a_parented_character_moves_in_world_space() {
        let mut app = locomotion_app();
        let parent = app
            .world_mut()
            .spawn(Transform::from_xyz(100.0, 5.0, -50.0))
            .id();
        let child = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                AiLocomotion::default(),
                LocomotionProfile::authored(1.5, 4.0),
            ))
            .id();
        app.world_mut().entity_mut(parent).add_child(child);
        app.update(); // propagate once so the child has a GlobalTransform

        for _ in 0..40 {
            app.world_mut()
                .entity_mut(child)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(2.0, 0.0));
            tick(&mut app, 50);
        }
        let local = app
            .world()
            .entity(child)
            .get::<Transform>()
            .unwrap()
            .translation;
        let world = app
            .world()
            .entity(child)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        assert!(
            local.x > 1.0,
            "the child advanced, local x = {:.2}",
            local.x
        );
        assert!(
            (world.x - (100.0 + local.x)).abs() < 1e-3,
            "world x tracks the parent offset: {world:?} vs local {local:?}"
        );
    }

    #[test]
    fn test_default_enemy_creature_values() {
        let enemy = EnemyCreature::default();
        assert_eq!(enemy.speed, 2.0);
        assert_eq!(enemy.aggro_radius, 18.0);
        assert_eq!(enemy.attack_radius, 2.2);
        assert_eq!(enemy.home, None, "home latches to the spawn on first tick");
        assert_eq!(enemy.state, EnemyState::Wander);
    }

    #[test]
    fn test_wander_target_is_deterministic_and_bounded() {
        let home = Vec2::new(40.0, -12.0);
        let a = wander_target(home, WANDER_RADIUS, 7);
        let b = wander_target(home, WANDER_RADIUS, 7);
        assert_eq!(a, b, "same seed → same patrol point");
        let c = wander_target(home, WANDER_RADIUS, 8);
        assert_ne!(a, c, "different seed → different patrol point");
        for target in [a, b, c] {
            assert!(
                (target - home).length() <= WANDER_RADIUS + 1e-3,
                "target {target:?} outside the patrol disc around {home:?}"
            );
        }
    }

    #[test]
    fn test_enemy_next_state_hysteresis() {
        let aggro = 18.0;
        let deaggro = aggro * DEAGGRO_HYSTERESIS; // 32.4
        // Inside aggro: either state engages the chase.
        assert_eq!(
            enemy_next_state(5.0, EnemyState::Wander, aggro, deaggro),
            EnemyState::Chase
        );
        assert_eq!(
            enemy_next_state(5.0, EnemyState::Chase, aggro, deaggro),
            EnemyState::Chase
        );
        // Hysteresis band (aggro..deaggro): state is sticky.
        let mid = (aggro + deaggro) * 0.5;
        assert_eq!(
            enemy_next_state(mid, EnemyState::Wander, aggro, deaggro),
            EnemyState::Wander
        );
        assert_eq!(
            enemy_next_state(mid, EnemyState::Chase, aggro, deaggro),
            EnemyState::Chase
        );
        // Past deaggro the chase drops; wander never engages far away.
        assert_eq!(
            enemy_next_state(60.0, EnemyState::Chase, aggro, deaggro),
            EnemyState::Wander
        );
        assert_eq!(
            enemy_next_state(60.0, EnemyState::Wander, aggro, deaggro),
            EnemyState::Wander
        );
        // No player → infinite distance never aggros.
        assert_eq!(
            enemy_next_state(f32::INFINITY, EnemyState::Wander, aggro, deaggro),
            EnemyState::Wander
        );
    }

    #[test]
    fn test_clamp_to_world_scales_xz_and_keeps_y() {
        let inside = clamp_to_world(Vec3::new(5.0, 3.0, -7.0), 100.0);
        assert_eq!(
            inside,
            Vec3::new(5.0, 3.0, -7.0),
            "inside the disc: untouched"
        );
        let outside = clamp_to_world(Vec3::new(30.0, 2.0, 40.0), 50.0);
        assert_eq!(outside.y, 2.0, "height passes through");
        let clamped_xz = Vec2::new(outside.x, outside.z);
        assert!(
            (clamped_xz.length() - 50.0).abs() < 1e-4,
            "XZ rescaled onto the limit: {clamped_xz:?}"
        );
        assert!(
            Vec2::new(30.0, 40.0).angle_to(clamped_xz).abs() < 1e-4,
            "direction preserved"
        );
        let zeroed = clamp_to_world(Vec3::new(9.0, 1.0, 9.0), 0.0);
        assert_eq!(
            Vec2::new(zeroed.x, zeroed.z),
            Vec2::ZERO,
            "limit 0 zeroes XZ"
        );
    }

    #[test]
    fn test_enemy_seed_varies_per_entity_and_pick() {
        assert_eq!(enemy_seed(3, 2), enemy_seed(3, 2), "deterministic");
        assert_ne!(enemy_seed(3, 2), enemy_seed(4, 2));
        assert_ne!(enemy_seed(3, 2), enemy_seed(3, 3));
    }

    /// Chão plano de teste: um disco de raio fixo a altura 0 — cobre o snap
    /// do Y, o clamp da borda e o "fora do heightfield" sem terreno real.
    struct FlatDisc {
        radius: f32,
    }

    impl GroundQuery for FlatDisc {
        fn world_radius(&self) -> Option<f32> {
            Some(self.radius)
        }

        fn in_field(&self, x: f32, z: f32) -> bool {
            (x * x + z * z).sqrt() <= self.radius
        }

        fn surface_below(&self, _x: f32, _z: f32, _from_y: f32) -> Option<f32> {
            Some(0.0)
        }
    }

    /// Headless end-to-end: [`AiPlugin`] over a flat disc ground, one
    /// wanderer and one player inside the aggro radius. Sem script não há
    /// perseguição: a criatura fica onde nasceu, agarrada ao chão.
    #[test]
    fn test_enemy_ai_chases_player_over_terrain_headless() {
        let mut app = App::new();
        // TimePlugin is disabled: it would overwrite the clock with real
        // (microscopic) deltas every frame; we advance `Time` by hand instead.
        app.add_plugins(MinimalPlugins.build().disable::<bevy::time::TimePlugin>())
            .add_plugins(bevy::transform::TransformPlugin)
            .add_plugins(AiPlugin);
        app.init_resource::<Time>();
        app.insert_resource(Ground::new(FlatDisc { radius: 64.0 }));

        app.world_mut()
            .spawn((Player::default(), Transform::from_xyz(5.0, 0.0, 0.0)));
        app.world_mut()
            .spawn((EnemyCreature::default(), Transform::from_xyz(0.0, 0.0, 0.0)));

        // 20 × 0.25 s = 5 simulated seconds — enough to close 2.8 m at 2 m/s.
        for _ in 0..20 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(250));
            app.update();
        }

        let world = app.world_mut();
        let mut q = world.query::<(Entity, &EnemyCreature, &Transform)>();
        let (entity, enemy, transform) = q
            .iter(world)
            .next()
            .expect("enemy alive with its components");
        assert_eq!(
            enemy.state,
            EnemyState::Wander,
            "sem script não há perseguição"
        );
        let pos = transform.translation;
        assert!(
            pos.xz().distance(Vec2::ZERO) < 0.05,
            "a criatura fica onde nasceu, agora {pos:?}"
        );
        assert!(world.get::<WanderState>(entity).is_none());
    }

    /// Same world, no player: the creature stays a wanderer, moves at the
    /// wander pace and never strays past its patrol radius.
    #[test]
    fn test_enemy_ai_wanders_without_a_player_headless() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build().disable::<bevy::time::TimePlugin>())
            .add_plugins(bevy::transform::TransformPlugin)
            .add_plugins(AiPlugin);
        app.init_resource::<Time>();
        app.insert_resource(Ground::new(FlatDisc { radius: 64.0 }));
        let home = Vec2::new(40.0, -30.0);
        // Enemy B patrols around an in-world home…
        app.world_mut().spawn((
            EnemyCreature {
                home: Some(home),
                ..EnemyCreature::default()
            },
            Transform::from_xyz(home.x, 0.0, home.y),
        ));
        // …enemy A spawns outside the world disc to exercise clamp_to_world:
        // it ends up on the border, which is by design farther than the
        // patrol radius from its home.
        let outside = Vec2::new(200.0, 200.0);
        app.world_mut().spawn((
            EnemyCreature {
                home: Some(outside),
                ..EnemyCreature::default()
            },
            Transform::from_xyz(outside.x, 0.0, outside.y),
        ));

        // 30 × 0.25 s = 7.5 simulated seconds of patrol (two retargets).
        for _ in 0..30 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(250));
            app.update();
        }

        let world = app.world_mut();
        let mut q = world.query::<(&EnemyCreature, &Transform)>();
        let mut border = None;
        let mut patroller = None;
        for (enemy, transform) in q.iter(world) {
            if enemy.home == Some(outside) {
                border = Some((enemy, transform));
            } else {
                patroller = Some((enemy, transform));
            }
        }
        let (border_enemy, border_transform) = border.expect("border enemy alive");
        let (patrol_enemy, patrol_transform) = patroller.expect("patrolling enemy alive");
        assert_eq!(
            border_enemy.state,
            EnemyState::Wander,
            "no player → never chases"
        );
        assert_eq!(
            patrol_enemy.state,
            EnemyState::Wander,
            "no player → never chases"
        );

        assert_eq!(patrol_enemy.home, Some(home));
        let pos = Vec2::new(
            patrol_transform.translation.x,
            patrol_transform.translation.z,
        );
        assert!(
            pos.distance(home) < 0.05,
            "sem script não patrulha: {pos:?}"
        );
        let border_pos = Vec2::new(
            border_transform.translation.x,
            border_transform.translation.z,
        );
        assert!(
            border_pos.distance(outside) < 0.05,
            "sem script não é puxada para o disco: {border_pos:?}"
        );
    }

    #[test]
    fn test_locomotion_slides_around_static_rock_colliders_instead_of_entering() {
        let mut app = locomotion_app();
        let rock_radius = 0.80_f32;
        let rock = avian3d::prelude::Collider::cylinder(rock_radius, 0.80);
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 0.40, 0.0),
            GlobalTransform::from(Transform::from_xyz(0.0, 0.40, 0.0)),
            rock,
        ));
        let enemy = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.15, 0.0, -2.0),
                GlobalTransform::from(Transform::from_xyz(0.15, 0.0, -2.0)),
                AiLocomotion::default(),
                LocomotionProfile::authored(1.0, 4.0),
            ))
            .id();

        for _ in 0..80 {
            app.world_mut()
                .entity_mut(enemy)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(0.0, 4.0));
            tick(&mut app, 25);
            let pos = app.world().entity(enemy).get::<Transform>().unwrap().translation;
            let dist_xz = Vec2::new(pos.x, pos.z).length();
            assert!(
                dist_xz >= rock_radius + 0.34,
                "enemy entered rock collider: pos={pos:?} (dist_xz={dist_xz:.3})"
            );
        }
        let end_pos = app.world().entity(enemy).get::<Transform>().unwrap().translation;
        assert!(
            end_pos.z > 0.5,
            "enemy slid past the rock instead of getting stuck: end_pos={end_pos:?}"
        );
    }

    #[test]
    fn test_locomotion_ignores_own_child_body_collider_and_dynamic_bodies() {
        let mut app = locomotion_app();
        let npc = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 0.0)),
                AiLocomotion::default(),
                LocomotionProfile::authored(1.1, 10.0),
            ))
            .id();
        // Exact `#NpcBody` geometry as a child entity (even without the marker).
        app.world_mut().spawn((
            ChildOf(npc),
            Transform::IDENTITY,
            GlobalTransform::IDENTITY,
            avian3d::prelude::RigidBody::Static,
            avian3d::prelude::Collider::compound(vec![(
                Vec3::Y * 0.85,
                Quat::IDENTITY,
                avian3d::prelude::Collider::capsule(0.32, 0.86),
            )]),
        ));
        // Nearby dynamic body (e.g. a `PuppetBody` limb).
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 0.5, 0.15),
            GlobalTransform::from(Transform::from_xyz(0.0, 0.5, 0.15)),
            avian3d::prelude::RigidBody::Dynamic,
            avian3d::prelude::Collider::capsule(0.12, 0.4),
        ));

        let mut prev = Vec3::ZERO;
        for _ in 0..20 {
            app.world_mut()
                .entity_mut(npc)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(0.0, 1.1));
            tick(&mut app, 16);
            let pos = app.world().entity(npc).get::<Transform>().unwrap().translation;
            let step = (pos - prev).length();
            assert!(
                step < 0.03,
                "NPC must walk smoothly (~0.018 m/frame), not self-eject: step={step:.3}, pos={pos:?}"
            );
            prev = pos;
        }
        assert!(
            prev.z > 0.20 && prev.z < 0.45 && prev.x.abs() < 1e-4,
            "NPC walked straight at ~1.1 m/s for 0.32s: end={prev:?}"
        );
    }

    #[test]
    fn test_locomotion_ignores_character_body_collider_marker() {
        let mut app = locomotion_app();
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 0.0, 0.2),
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 0.2)),
            bevy_ext_core::CharacterBodyCollider,
            avian3d::prelude::RigidBody::Static,
            avian3d::prelude::Collider::compound(vec![(
                Vec3::Y * 0.85,
                Quat::IDENTITY,
                avian3d::prelude::Collider::capsule(0.32, 0.86),
            )]),
        ));
        let npc = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 0.0)),
                AiLocomotion::default(),
                LocomotionProfile::authored(1.1, 10.0),
            ))
            .id();

        app.world_mut()
            .entity_mut(npc)
            .get_mut::<AiLocomotion>()
            .unwrap()
            .drive(Vec2::new(0.0, 1.1));
        tick(&mut app, 16);
        let pos = app.world().entity(npc).get::<Transform>().unwrap().translation;
        assert!(
            pos.length() < 0.03,
            "CharacterBodyCollider must not teleport AI movers: pos={pos:?}"
        );
    }
}
