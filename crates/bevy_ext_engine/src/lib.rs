//! Grab-bag transitório de sistemas pequenos da engine Viber — módulos
//! extraídos de `viber/src/*.rs` um a um, com o plugin de cada um exposto.
//! **Será dividido** em crates temáticos quando os módulos ainda presos ao
//! monólito (`viber::…`, marcados `// TODO(viber-exit)`) saírem de lá.
//!
//! Os TypePath históricos estão preservados: os tipos refletidos mantêm
//! `#[type_path = "viber::<módulo>"]`, portanto o registry de reflexão, os
//! saves e as ferramentas BRP continuam a ver `viber::worldsys::DayCycleState`
//! e companhia como antes.
//!
//! | Módulo | Plugin |
//! |--------|--------|
//! | [`netplay`] | [`netplay::NetPlayPlugin`] |
//! | [`cinematic`] | [`cinematic::CinematicPlugin`] |
//! | [`decal_fx`] | [`decal_fx::DecalFxPlugin`] |
//! | [`foliage`] | [`foliage::FoliagePlugin`] |
//! | [`grass`] | [`grass::GrassPlugin`] |
//! | [`textures`] | [`textures::TexturesPlugin`] (reexport do `bevy_ext_gltf`) |
//! | [`editor_camera`] | [`editor_camera::EditorCameraPlugin`] |
//! | [`sky`] | [`sky::SkyPlugin`] |
//! | [`vitals`] | [`vitals::VitalsPlugin`] |
//! | [`feedback`] | [`feedback::FeedbackPlugin`] |
//! | [`frame_pacing`] | [`frame_pacing::FramePacingPlugin`] |
//! | [`draw_diag`] | [`draw_diag::DrawDiagPlugin`] |
//! | [`water_ssr`] | [`water_ssr::WaterSsrPlugin`] |
//! | [`worldsys`] | [`worldsys::WorldsysPlugin`] (drivers do mundo) |
//!
//! [`graphics`] (preset de gráficos), [`dlss`] (parser do gate) e [`textures`]
//! não têm plugin próprio aqui: são config/helpers ou vivem no
//! `bevy_ext_gltf`.

pub mod cinematic;
pub mod decal_fx;
pub mod dlss;
pub mod draw_diag;
pub mod editor_camera;
pub mod feedback;
pub mod foliage;
pub mod frame_pacing;
pub mod graphics;
pub mod grass;
pub mod netplay;
pub mod sky;
pub mod textures;
pub mod vitals;
pub mod water_ssr;
pub mod worldsys;
