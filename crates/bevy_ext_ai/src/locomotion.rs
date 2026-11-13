//! Nominais de locomoção por rig e rotação de facing (+Z).
//!
//! [`LocomotionProfile`] vivia no módulo de animação da engine: o serviço de
//! IA é quem o LÊ no integrador (`accel`/`turn_rate`) e a animação é quem o
//! calibra ([`LocomotionProfile::observe`]). Vive no crate de IA enquanto
//! não houver crate de animação.
//!
//! TODO(viber-exit): `speeds()`/`gates()` (os clamps de playback de clip e
//! os limiares idle/walk/run) e os tipos da animação (`MotionSpeeds`,
//! `MotionGates`) ficaram no monólito — esta cópia traz só o que a
//! locomoção precisa. O caminho de reflexão histórico
//! (`viber::animation::LocomotionProfile`) fica intacto.

use bevy::math::{Quat, Vec3};
use bevy::prelude::*;

/// Default yaw rate (rad/s) of a character with no authored profile.
///
/// ~7 rad/s turns 180° in a quarter second: fast enough that a creature still
/// commits to a new target immediately, slow enough that the eye reads a turn
/// instead of a teleported orientation.
pub const DEFAULT_TURN_RATE: f32 = 7.0;

/// Default acceleration (m/s²) toward the desired velocity.
///
/// At 12 m/s² a 4 m/s chase reaches full speed in a third of a second — the
/// ramp is what the animation's own low-pass (`SPEED_SMOOTH_TAU`) was fighting
/// when the AI went from standing to full speed inside one frame.
pub const DEFAULT_ACCEL: f32 = 12.0;

/// Nominais de criatura sem perfil medido: wander ~1.5 m/s, chase ~4 m/s
/// (`MotionSpeeds::CREATURE` da animação).
const CREATURE_WALK: f32 = 1.5;
const CREATURE_RUN: f32 = 4.0;

/// Speed below which a commanded step counts as "not moving" for calibration.
const CALIBRATION_FLOOR: f32 = 0.05;

/// A walk nominal this close to the run nominal means we only ever saw ONE
/// speed, so the pair is synthesised instead of measured.
const CALIBRATION_SPREAD: f32 = 0.85;

/// Nominal walk as a fraction of run, when only one speed was ever observed.
const CALIBRATION_WALK_FRACTION: f32 = 0.45;

/// Per-rig locomotion nominals — what "walking" and "running" mean for THIS
/// character, plus how fast it may turn and accelerate.
///
/// Authoring is optional by design (mission premise "ease over knobs"): the
/// component calibrates itself from the speeds the entity is actually commanded
/// at, through [`Self::observe`]. The Luau scripts pass their speed to
/// `viber.move_towards`, and the Rust FSM has it in
/// `EnemyCreature::speed`, so both paths teach it the truth without a single
/// line of authoring. An explicit `locomotion="walk: 1.1; run: 4.6"` (or
/// `viber.set_locomotion`) latches [`Self::authored`] and stops the
/// calibration.
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::animation"]
pub struct LocomotionProfile {
    /// Nominal walking speed (m/s) of the rig's walk clip.
    pub walk: f32,
    /// Nominal running speed (m/s) of the rig's run clip.
    pub run: f32,
    /// Maximum yaw rate (rad/s). The whole reason a creature stops spinning
    /// 180° in one frame when it picks a new wander target.
    pub turn_rate: f32,
    /// Acceleration (m/s²) toward the desired velocity.
    pub accel: f32,
    /// Nominals came from authoring, not from observation.
    pub authored: bool,
    /// Slowest non-zero speed ever commanded (`INFINITY` until the first one).
    observed_min: f32,
    /// Fastest speed ever commanded.
    observed_max: f32,
}

impl Default for LocomotionProfile {
    fn default() -> Self {
        Self {
            walk: CREATURE_WALK,
            run: CREATURE_RUN,
            turn_rate: DEFAULT_TURN_RATE,
            accel: DEFAULT_ACCEL,
            authored: false,
            observed_min: f32::INFINITY,
            observed_max: 0.0,
        }
    }
}

impl LocomotionProfile {
    /// A profile with nominals the world author (or a script) stated outright.
    pub fn authored(walk: f32, run: f32) -> Self {
        Self {
            walk: walk.max(1e-3),
            run: run.max(walk).max(1e-3),
            authored: true,
            ..Self::default()
        }
    }

    /// Feeds one commanded speed into the calibration.
    ///
    /// Returns `true` when the nominals moved, so callers can skip change
    /// detection noise on the component.
    pub fn observe(&mut self, commanded: f32) -> bool {
        if self.authored || !commanded.is_finite() || commanded <= CALIBRATION_FLOOR {
            return false;
        }
        let min = self.observed_min.min(commanded);
        let max = self.observed_max.max(commanded);
        if min == self.observed_min && max == self.observed_max {
            return false;
        }
        self.observed_min = min;
        self.observed_max = max;
        // Two clearly separate speeds (wander vs chase) ARE the two nominals.
        // One speed only — a townsfolk that never runs — gives the walk
        // nominal, and the run nominal is synthesised above it so the rig keeps
        // a headroom instead of flipping to the run clip on its own walk.
        self.run = if min < max * CALIBRATION_SPREAD {
            self.walk = min;
            max
        } else {
            self.walk = max;
            max / CALIBRATION_WALK_FRACTION
        };
        true
    }

    /// True once the profile has something better than the module defaults.
    pub fn is_calibrated(&self) -> bool {
        self.authored || self.observed_max > 0.0
    }
}

/// Face a movement direction (models from the pipeline face +Z).
// TODO(viber-exit): cópia da função pura `viber::player::facing_rotation`;
// quando o módulo do player sair do monólito, re-exportar de lá em vez de
// duplicar.
pub fn facing_rotation(dir: Vec3) -> Quat {
    Quat::from_rotation_y(dir.x.atan2(dir.z))
}

/// Slerp factor for facing: `rotation_speed · dt` converted to a 0..1 t,
/// mirroring the `calculateSlerpFactor` clamp.
// TODO(viber-exit): cópia da função pura `viber::player::facing_slerp_factor`.
pub fn facing_slerp_factor(current: Quat, target: Quat, rotation_speed: f32, dt: f32) -> f32 {
    let max_radians = rotation_speed * dt;
    let angle = current.angle_between(target);
    if angle <= f32::EPSILON {
        1.0
    } else {
        (max_radians / angle).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Funções puras copiadas de `viber::player` — o golden test do monólito
    /// congela o comportamento (+Z para a frente, fator clampado a 0..1).
    #[test]
    fn facing_matches_the_player_module_contract() {
        // +Z para a frente: dir (0,0,1) → identidade; dir (1,0,0) → yaw +90°.
        assert!(facing_rotation(Vec3::Z).angle_between(Quat::IDENTITY) < 1e-5);
        let right = facing_rotation(Vec3::X);
        assert!((right * Vec3::Z - Vec3::X).length() < 1e-5);

        // Duas rotações opostas (π rad de distância) a 1 rad/s·0,5 s:
        // o fator é max_radians/ângulo = 0,5/π.
        let current = Quat::from_rotation_y(std::f32::consts::PI);
        let target = Quat::IDENTITY;
        let t = facing_slerp_factor(current, target, 1.0, 0.5);
        assert!(
            (t - 0.5 / std::f32::consts::PI).abs() < 1e-5,
            "π rad de distância, 1 rad/s·0,5 s → 0,5/π, got {t}"
        );
        // Já lá está: fator cheio.
        assert_eq!(facing_slerp_factor(target, target, 2.0, 0.5), 1.0);
    }

    /// A calibração dá o par de nominais às duas velocidades claramente
    /// separadas (wander vs chase) e mantém o run acima do walk.
    #[test]
    fn calibration_keeps_the_run_above_the_walk() {
        let mut profile = LocomotionProfile::default();
        assert!(profile.observe(1.5), "primeira velocidade calibra");
        assert!(profile.observe(4.0), "segunda velocidade calibra");
        assert!(!profile.observe(4.0), "repetida não mexe");
        assert!(profile.is_calibrated());
        assert_eq!(profile.walk, 1.5);
        assert_eq!(profile.run, 4.0);

        // Uma velocidade só: o run é sintetizado ACIMA do walk observado.
        let mut solo = LocomotionProfile::default();
        solo.observe(2.0);
        assert!(solo.run > solo.walk, "headroom para o clip de run");
    }
}
