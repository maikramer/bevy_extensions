//! Voxel/SDF terrain: the 3D shape layer over a heightfield.
//!
//! The heightfield is demoted to a **term** of the signed-distance field; the
//! voxel mods (caves, cliffs, bridges, arches, riverbanks) are CSG solids on
//! top. This crate is the runtime-agnostic core: [`VoxelField`], the CSG mods,
//! the column index and the Transvoxel mesher. The ECS spawner, colliders,
//! water/roads authoring and the concrete `BrushGrid` stay in the engine or in
//! a future `bevy_ext_terrain` crate.
//!
//! The [`HeightField`] trait is the boundary: the host app supplies the base
//! heightfield (AHGT, procedural, edited overlay), and the voxel field adds the
//! 3D solids without owning it.

pub mod arch;
pub mod bridge;
pub mod cave;
pub mod field;
pub mod heightfield;
pub mod index;
pub mod mesh;
pub mod mods;
pub mod paths;
pub mod transvoxel_mesh;

pub use arch::ArchSpec;
pub use bridge::{BridgeSpec, BridgeStyle};
pub use cave::CaveSpec;
pub use field::{Span, VoxelField};
pub use heightfield::HeightField;
pub use index::{ChunkClass, ModIndex};
pub use mesh::{ChunkMeshData, TintParams, tint_vertex_color};
pub use mods::{
    ArchMod, Bounds3, BoxMod, CapsuleMod, EllipsoidMod, HalfEllipsoidMod, ModOp, OrientedBoxMod,
    RoundConeMod, VoxelMod,
};
pub use transvoxel_mesh::{
    VOXEL_CHUNK_CELLS, VoxelChunkParams, build_voxel_mesh, transition_sides,
};
