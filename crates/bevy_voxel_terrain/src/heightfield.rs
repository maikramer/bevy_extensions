//! The heightfield base term of the voxel SDF.
//!
//! The host app owns the heightfield (AHGT, procedural or edited overlay); the
//! voxel field borrows it per query. Keeping this as a trait is what lets the
//! voxel crate stay independent of the terrain storage/renderer.

use bevy::math::Vec3;

pub trait HeightField {
    /// Terrain height at a world XZ position (per the grid's interpolation mode).
    fn sample(&self, world_x: f32, world_z: f32) -> f32;
    /// Surface normal via central differences with the given world epsilon.
    /// Callers pass a terrain-wide constant epsilon (not per-chunk/per-LOD)
    /// so normals agree on shared chunk borders.
    fn sample_normal(&self, world_x: f32, world_z: f32, epsilon: f32) -> Vec3;
    /// Peak height of the heightfield (meters).
    fn max_height(&self) -> f32;
    /// Raw min/max height over the axis-aligned world XZ range (meters).
    /// `None` = the field cannot answer range queries (the mesh builder then
    /// falls back to plain point sampling).
    fn range_over(&self, _min_x: f32, _min_z: f32, _max_x: f32, _max_z: f32) -> Option<(f32, f32)> {
        None
    }
}
