//! Ponte da flutuação da crate `bevy_ext_water` para o runtime de terreno.
//!
//! [`FloatingBody`]/[`FloatState`] e o integrador restrito vivem na crate; o
//! plugin aqui já vem especializado no [`TerrainRuntime`] desta crate.
//! Barcos costumam usá-lo diretamente (`step_float` para assentar no spawn).

pub use bevy_ext_water::buoyancy::{
    FLOAT_GRAVITY, FloatForces, FloatSample, FloatState, FloatingBody, HULL_COLUMNS, MAX_SUBSTEP,
    MAX_SUBSTEPS, MAX_TILT, WATER_DENSITY, hydrostatic_forces, step_float,
};

use bevy::prelude::*;

/// Flutuação da engine, ligada ao [`TerrainRuntime`].
///
/// [`TerrainRuntime`]: crate::runtime::TerrainRuntime
pub struct WaterBuoyancyPlugin;

impl Plugin for WaterBuoyancyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_ext_water::WaterBuoyancyPlugin::<
            crate::runtime::TerrainRuntime,
        >::default());
    }
}
