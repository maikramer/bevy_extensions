//! Procedural primitive rendering for `bevy_gradmesh`.
//!
//! The authored contract (`Shape`, `MaterialSpec`, `Preset`, the DSL attr
//! parser and the `PrimitivePart` bridge) lives in `bevy_ext_scene`, shared by
//! the game, the headless tools and the Jackdaw editor. This crate is the
//! render half:
//!
//! * [`gradmesh_shape`]/[`build_mesh`] — watertight `bevy_gradmesh` geometry
//!   with a subtle per-object vertical gradient baked into vertex colours
//!   (shared materials, no per-entity uniforms);
//! * [`style`] — colour/PBR plus the procedural look presets that replace the
//!   sampled albedo/normal maps;
//! * [`collider`] (feature `physics`) — exact Rapier colliders, trimesh for
//!   the procedural shapes.
//!
//! The game (`VaelPrimitivesPlugin`) and the editor preview both call this
//! crate, so they cannot disagree about the look. The transitional engine
//! only parses the DSL and inserts [`PrimitivePart`]; it does not render
//! gradmesh.

mod material;
mod mesh;
mod shape;

#[cfg(feature = "physics")]
mod collider;

pub use bevy_ext_scene::primitives::*;
// Re-exports so the game only needs this crate in its manifest.
pub use bevy_gradmesh::{GradMeshMaterial, GradMeshPlugin, Shape as GradShape, ShapeDetail};
#[cfg(feature = "physics")]
pub use collider::collider;
pub use material::style;
pub use mesh::{VERTEX_GRADIENT_DARK, bake_vertical_gradient, build_mesh};
pub use shape::gradmesh_shape;
