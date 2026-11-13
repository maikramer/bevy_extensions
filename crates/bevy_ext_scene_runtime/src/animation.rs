//! O contrato de animação que o spawner de cena consome.
//!
//! O [`AnimatedScene`] nasceu no monólito (`viber::animation`), onde vivem o
//! driver de personagens e o [`bind_animations`]; o spawner do mundo só
//! PRENDE o pedido ao glTF no `<AnimatedScene scene="…">`. Este crate é o
//! dono do tipo que o spawn insere — o driver continua no monólito e lê o
//! MESMO tipo daqui quando adotar este crate (TypePath `viber::animation`
//! congelado: BRP/saves/editor não mudam).
//!
//! [`bind_animations`]: viber::animation::bind_animations

use bevy::prelude::*;

/// Asks for a glTF's clips to be bound to the `AnimationPlayer` the scene
/// spawns. Placed on the entity that owns the scene; `bind_animations` (no
/// monólito) consumes it once a *virgin* player exists somewhere below.
///
/// Consumed on bind, and re-inserted by `rearm_after_scene_swap` whenever
/// something swaps the scene under an already-bound character — today the
/// render-LOD ladder, anything else that mutates `WorldAssetRoot` tomorrow.
#[derive(Debug, Component, Clone, Reflect)]
#[reflect(Component)]
#[type_path = "viber::animation"]
pub struct AnimatedScene {
    pub gltf: Handle<bevy::gltf::Gltf>,
}
