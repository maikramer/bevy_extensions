//! Grab-bag de sistemas pequenos da engine Viber — módulos extraídos do
//! monólito um a um, com o plugin de cada um exposto. **Será dividido** em
//! crates temáticos no fim da migração.
//!
//! Os TypePath históricos estão preservados: os tipos refletidos mantêm
//! `#[type_path = "viber::<módulo>"]`, portanto o registry de reflexão, os
//! saves e as ferramentas BRP continuam a ver `viber::worldsys::DayCycleState`
//! e companhia como antes.
//!
//! | Módulo | Plugin |
//! |--------|--------|
//! | [`cinematic`] | [`cinematic::CinematicPlugin`] |
//! | [`decal_fx`] | [`decal_fx::DecalFxPlugin`] |
//! | [`foliage`] | [`foliage::FoliagePlugin`] |
//! | [`grass`] | [`grass::GrassPlugin`] |
//! | [`textures`] | [`textures::TexturesPlugin`] (reexport do `bevy_ext_gltf`) |
//! | [`editor_camera`] | [`editor_camera::EditorCameraPlugin`] |
//! | [`sky`] | [`sky::SkyPlugin`] |
//! | [`vitals`] | [`vitals::VitalsPlugin`] (reexport do `bevy_ext_core`) |
//! | [`feedback`] | [`feedback::FeedbackPlugin`] |
//! | [`camera_glue`] | sistemas de câmara de jogo (third-person/script/FX) |
//! | [`netplay`] | [`netplay::NetPlayPlugin`] (listen server `VIBER_NET`) |
//! | [`render_lod`] | ladder de LOD/glTF por distância |
//! | [`frame_pacing`] | [`frame_pacing::FramePacingPlugin`] |
//! | [`draw_diag`] | [`draw_diag::DrawDiagPlugin`] |
//! | [`water_ssr`] | [`water_ssr::WaterSsrPlugin`] |
//! | [`worldsys`] | [`worldsys::WorldsysPlugin`] (drivers do mundo) |
//!
//! [`graphics`] (preset de gráficos), [`dlss`] (parser do gate) e [`textures`]
//! não têm plugin próprio aqui: são config/helpers ou vivem no
//! `bevy_ext_gltf`.

// O caminho `viber::…` resolve para este próprio crate (só o `hud::assets`
// do alias de compat em baixo o usa).
extern crate self as viber;

pub mod app;
pub mod ambient;
pub mod animation;
pub mod camera;
pub mod camera_glue;
pub mod cinematic;
pub mod config;
pub mod decal_fx;
pub mod dlss;
pub mod draw_diag;
pub mod editor_camera;
pub mod feedback;
pub mod foliage;
pub mod frame_pacing;
pub mod graphics;
pub mod grass;
pub mod hud;
pub mod hud_assets;
pub mod ibl;
pub mod luau;
pub mod netplay;
pub mod profiler;
pub mod render_lod;
pub mod scene_loader;
pub mod sky;
pub mod sky_nishita;
pub mod spawner;
pub mod textures;
pub mod vitals;
pub mod water_ssr;
pub mod worldsys;

