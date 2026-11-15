//! Marca de morte/cadáver.
//!
//! A definição canónica vive no `bevy_ext_physics` (exit4/G1 — o mesmo tipo
//! que o `viber::corpse` do monólito reexporta); aqui fica só o reexport
//! para os caminhos históricos do crate (`bevy_ext_ai::corpse::Corpse`).

pub use bevy_ext_physics::corpse::{CORPSE_LIFETIME, Corpse};
