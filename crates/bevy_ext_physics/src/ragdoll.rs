// Contrato movido ao `bevy_ext_core` (com o consumidor `bevy_ext_nav`, que
// filtra obstáculos pelo MESMO tipo que a física insere); o TypePath
// `viber::ragdoll` fica congelado no core.
pub use bevy_ext_core::RagdollBodyOf;
