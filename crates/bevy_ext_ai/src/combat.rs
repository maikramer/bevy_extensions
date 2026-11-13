//! Marcador de hit-react (flinch ao levar dano).
//!
//! TODO(viber-exit): o dono histórico é o módulo de animação da engine (o
//! hit-react system é quem o consome para tocar o clip de reação); vive aqui
//! enquanto não houver crate própria porque os sentidos
//! ([`crate::senses::hear_combat`]) o observam para sintetizar audição. O
//! caminho de reflexão histórico (`viber::animation::HitReact`) fica intacto.

use bevy::prelude::*;

/// Pedido de hit-react: inserido pelas vias de dano (golpe de melee no land,
/// `PlayerHurt` físico no herói). Consumido pelo hit-react system, que o
/// remove SEMPRE — um pedido órfão nunca re-dispara mais tarde.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::animation"]
pub struct HitReact;
