//! Exclusões de spawn — extraído do `spawner.rs` do monólito; o monólito
//! reexporta para `viber::spawner::{SpawnExclusion, SpawnExclusions}`
//! continuarem a resolver.
//!
//! Os spawners (colocação de props) continuam no monólito e inserem o
//! recurso; os consumidores de fora (relva procedural) leem o tipo daqui —
//! um tipo só no ECS.

use bevy::prelude::*;

/// Todos os discos `<SpawnExclusion>` do mundo, como RECURSO.
///
/// Separado do estado de colocação (que é de uma passagem só e é removido no
/// fim) porque os discos são dados AUTORAIS e têm mais do que um consumidor:
/// os spawners (colocação) e a relva procedural (que desenha um tapete por
/// cima de tudo e ignorava os discos — um acampamento numa clareira acordava
/// enterrado em tufos).
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::spawner"]
pub struct SpawnExclusions(pub Vec<SpawnExclusion>);

impl SpawnExclusions {
    /// `(x, z)` está dentro de algum disco?
    pub fn contains(&self, x: f32, z: f32) -> bool {
        self.0.iter().any(|e| {
            let dx = x - e.center.x;
            let dz = z - e.center.y;
            dx * dx + dz * dz <= e.radius * e.radius
        })
    }
}

/// Global no-spawn circle (`<SpawnExclusion at="x z" radius="n">`).
#[derive(Debug, Clone, Copy, Reflect)]
#[type_path = "viber::spawner"]
pub struct SpawnExclusion {
    pub center: bevy::math::Vec2,
    pub radius: f32,
}
