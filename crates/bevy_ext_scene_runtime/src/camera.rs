//! Helpers de câmara que o spawn e a troca de cena usam.
//!
//! O resto do módulo de câmara (`third_person_camera`, colisão de câmara,
//! auto-follow) continua no monólito `viber::camera` — é o host que o
//! registra na App. O que o spawn precisa é da MATEMÁTICA pura
//! ([`camera_offset`]/[`behind_yaw_deg`] + os defaults do plugin), movida
//! para cá; o sistema fica acessível via re-export para as ordens de
//! schedule (`.before(third_person_camera)`) continuarem a apontar ao
//! sistema que o host adiciona.

use bevy::math::Vec3;

/// O sistema completo de câmara 3.ª pessoa — ainda no monólito
/// (`TODO(viber-exit)`): o host é quem o adiciona à App; o spawner só o
/// referencia em ordens de schedule.
pub use viber::camera::third_person_camera;

/// Delta de ângulo mais curto (graus, −180..180) — o HUD (bússola) e o
/// settle da câmara usam-no; pureza movida com os helpers.
pub use viber::camera::shortest_angle_delta_deg;

/// Default follow-point time constant (plugin default 0.18 s; the
/// simple-rpg world overrides with 0.18).
pub const DEFAULT_FOLLOW_LAG: f32 = 0.18;
/// Default yaw-trail time constant (simple-rpg world: 0.38 s).
pub const DEFAULT_TURN_LAG: f32 = 0.38;
/// Default terrain clearance (plugin default `minTerrainDistance`).
pub const DEFAULT_MIN_TERRAIN_DISTANCE: f32 = 1.0;

/// Camera-to-target offset for a third-person rig. Formula: the
/// pitch ring sits `height` ABOVE the follow point (`desiredY = followY +
/// height + sin(pitch)·dist`) — the authored `height` is additive framing,
/// not the pivot. Pitch 0 keeps the ring horizontal; +90° is straight above.
pub fn camera_offset(yaw_deg: f32, pitch_deg: f32, distance: f32, height: f32) -> Vec3 {
    let yaw = yaw_deg.to_radians();
    // A ±90° o `look_at(…, Vec3::Y)` fica paralelo ao up (roll arbitrário) e
    // além disso `cos` troca de sinal, pondo a câmara do lado oposto e de
    // pernas para o ar. Um grau de folga mantém o rig estável.
    let pitch = pitch_deg.clamp(-89.0, 89.0).to_radians();
    Vec3::new(
        yaw.sin() * pitch.cos() * distance,
        height + pitch.sin() * distance,
        yaw.cos() * pitch.cos() * distance,
    )
}

/// Yaw that puts the camera BEHIND a character moving along heading
/// `(x, z)`: the camera forward is `-(sin yaw, cos yaw)`, so behind a run
/// along `h` is `atan2(-h.x, -h.z)`.
pub fn behind_yaw_deg(vel_x: f32, vel_z: f32) -> f32 {
    (-vel_x).atan2(-vel_z).to_degrees()
}
