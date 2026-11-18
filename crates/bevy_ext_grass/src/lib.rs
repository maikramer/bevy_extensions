//! Relva GPU para Bevy, com o `bevy_grass` como renderer.
//!
//! O crate é o meio-termo entre o renderer puro (`bevy_grass`) e um jogo
//! concreto:
//!
//! * **terreno genérico**: o jogo injeta um [`GrassTerrain`] (altura, regras
//!   de sítio e bioma) e um [`GrassFieldLayout`] (onde ficam os campos); o
//!   crate faz o bake das máscaras/heightmaps numa task, a 2 m/4 m;
//! * **estilos autorados**: [`GrassSettingsSpec`] e [`GrassStyleSpec`] são
//!   componentes `Reflect` (`type_path = "bevy_ext_grass"`) para editar no
//!   Jackdaw e guardar no `.bsn`;
//! * **Bevy 0.20**: plugin, sistemas em [`GrassSystems`], sombras do sol,
//!   `NoGrass` em câmaras de render-para-textura e prepass herdado do
//!   `bevy_grass`;
//! * **avian3d**: [`GrassTrampler`] + collider viram `GrassInteractor` para o
//!   herói/NPCs dobrarem a relva (feature `avian`, default).
//!
//! O jogo só tem de inserir um [`GrassProvider`] (e trocá-lo ao mudar de
//! cena), opcionalmente escrever [`GrassTint`] e o `bevy_grass::GrassWind`, e
//! marcar os tramplers. Sem provider não há campos: cenas de interior ficam
//! sem relva sem nenhum código extra.
//!
//! O `bevy_grass` é re-exportado ([`bevy_grass`]) para o jogo não precisar de
//! o declarar outra vez.

mod bake;
mod plugin;
mod specs;
mod terrain;
#[cfg(feature = "avian")]
mod trample;

pub use bake::{FieldData, GrassBakeConfig, StaticMaps, bake_dynamic, bake_static};
pub use plugin::{
    GrassBiomeSnow, GrassPlugin, GrassPluginConfig, GrassSettings, GrassSettingsSpecValue,
    GrassState, GrassStyleRegistry, GrassStyleSpecSet, GrassSystems, GrassTint, grass_snow_tint,
    maintain_grass, settings_to_bg,
};
pub use specs::{
    GrassPlantSpec, GrassQualitySpec, GrassSettingsSpec, GrassSettingsSpecPlugin, GrassStyleSpec,
    style_from_spec,
};
pub use terrain::{
    FieldPlan, GrassBiomeId, GrassFieldLayout, GrassLayout, GrassProvider, GrassSite, GrassTerrain,
    SingleFieldLayout,
};
#[cfg(feature = "avian")]
pub use trample::{GrassTrampler, GrassTramplerDisc};

#[cfg(feature = "avian")]
pub use avian3d;
/// O renderer por baixo: `bevy_grass` 0.20.6. Re-exportado para o jogo usar
/// `GrassWind`, `GrassStyle`, `GrassStats`, etc. sem uma dependência paralela.
pub use bevy_grass;
pub use bevy_grass::{
    GrassBlade, GrassField, GrassFieldOverrides, GrassGround, GrassInteractor,
    GrassInteractorFromCollider, GrassPlugin as BevyGrassPlugin, GrassQuality, GrassShadows,
    GrassSpecies, GrassStats, GrassStyle, GrassSun, GrassSupport, GrassVariation, GrassWind,
    GroundTint, NoGrass,
};
