//! glTF reading for Bevy 0.20: `EXT_meshopt_compression`,
//! `KHR_mesh_quantization` and `KHR_texture_basisu`, plus the multi-root asset
//! source they run in.
//!
//! This module is the public surface the game and the editor import; the
//! implementation lives in [`crate::decode`] (the rewrite), [`crate::reader`]
//! (the `AssetReader`), [`crate::pool`] (existence checks) and
//! [`crate::dequant`] (quantized attributes → `f32`).

pub use crate::decode::{
    Filter, Mode, decode_glb, decode_glb_named, decode_stream, decode_stream_into, needs_decode,
    needs_rewrite,
};
pub use crate::pool::asset_file_exists;
pub use crate::reader::{MeshoptAssetReader, load_gltf, load_gltf_with, register_asset_source};
pub use crate::settings::{GltfDecodeSettings, Mipmaps, settings};
pub use crate::stats::{DecodeStats, GltfStatsPlugin, decode_stats};
