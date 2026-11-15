//! Ponte dos FX de água da crate `bevy_ext_water` para o Vael.
//!
//! A deteção de contacto (entrada/vadeação/saída), a esteira Kelvin e os anéis
//! de superfície vivem na crate, que emite [`WaterSplashEvent`] em vez de
//! conhecer o sistema de partículas do jogo. `water_bridge` traduz esses
//! eventos para `spawn_burst` e marca os corpos (`WaterBodied`).

pub use bevy_ext_water::fx::{
    DEFAULT_SWIMMER_RADIUS, ImpactImpulse, Ripple, RippleAssets, SplashPreset, WaterBodied,
    WaterContact, WaterDepthZone, WADE_ENTER_DEPTH, WADE_EXIT_DEPTH, classify_depth_zone,
    compute_entry_impulse, compute_wake_impulse, ripple_alpha, ripple_mesh, ripple_radius,
    splash_count, swept_entry_xz,
};

use bevy::prelude::*;

/// FX de água da engine, já ligados ao [`TerrainRuntime`].
///
/// [`TerrainRuntime`]: crate::terrain::runtime::TerrainRuntime
pub struct WaterFxPlugin;

impl Plugin for WaterFxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(
            bevy_ext_water::WaterFxPlugin::<crate::terrain::runtime::TerrainRuntime>::default(),
        );
    }
}
