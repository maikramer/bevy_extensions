//! Meshopt/KTX2 GLB decoder — shim para o `bevy_ext_gltf::meshopt`.
//!
//! O decoder, o leitor multi-root de assets e o `load_gltf` (com a ladder
//! implícita de LODs) vivem no crate [`bevy_ext_gltf`]; o spawner de cena
//! consome-os por aqui para os paths `crate::meshopt::…` não mudarem.

pub use bevy_ext_gltf::meshopt::*;
