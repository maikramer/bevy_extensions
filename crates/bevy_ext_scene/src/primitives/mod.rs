//! Primitive shapes and materials — the world DSL contract.
//!
//! This module is render-free: the game parser, the headless tools and the
//! Jackdaw editor all hold the same values. `bevy_ext_gradmesh` converts them
//! into meshes and gradient materials; the game's primitive plugin and the
//! editor preview call that conversion.
//!
//! [`PrimitivePart`] is the bridge used while the world spawner still lives in
//! the transitional engine: it tags a spawned primitive with the authored
//! shape/material so the game can take over its render (and exact collider).

mod attrs;
mod material;
mod shape;

use bevy::prelude::*;

pub use attrs::{PRIMITIVE_TAGS, is_primitive_tag, material_from_attrs, shape_from_attrs};
pub use material::{MaterialSpec, Preset, material_fingerprint};
pub use shape::{Shape, ShapeBounds, bounds};

/// A runtime entity spawned from an authored primitive, before the game's
/// procedural renderer has replaced its fallback mesh/material.
///
/// The transitional engine spawner inserts it; `VaelPrimitivesPlugin` consumes
/// it (and removes the marker). When the Vael owns the world spawner, the
/// same component is inserted directly.
#[derive(Component, Clone, Debug)]
pub struct PrimitivePart {
    pub shape: Shape,
    pub material: MaterialSpec,
}
