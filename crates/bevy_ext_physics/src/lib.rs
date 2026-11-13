//! Rapier physics for the Viber world contract.
//!
//! Extracted from the engine monolith (`viber::physics`); the reflected types
//! keep the historical `#[type_path]` values so `.bsn` files, saves and BRP
//! clients keep working.
//!
//! - [`physics`] — colliders and rigid bodies from the declarative world
//!   ([`PhysicsPlugin`]): parsing helpers, deferred mesh shapes, the
//!   `ColliderCache`, voxel terrain collision streaming and the debug render
//!   toggle.
//!
//! O `physics_fx` (impactos/knockback) e o `ragdoll` (mortes curtas) voltaram
//! para o monólito nesta onda: os sistemas deles leem `corpse::Corpse`,
//! `animation::{CharacterAnimator, FrozenAnimation}`, `ik::is_*` e
//! `player::ground_near`, que ainda não saíram para crates próprios — a dep
//! inversa (crate → monólito) bloqueava o shim do `physics` no monólito.
//! Quando `corpse`/`animation` saírem, os dois módulos regressam para cá.

pub mod physics;

pub use physics::{PhysicsPlugin, PENDING_TIMEOUT};
