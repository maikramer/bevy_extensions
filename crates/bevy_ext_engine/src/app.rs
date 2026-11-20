//! Contratos de modo da App — extraído do `app.rs` do monólito; o monólito
//! reexporta para `viber::app::RunMode` continuar a resolver.

use bevy::prelude::*;

/// Para quê a App é montada.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Resource)]
pub enum RunMode {
    /// `RunMode::Game`: o jogo.
    #[default]
    Game,
    /// Modo de edição in-process: cena congelada.
    Edit,
}
