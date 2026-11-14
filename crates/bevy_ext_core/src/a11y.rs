//! Acessibilidade mínima: legendas, shake, hit-stop e interação por hold.
//!
//! Definição CANÓNICA: o monólito (`viber::a11y`) re-exporta este módulo —
//! a câmara (shake), o HUD (legendas) e o interact leem O MESMO recurso.

use serde::{Deserialize, Serialize};

use bevy::prelude::*;

#[derive(Resource, Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::a11y"]
pub struct A11y {
    /// Escala do texto do balão / legendas.
    pub subtitle_scale: f32,
    pub shake: bool,
    pub hitstop: bool,
    /// [E] precisa de ficar premido [`HOLD_SECS`] em vez de um toque.
    pub interact_hold: bool,
}

impl Default for A11y {
    fn default() -> Self {
        Self {
            subtitle_scale: 1.0,
            shake: true,
            hitstop: true,
            interact_hold: false,
        }
    }
}

pub const HOLD_SECS: f32 = 0.35;

pub fn shake_scale(a11y: &A11y) -> f32 {
    if a11y.shake { 1.0 } else { 0.0 }
}

pub fn hitstop_allowed(a11y: &A11y) -> bool {
    a11y.hitstop
}

pub fn subtitle_px(base: f32, a11y: &A11y) -> f32 {
    base * a11y.subtitle_scale.clamp(0.5, 3.0)
}

/// `true` quando a interação deve disparar este frame.
pub fn interact_ready(hold: bool, pressed_for: f32, just: bool) -> bool {
    if hold {
        pressed_for >= HOLD_SECS && pressed_for - 0.05 < HOLD_SECS
    } else {
        just
    }
}

pub struct A11yPlugin;

impl Plugin for A11yPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<A11y>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    #[test]
    fn shake_and_hitstop_can_be_silenced() {
        let mut a = A11y::default();
        assert_eq!(shake_scale(&a), 1.0);
        a.shake = false;
        a.hitstop = false;
        assert_eq!(shake_scale(&a), 0.0);
        assert!(!hitstop_allowed(&a));
    }

    #[test]
    fn hold_fires_once_when_the_threshold_is_crossed() {
        assert!(!interact_ready(true, 0.1, false));
        assert!(interact_ready(true, HOLD_SECS, false));
        assert!(!interact_ready(true, HOLD_SECS + 0.2, false));
        assert!(interact_ready(false, 0.0, true));
        assert!(!interact_ready(false, 1.0, false));
    }

    /// Contrato da extração: o TypePath histórico fica congelado — o BRP e
    /// os saves não podem ver um caminho novo depois do tipo mudar de crate.
    #[test]
    fn reflected_types_keep_the_historical_viber_paths() {
        assert_eq!(A11y::type_path(), "viber::a11y::A11y");
    }
}
