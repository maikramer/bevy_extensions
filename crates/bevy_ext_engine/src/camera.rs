//! Contratos de câmara — extraídos do monólito (`camera::camera_offset` e o
//! componente `recipes::spawn::OrbitCamera`); o monólito reexporta para
//! `viber::camera::camera_offset` e `viber::recipes::spawn::OrbitCamera`
//! continuarem a resolver. Os sistemas de câmara (third-person, follow,
//! colisão de terreno, FX) vivem no [`crate::camera_glue`] — o monólito
//! re-exporta esse módulo como `viber::camera` (exit2/luau).

use bevy::prelude::*;

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

/// Look-at height used when a world sets no `height` (meters).
pub const DEFAULT_ORBIT_PIVOT_HEIGHT: f32 = 1.2;

/// Marker for `<OrbitCamera>`: keeps its offset from a named target.
///
/// Target-less cameras in worlds with a `<PlayerGLTF>` are third-person
/// cameras: o `third_person_camera` do monólito conduz-nas (follow
/// desacoplado, colisão de terreno) e o `player_movement` governa o yaw com
/// A/D. Cameras com a `target` ficam no follow rígido.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
// Preserva o TypePath histórico (o crate mudou de nome para bevy_ext_engine).
#[type_path = "viber::recipes::spawn"]
pub struct OrbitCamera {
    pub target: Option<String>,
    pub distance: f32,
    pub height: f32,
    /// `Some` only when the world set `pitch` explicitly; it overrides
    /// `height` as the ring seed (ver `orbit_camera_follow` no monólito).
    pub pitch_deg: Option<f32>,
    /// Live orbit pitch (degrees) — seeded from `pitch_deg` or from
    /// `atan(height/distance)`.
    pub pitch_state_deg: f32,
    /// Steered orbit yaw (degrees) — A/D no `player_movement` do monólito.
    pub yaw_deg: f32,
    /// Degrees per pixel of mouse drag (parsed for XML parity; the runtime
    /// is gamepad-style and never reads the mouse).
    pub mouse_sensitivity: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    // --- third-person smoothing config (ThirdPersonCamera) ---
    /// Position follow time constant in seconds (larger = more lag on
    /// sprints).
    pub follow_lag: f32,
    /// Yaw follow time constant in seconds (larger = the camera turns
    /// slower/later than the steering).
    pub turn_lag: f32,
    /// Minimum clearance above the terrain (0 disables camera collision).
    pub min_terrain_distance: f32,
    // --- third-person smoothing state ---
    /// Smoothed follow point the camera orbits and looks at; decoupled from
    /// the raw character transform so view jitter can't accumulate.
    pub follow_point: Vec3,
    /// Lagged yaw the camera actually orbits at; trails [`Self::yaw_deg`].
    pub smooth_yaw_deg: f32,
    /// Current (post-collision, micro-smoothed) camera position.
    pub current_pos: Vec3,
    /// Whether the smoothed state has been seeded onto the target.
    pub initialized: bool,
}

impl OrbitCamera {
    /// Height above the target the camera aims at (meters).
    ///
    /// `height` doubles as the pitch seed when the world gives no explicit
    /// `pitch`; either way it describes how far above the target's origin the
    /// framing sits, so it is the pivot too. Falls back to chest height for
    /// worlds that set neither.
    pub fn pivot_height(&self) -> f32 {
        if self.height > 0.0 {
            self.height
        } else {
            DEFAULT_ORBIT_PIVOT_HEIGHT
        }
    }
}
