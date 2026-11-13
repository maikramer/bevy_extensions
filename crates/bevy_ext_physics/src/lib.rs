//! Rapier physics for the Viber world contract.
//!
//! Extracted from the engine monolith (`viber::physics`, `viber::physics_fx`
//! and `viber::ragdoll`); the reflected types keep the historical
//! `#[type_path]` values so `.bsn` files, saves and BRP clients keep working.
//!
//! - [`physics`] — colliders and rigid bodies from the declarative world
//!   ([`PhysicsPlugin`]): parsing helpers, deferred mesh shapes, the
//!   `ColliderCache`, voxel terrain collision streaming and the debug render
//!   toggle.
//! - [`physics_fx`] — transform-driven impact FX ([`PhysicsFxPlugin`]):
//!   knockback with exact exponential decay and destructible falls. The
//!   frozen component types live in `bevy_ext_core`.
//! - [`ragdoll`] — short death ragdolls ([`RagdollPlugin`]).

pub mod physics;
pub mod physics_fx;
pub mod ragdoll;

pub use physics::{PhysicsPlugin, PENDING_TIMEOUT};
pub use physics_fx::PhysicsFxPlugin;
pub use ragdoll::{RagdollPlugin, RAGDOLL_SECS};
