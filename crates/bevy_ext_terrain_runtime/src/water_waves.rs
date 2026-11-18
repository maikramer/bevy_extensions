//! Ponte do solver de água da crate `bevy_ext_water` para o terreno.
//!
//! O solver (compute iWave, janela rolante, domínio e impulsos) vive na crate,
//! genérico sobre [`bevy_ext_water::WaterSource`]. Aqui só ficam o
//! re-export dos tipos históricos e o plugin já especializado no
//! [`TerrainRuntime`](crate::runtime::TerrainRuntime) desta crate.

pub use bevy_ext_water::waves::{
    DOMAIN_GRID_SIZE, DOMAIN_SNAP_M, DOMAIN_TEXEL_M, MAX_IMPULSES, WATER_WAVES_WGSL,
    WAVE_GRID_SIZE, WAVE_TEXEL_M, WAVE_WORLD_SIZE, WaterKinematics, WaterWaveWorld,
    WaveImpulseRequest, bodies_signature, compute_iwave_kernel, make_wave_rgba16f_image,
    rasterize_domain_into_bytes, rasterize_domain_with_obstacles,
};

use bevy::prelude::*;

/// Solver de ondas da engine, já ligado ao [`TerrainRuntime`].
///
/// [`TerrainRuntime`]: crate::runtime::TerrainRuntime
pub struct WaterWavePlugin;

impl Plugin for WaterWavePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_ext_water::WaterWavePlugin::<
            crate::runtime::TerrainRuntime,
        >::default());
    }
}
