//! Scene contract for `bevy_voxel_terrain`: terrain entities, child features,
//! the `.ahgt` heightfield adapter and the field builder.
//!
//! The core crate is runtime-agnostic; this crate is the shared Bevy layer
//! used by the game and by the Jackdaw editor. It deliberately has no
//! renderer and no editor dependency.

pub mod ahgt;
pub mod voxel;

pub use ahgt::AhgtHeightField;
pub use voxel::{
    VoxelArch, VoxelBridge, VoxelBridgeStyle, VoxelCave, VoxelChamber, VoxelFeature, VoxelShaft,
    VoxelTerrain, build_voxel_field,
};
