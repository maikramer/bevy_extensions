//! Viber — native Bevy engine for AiGameKit declarative world XML.
//!
//! Library crate exposing the XML loader (`xml`), the recipe IR (`recipes`)
//! and gameplay modules (`terrain`); the `viber` binary is a thin CLI on top.

// Os sistemas Bevy exprimem queries grandes e os testes das VMs constroem o
// estado campo a campo; estes lints são ruído de estilo neste código.
#![allow(
    clippy::assertions_on_constants,
    clippy::field_reassign_with_default,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

// NOTA(exit2/scene, 2026-11-15): `config`, `interact`, `meshopt`,
// `particles`, `animation` (o `AnimatedScene`), `hud`, `music`, `streaming`,
// `spawn_gate` e `attachment` TIVERAM gémeos no `bevy_ext_scene_runtime` —
// o crate foi APAGADO (órfão: zero consumidores; cópias reconciliadas aqui
// em `f565750`). Este crate voltou a ser a FONTE ÚNICA destes módulos: na
// saída vão DIRECTOS para os crates de domínio (fases D/F/G do
// DELETE-MONOLITH.md, no Vael) — sem crate-hub novo; a carga de mundo é o
// formato bsn/Jackdaw (`bevy_ext_world::bsn` + specs tipadas
// `bevy_ext_scene`). O `terrain` e o `physics` JÁ são shims para os crates
// extraídos; `a11y`/`locale`/`vitals`/`camera`/`feedback`/`editor_camera`
// também (core/engine, exit2/luau).
// Acessibilidade movida ao `bevy_ext_core`; o caminho `viber::a11y` continua
// a resolver e o recurso é O MESMO (ver o `pub use` na lista em baixo).
pub mod adaptive_quality;
pub mod ai;
pub mod ambient;
pub mod animation;
pub mod app;
pub mod attachment;
// Som 3D + oclusão movidos ao `bevy_ext_audio` (onda exit3/wave-b); o
// caminho `viber::audio_space` reexporta o módulo e o TypePath do
// `SpatialOneShot` fica congelado lá.
pub use bevy_ext_audio::audio_space as audio_space;
pub mod audit;
pub mod bake;
pub mod bridge;
// Câmara de jogo (third-person/script/FX) movida ao `bevy_ext_engine`
// (onda exit2/luau); os contratos (`OrbitCamera`, `ScriptCameraFree`,
// `CameraSnapshot`) são do `bevy_ext_core` — o caminho `viber::camera`
// reexporta o módulo e os tipos são OS MESMOS no jogo.
pub use bevy_ext_engine::camera_glue as camera;
pub mod cinematic;
pub mod cinematic_cli;
// Contrato `config.yaml` movido ao `bevy_ext_engine` (onda exit3/wave-b) —
// o MESMO tipo no jogo; o `fixture()` dos testes do audit também vem daí.
pub use bevy_ext_engine::config as config;
// Módulo movido ao `bevy_ext_physics` (onda exit2/hubs); o caminho
// `viber::corpse` continua a resolver e o tipo é o MESMO no jogo.
pub use bevy_ext_physics::corpse;
pub mod decal_fx;
pub mod dialogue;
pub mod draw_diag;
// Câmara de editor movida ao `bevy_ext_engine` (onda exit2/hubs); o
// `EditorCameraPlugin` é o MESMO tipo no jogo.
pub mod editor_camera;
// Feedback de impacto movido ao `bevy_ext_engine` (onda exit2/hubs); o
// caminho `viber::feedback` reexporta o módulo — o `FeedbackPlugin`
// registado no `app.rs` é o do engine (mesma mensagem para o Vael).
pub use bevy_ext_engine::feedback;
pub mod foliage;
pub mod frame_pacing;
pub mod fx_specs;
pub mod glb_bounds;
pub mod graphics;
pub mod hot_reload;
pub mod hud;
pub mod ibl;
pub use bevy_ext_physics::ik;
pub mod impact;
pub mod input_map;
pub mod interact;
pub mod lint;
// Acessibilidade movida ao `bevy_ext_core` (onda exit2/luau); o caminho
// `viber::a11y` reexporta o módulo — um só recurso `A11y` no jogo.
pub use bevy_engine_core::a11y;
// Texto localizado movido ao `bevy_ext_core` (onda exit2/luau); o host de
// UI (`bevy_ext_ui::ui::host`) reexporta O MESMO `Locale`.
pub use bevy_engine_core::locale;
// Vitais (`Health`/`Hostile`/dano) movidos ao `bevy_ext_core` (onda
// exit2/luau); `bevy_ext_engine` e `bevy_ext_ai` reexportam o MESMO módulo.
pub use bevy_engine_core::vitals;
pub mod luau;
pub mod menus;
pub mod meshopt;
pub mod music;
pub mod nav;
pub mod netplay;
pub mod particles;
pub mod physics;
pub mod physics_fx;
pub mod placement;
pub mod player;
pub mod postfx;
pub mod postfx_toggle;
pub mod probes;
pub mod profiler;
pub mod project;
pub mod projectile;
pub mod prop_tint;
pub mod prune;
// Ragdoll movido ao `bevy_ext_physics` (onda D): o caminho
// `viber::ragdoll` reexporta o módulo e os tipos são OS MESMOS no jogo.
pub use bevy_ext_physics::ragdoll;
pub mod recipes;
pub mod reflect;
pub use bevy_ext_engine::render_lod;
pub mod rng;
pub mod save;
pub mod scaffold;
pub mod scene;
pub mod scene_loader;
pub mod script_kind;
pub mod senses;
pub mod session;
pub mod sky;
pub use bevy_ext_engine::sky_nishita;
pub mod spawn_gate;
pub mod spawner;
pub mod streaming;
pub mod world_ready;
pub mod terrain;
pub mod textures;
pub mod time_fx;
pub mod ui;
pub mod vinst;
pub mod water_ssr;
pub mod worldsys;
pub mod xml;

/// Checkout do jogo Vael, irmão deste repo.
///
/// `VAEL_ROOT` ganha. Sem a variável, procura `../Vael` a partir da raiz do
/// crate. Testes que leem o mundo do jogo saltam quando isto é `None`
/// (CI da engine sozinha).
pub fn vael_root() -> Option<std::path::PathBuf> {
    let candidate = match std::env::var_os("VAEL_ROOT") {
        Some(value) => std::path::PathBuf::from(value),
        None => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Vael"),
    };
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    candidate.is_dir().then_some(candidate)
}
