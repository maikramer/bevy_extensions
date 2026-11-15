//! Runtime de terreno volumétrico — a camada ECS sobre [`bevy_voxel_terrain`].
//!
//! # Contratos
//!
//! * **O heightfield é input, não a forma.** O [`brush::BrushGrid`] é o termo
//!   base do SDF (`densidade(p) = p.y − base.sample(p.xz)`); o
//!   [`voxel::VoxelField`] combina-o em CSG com os mods 3D e o transvoxel
//!   mesha o zero do campo. A grelha não é renderizada nem colide
//!   directamente — é o alvo dos carves e a fonte dos registries.
//! * **Colunas, não chunks 2.5D.** Cada coluna de `chunk-size` metros é
//!   meshada por caixas voxel e colide por um trimesh Rapier com os mesmos
//!   triângulos do render (`voxel::spawn`), assado no swap atómico do LOD.
//! * **LOD por coluna** com histerese e clamp 2:1; o transvoxel faz as
//!   transições laterais sem saias ([`plugin::TerrainPlugin`]).
//! * **Duas vias de look:** o tint legado de altura/inclinação em vertex
//!   colors, ou o splat blend de [`splat`] + [`layer_material`] (13 slots de
//!   camada, material bindless único), que substitui o tint.
//! * **Edições vivas** ficam no overlay [`delta::DeltaGrid`]; nenhum
//!   consumidor lê a grelha crua em runtime.
//!
//! # Bootstrap
//!
//! O [`runtime::TerrainFeaturesPlugin`] é o arranque one-shot: carrega/gera a
//! heightmap → carve pads → água → estradas → cliffs 3D + máscara →
//! `<RockFeatures>` → mods do campo voxel → spawna as colunas dentro do raio
//! de render, água e ribbons. O input é o [`runtime::PendingTerrain`]
//! (spec + features + pasta base), preenchido pelo spawner declarativo do
//! host — esta crate não conhece o IR de parsing.
//!
//! # Integração com o host (TODO(viber-exit))
//!
//! Os pontos onde o runtime toca serviços do monólito Viber vivem em
//! [`host`]: marcadores de cena/âncoras de streaming, colliders voxel,
//! specs de partículas e a vista do céu/tempo. Cada item documenta o que o
//! host tem de fazer para os pontes. Os `TypePath` históricos
//! `viber::terrain::…` estão congelados nos tipos reflectidos (o `.bsn`, os
//! saves e o BRP dependem deles).

pub mod brush;
pub mod cliffs;
pub mod cut;
pub mod decal;
pub mod delta;
pub mod features;
pub mod heightmap;
pub mod host;
pub mod layer_material;
pub mod mesh;
pub mod pads;
pub mod paint;
pub mod paths;
pub mod paving;
pub mod plateau;
pub mod plugin;
pub mod profiling;
pub mod roads;
pub mod runtime;
pub mod shore_rocks;
pub mod spec;
pub mod splat;
pub mod voxel;
pub mod water;
pub mod water_buoyancy;
pub mod water_fx;
pub mod water_material;
pub mod water_obstacles;
pub mod water_waves;

pub use brush::{BrushGrid, BrushMode};
pub use cliffs::{CliffMask, CliffProfile, CliffSide, CliffSpec, sharpen_terrain};
pub use cut::CutSpec;
pub use decal::{GroundDecalSpec, ground_decal_mesh};
pub use delta::{DeltaGrid, EditedBase, TerrainEdit, TerrainEditQueue};
pub use heightmap::HeightMapU16;
pub use host::StreamingBoost;
pub use layer_material::TerrainChunkMaterial;
pub use mesh::{ChunkMeshData, HeightField};
pub use pads::ResolvedPad;
pub use paint::PaintGrid;
pub use paths::{chaikin_smooth, resample};
pub use plateau::PlateauSpec;
pub use plugin::{TerrainPlugin, TerrainSet};
pub use profiling::{Group, timed};
pub use roads::{RoadNetworkSpec, RoadPath, RoadProfile, RoadSpec, SegmentSpec, WaySpec};
pub use runtime::{PendingTerrain, TerrainCollect, TerrainFeaturesPlugin, TerrainRuntime};
pub use shore_rocks::ShoreRocksSpec;
pub use spec::{TerrainPadSpec, TerrainSpec, TerrainTint};
pub use voxel::{
    Bounds3, BoxMod, CapsuleMod, ChunkClass, ModIndex, ModOp, Span, VoxelChunkParams, VoxelField,
    VoxelMod, build_voxel_mesh,
};
pub use water::{
    CascadeInfo, LakeAuthoring, LakeSpec, RiverSpec, WaterBody, WaterKind, surface_height_at,
    wave_height_at,
};
pub use water_buoyancy::{FloatSample, FloatState, FloatingBody, WaterBuoyancyPlugin};
pub use water_fx::WaterFxPlugin;
pub use water_material::{WaterExtension, WaterMaterial, WaterSurfaceConfig};
pub use water_waves::{WaterKinematics, WaterWavePlugin, WaterWaveWorld, WaveImpulseRequest};
