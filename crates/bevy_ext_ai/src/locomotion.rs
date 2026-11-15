//! Nominais de locomoção por rig e rotação de facing (+Z).
//!
//! [`LocomotionProfile`] foi movido ao `bevy_ext_core` (exit4/G1): é
//! calibrado pela ANIMAÇÃO do host ([`LocomotionProfile::observe`]) e lido
//! pela IA no integrador (`accel`/`turn_rate`) — UM tipo por contrato, o
//! caminho de reflexão histórico (`viber::animation::LocomotionProfile`)
//! fica congelado no core. Os clamps de playback de clip
//! (`speeds()`/`gates()`) ficaram no módulo de animação do monólito.
//!
//! As funções puras de facing continuam aqui (cópias congeladas por teste
//! das de `viber::player` — o módulo do player ainda vive no monólito).

use bevy::math::{Quat, Vec3};

pub use bevy_ext_core::{DEFAULT_ACCEL, DEFAULT_TURN_RATE, LocomotionProfile};

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
