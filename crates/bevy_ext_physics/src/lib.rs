//! Avian physics for the Viber world contract.
//!
//! Extracted from the engine monolith (`viber::physics`, `viber::physics_fx`,
//! `viber::ragdoll`, `viber::corpse` and `viber::ik`); the reflected types
//! keep the historical `#[type_path]` values so `.bsn` files, saves and BRP
//! clients keep working. O backend físico é o `avian3d` (linha do Vael/
//! monólito, 0.8 sobre bevy 0.20).
//!
//! - [`physics`] — colliders and rigid bodies from the declarative world
//!   ([`PhysicsPlugin`]): parsing helpers, deferred mesh shapes, the
//!   `ColliderCache`, voxel terrain collision streaming and the debug render
//!   toggle.
//! - [`physics_fx`] — transform-driven impact FX ([`PhysicsFxPlugin`]):
//!   knockback with exact exponential decay and destructible falls, com o
//!   sampler de chão curto ([`physics_fx::ground_near`]). The frozen
//!   component types live in `bevy_ext_core`.
//! - [`ragdoll`] — short death ragdolls ([`RagdollPlugin`]).
//! - [`corpse`] — a marca de cadáver (filtro de IA/projéteis).
//! - [`ik`] — IK de dois ossos dos pés, look-at da cabeça e offsets de ossos.
//! - [`profiling`] — cópia local do contrato `viber::profiler::{Group, timed}`
//!   (o `timed` real, com os anéis de stats, vive no `bevy_ext_engine`).

pub mod corpse;
pub mod ik;
pub mod physics;
pub mod physics_fx;
pub mod profiling;
pub mod ragdoll;

pub use corpse::{Corpse, CORPSE_LIFETIME};
pub use physics::{PhysicsPlugin, PENDING_TIMEOUT};
pub use physics_fx::PhysicsFxPlugin;
pub use profiling::{Group, timed};
pub use ragdoll::{RagdollPlugin, RAGDOLL_SECS};
