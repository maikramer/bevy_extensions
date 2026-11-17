//! GLB decoding and asset helpers that Bevy 0.20 does not provide.
//!
//! The flagship is [`meshopt`]: an `AssetReader` that expands
//! `EXT_meshopt_compression` in-process, widens `KHR_mesh_quantization`
//! attributes to the `f32` streams Bevy expects, and lifts
//! `KHR_texture_basisu` sources so KTX2 textures load without a
//! pre-decompression build step.
//!
//! Optional helpers:
//! - [`glb_bounds`] reads the scene AABB from the GLB JSON without loading the
//!   renderer.
//! - [`textures`] generates CPU mipmaps and applies the world-tiled sampler.
//!
//! The crate is independent of any game runtime: the reader wraps the default
//! `AssetSource` and the caller registers it before `AssetPlugin` is built.

pub mod glb;

/// Public façade: the module path the game and the editor already import.
pub mod meshopt;

pub mod extensions;

pub mod decode;

pub mod dequant;

pub mod reader;

pub mod pool;

pub mod settings;

pub mod stats;

/// The pre-rewrite decoder, kept as a byte-for-byte oracle for the parity
/// tests and the benchmark's A/B. Not meant for shipping builds.
#[cfg(feature = "legacy")]
pub mod legacy;

#[cfg(test)]
mod test_support;

#[cfg(feature = "bounds")]
pub mod glb_bounds;

#[cfg(feature = "textures")]
pub mod textures;

pub use crate::meshopt::{
    DecodeStats, Filter, GltfDecodeSettings, GltfStatsPlugin, MeshoptAssetReader, Mipmaps, Mode,
    asset_file_exists, decode_glb, decode_stats, decode_stream, load_gltf, load_gltf_with,
    needs_decode, needs_rewrite, register_asset_source, settings,
};

#[cfg(feature = "bounds")]
pub use glb_bounds::GlbBounds;

#[cfg(feature = "textures")]
pub use textures::{
    TexturesPlugin, WorldTiledTextures, generate_mipmaps_on_load, load_tiled_image, patch_image,
    world_tiled_sampler,
};
