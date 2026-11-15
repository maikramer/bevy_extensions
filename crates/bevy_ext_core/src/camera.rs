//! Utilidades de câmara partilhadas (o "olho" que desenha a janela).

use bevy::prelude::*;

/// Uma câmara que desenha para uma janela e está activa — o olho do jogo
/// (ignora câmaras offscreen como o bake de impostores da flora).
pub fn is_window_camera(
    cam: Option<&Camera>,
    target: Option<&bevy::camera::RenderTarget>,
) -> bool {
    let active = cam.is_none_or(|c| c.is_active);
    let window = target.is_none_or(|t| matches!(t, bevy::camera::RenderTarget::Window(_)));
    active && window
}
