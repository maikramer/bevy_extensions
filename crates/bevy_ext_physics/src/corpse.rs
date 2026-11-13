//! Marca de morte/cadáver.
//!
//! A ENGINE NÃO DECIDE o que a morte significa: `viber.entity_damage` só
//! aplica dano e emite `kill`; o jogo toca o clip, marca `viber.corpse(id)`
//! e despawna quando quiser. `Corpse` serve de filtro para IA/projéteis não
//! continuarem a tratar o corpo como vivo.
//!
//! Movido do monólito (`viber::corpse`); o monólito reexporta este módulo
//! para o caminho histórico continuar a resolver.

use bevy::prelude::*;

/// TTL de referência de um cadáver (s). O despawn é do jogo.
pub const CORPSE_LIFETIME: f32 = 1.4;

/// Morto/em fade. Quem o tem fica fora das queries de alvo vivo.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
// Preserva o TypePath histórico do BRP/`viber.ecs` (o crate mudou de nome
// para bevy_ext_physics).
#[type_path = "viber::corpse"]
pub struct Corpse {
    pub timer: f32,
}
