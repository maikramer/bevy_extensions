//! Água interactiva para Bevy — independente do jogo.
//!
//! A crate só depende de `bevy` (+ `avian3d` para o adaptador de corpos
//! rígidos) e expõe cinco camadas:
//!
//! - [`body`] — corpos de água (lago/rio): consulta `surface_y_at`,
//!   [`WaterDomainSample`](body::WaterDomainSample), contorno orgânico;
//! - [`mirror`] — espelhos CPU do campo de ondas: a cota DESENHADA
//!   ([`surface_height_at`](mirror::surface_height_at)) e o detalhe das
//!   normais ([`wave_height_at`](mirror::wave_height_at));
//! - [`waves`] — solver iWave dispersivo em compute (GPU) com janela rolante
//!   de 128 m, domínio `(depth, flow, plunge)` e impulsos interactivos;
//! - [`material`] — material de dois passes (Multiply/Add) e o sync de
//!   ambiente/clima por uniform;
//! - [`fx`] — contactos de água (entrada/vadeação/saída), esteira Kelvin e
//!   anéis de superfície; partículas são eventos para o host;
//! - [`buoyancy`] — flutuação genérica ([`FloatingBody`](buoyancy::FloatingBody))
//!   com integrador próprio + adapter avian3d ([`buoyancy::avian`]).
//!
//! O jogo liga tudo com [`WaterPlugin`], implementando
//! [`WaterSource`](source::WaterSource) no seu recurso de mundo e publicando
//! [`WaterEnvironment`](environment::WaterEnvironment) e
//! [`WaterObstacles`](source::WaterObstacles) por frame.
//!
//! ```ignore
//! app.add_plugins(bevy_ext_water::WaterPlugin::<MyTerrain>::default());
//! ```

pub mod body;
pub mod buoyancy;
pub mod environment;
pub mod fx;
pub mod material;
pub mod mirror;
pub mod obstacles;
pub mod source;
pub mod waves;

use std::marker::PhantomData;

use bevy::prelude::*;

pub use body::{CascadeInfo, LakeAuthoring, LakeShape, WaterBody, WaterDomainSample, WaterKind};
pub use buoyancy::{FloatSample, FloatState, FloatingBody, WaterBuoyancyPlugin};
pub use environment::WaterEnvironment;
pub use fx::{
    DEFAULT_SWIMMER_RADIUS, Ripple, RippleAssets, SplashPreset, WaterBodied, WaterContact,
    WaterFxPlugin, WaterSplashEvent, WADE_ENTER_DEPTH, WADE_EXIT_DEPTH,
};
pub use material::{
    WaterConfigResource, WaterExtension, WaterMaterial, WaterSurfaceConfig, WaterUniforms,
    WATER_WGSL,
};
pub use mirror::{surface_height_at, wave_height_at};
pub use obstacles::{
    ObstacleSdfSample, WaterObstacle, WaterObstacleKind, deflect_flow_by_obstacles,
    nearest_obstacle_at,
};
pub use source::{WaterFocus, WaterObstacles, WaterSource};
pub use waves::{
    WaterKinematics, WaterWavePlugin, WaterWaveWorld, WaveImpulseRequest, compute_iwave_kernel,
    rasterize_domain_into_bytes, rasterize_domain_with_obstacles,
};

/// Plugin de fachada: solver + FX + flutuação + material.
///
/// `S` é o recurso do host que implementa [`WaterSource`]. O host continua a
/// ser dono de: coletar obstáculos (`WaterObstacles`), escrever o ambiente
/// (`WaterEnvironment`), marcar os corpos que salpicam ([`WaterBodied`]) e
/// consumir [`WaterSplashEvent`] com o seu sistema de partículas.
pub struct WaterPlugin<S: WaterSource>(pub PhantomData<S>);

impl<S: WaterSource> Default for WaterPlugin<S> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<S: WaterSource> Plugin for WaterPlugin<S> {
    fn build(&self, app: &mut App) {
        app.init_resource::<WaterFocus>()
            .init_resource::<WaterObstacles>()
            .init_resource::<WaterEnvironment>()
            .register_type::<WaterKinematics>()
            .register_type::<WaterContact>()
            .register_type::<Ripple>()
            .register_type::<WaterBodied>()
            .add_plugins(WaterWavePlugin::<S>::default())
            .add_plugins(WaterFxPlugin::<S>::default())
            .add_plugins(WaterBuoyancyPlugin::<S>::default());
    }
}

/// Re-export do prelude de avian para quem usa o adaptador de corpos rígidos.
#[cfg(feature = "avian")]
pub use avian3d;
