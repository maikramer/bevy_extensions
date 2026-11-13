//! Scene contract for `bevy_voxel_terrain`: terrain entities, child features,
//! the `.ahgt` heightfield adapter and the field builder.
//!
//! The core crate is runtime-agnostic; this crate is the shared Bevy layer
//! used by the game and by the Jackdaw editor. It deliberately has no
//! renderer and no editor dependency.

pub mod ahgt;
pub mod bsn;
pub mod paint;
pub mod voxel;

pub use ahgt::AhgtHeightField;
pub use bsn::feature_attr_default;
pub use paint::{LAYER_COUNT, PaintGrid, SLOT_FLAT};
pub use voxel::{
    VoxelArch, VoxelArchProfile, VoxelBridge, VoxelBridgeStyle, VoxelCave, VoxelChamber,
    VoxelCliff, VoxelFeature, VoxelShaft, VoxelTerrain, build_voxel_field,
};

/// Regista os tipos num registo avulso (loader sem `App`), como o
/// `bevy_ext_scene::register_types`.
pub fn register_types(registry: &mut bevy::reflect::TypeRegistry) {
    registry.register::<voxel::VoxelTerrain>();
    registry.register::<voxel::VoxelCave>();
    registry.register::<voxel::VoxelChamber>();
    registry.register::<voxel::VoxelShaft>();
    registry.register::<voxel::VoxelArch>();
    registry.register::<voxel::VoxelArchProfile>();
    registry.register::<voxel::VoxelBridge>();
    registry.register::<voxel::VoxelBridgeStyle>();
    registry.register::<voxel::VoxelCliff>();
    registry.register::<Option<f32>>();
    registry.register::<[f32; 2]>();
    registry.register::<Vec<[f32; 2]>>();
    registry.register::<Vec<f32>>();
    registry.register::<Vec<voxel::VoxelChamber>>();
    registry.register::<Vec<voxel::VoxelShaft>>();
}
