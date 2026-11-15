//! Runtime de cena dos mundos declarativos: a troca de cena in-process, o
//! spawner do mundo (entity IR → entidades Bevy), o portão de spawn
//! (`<SpawnGate>`) e os anéis de streaming.
//!
//! Extraído do monólito `viber` (fase de saída — `refactor/bevy-native`).
//! O crate é o **hub** da migração: consome os contratos já extraídos
//! ([`bevy_ext_core`], [`bevy_ext_scene`], [`bevy_ext_audio`],
//! [`bevy_ext_nav`], [`bevy_ext_terrain_runtime`], [`bevy_ext_physics`]) e
//! ainda depende do monólito (`viber` = `bevy_ext_viber_engine`) para o que
//! lá fica (parser do mundo, spawner de grupos, Luau, UI/HUD-declarativa,
//! céu, vitais, streaming frio). Cada linha da ponte em baixo é um
//! `TODO(viber-exit)`: quando o pedaço correspondente sair do `viber`,
//! trocar o caminho e apagar a linha.
//!
//! Os `TypePath` refletidos estão congelados nos caminhos históricos
//! (`viber::scene_loader::…`, `viber::recipes::spawn::…`, …) via
//! `#[type_path]` — o BRP, os saves e o editor continuam a ver os mesmos
//! tipos depois da mudança de crate.
//!
//! # Mapa
//!
//! | Módulo | Era | O que faz |
//! |--------|-----|-----------|
//! [`scene_loader`] | `viber::scene_loader` | fade → descarrega → carrega → assenta → fade; entradas/portais |
//! [`recipes::spawn`] | `viber::recipes::spawn` | spawner do mundo; `apply_script_spawns` (protótipos em runtime) |
//! [`spawn_gate`] | `viber::spawn_gate` | segura entidades no ar até haver chão de colisão |
//! [`streaming`] | `viber::streaming` | anéis quente/morno/frio; registo frio com pose e estado |
//! [`recipes`] (restante) | `viber::recipes` | ponte ao IR do parse — **transitória**, não extraída |
//!
//! # Módulos próprios (saíram do monólito nesta onda)
//!
//! `camera` (matemática pura + defaults do plugin), `interact` (foco de
//! interação), `meshopt` (shim do `bevy_ext_gltf`), `particles` (runtime de
//! `<ParticleSystem>`), `animation` (o pedido `AnimatedScene` que o spawn
//! insere), `hud` (a construção do HUD na troca de cena). O `cinematic`
//! TENTOU mover nesta onda e voltou: o IR `ParsedWorld.cinematics` tipa o
//! `CinematicSpec` do monólito. Os módulos de contrato que vivem noutros
//! crates chegam por módulos-shim com o MESMO caminho (`crate::menus::…`,
//! `crate::music::…`, `crate::terrain::…`), para os corpos movidos não
//! mudarem. O `config` (contrato `config.yaml`) saiu daqui na exit3/wave-b:
//! vive no `bevy_ext_engine` e é re-exportado — UM só tipo para monólito,
//! este crate e o jogo.

pub mod animation;
pub mod camera;
pub mod hud;
pub mod interact;
pub mod meshopt;
pub mod particles;
pub mod profiling;
pub mod recipes;
pub mod scene_loader;
pub mod spawn_gate;
pub mod streaming;

// `config` — o contrato `config.yaml` vive no `bevy_ext_engine` (onda
// exit3/wave-b); o caminho `crate::config::…` continua a resolver e o
// `GameConfig` é O MESMO tipo que o monólito e o jogo veem.
pub use bevy_ext_engine::config as config;

// ── contratos extraídos, consumidos DIRECTAMENTE ────────────────────────
//
// Shims de módulo com o MESMO caminho histórico (`crate::menus::…` etc.):
// os corpos movidos continuam a escrever os caminhos de sempre; a ORIGEM é
// o crate novo. Cada bloco apaga uma linha da ponte ao `viber`.

/// `app::RunMode` — o enum vive agora no `bevy_ext_core` (contrato de
/// partilha; o host e este crate têm de ver O MESMO resource).
pub mod app {
    pub use bevy_engine_core::RunMode;
}

/// `ambient` — só o que este crate consumia: o evento de SFX saiu para o
/// core; a marca de luz autoral ainda é do monólito (item-level).
pub mod ambient {
    pub use bevy_engine_core::SfxEvent;
    // TODO(viber-exit): `AuthoredShadowLight` segue no `viber::ambient`.
    pub use viber::ambient::AuthoredShadowLight;
}

/// `menus` — o contrato vive no `bevy_ext_core` (re-export `viber::menus`).
pub mod menus {
    pub use bevy_engine_core::MenusOpen;
}

/// `music` — o driver BGM (loops kira, `MusicLayerTag`, overrides por cena)
/// vive no `bevy_ext_audio`; o mixer é contrato do core.
pub mod music {
    pub use bevy_ext_audio::music::*;
    pub use bevy_engine_core::AudioMixerSettings;
}

/// `nav` — o reset de navmesh por cena vive no `bevy_ext_nav`.
pub mod nav {
    pub use bevy_ext_nav::reset_for_scene;
}

/// `physics` — colisores declarativos e corpos Rapier: o crate
/// [`bevy_ext_physics`] é a casa (TypePaths `viber::physics` congelados).
pub use bevy_ext_physics::physics as physics;

/// `physics_fx` — o `Knockback` que a troca de cena limpa é contrato do core
/// (o resto do driver de knockback segue no monólito: os sistemas leem
/// `corpse`/`terrain`, que ainda não saíram).
pub mod physics_fx {
    pub use bevy_engine_core::Knockback;
}

/// `player` — `Player`/`TeleportSettle` são contratos do core (re-exports
/// `viber::player`); o movimento e os probes do herói ainda são do monólito
/// (item-level).
pub mod player {
    pub use bevy_engine_core::{Player, TeleportSettle};
    // TODO(viber-exit): `landing_position`/`hero_*`/`player_movement` seguem
    // no `viber::player` até saírem para um crate de gameplay.
    pub use viber::player::{hero_collider, hero_controller, landing_position, player_movement};
}

/// `terrain` — o runtime voxel (specs das features, plugin/delta, splat)
/// vive no [`bevy_ext_terrain_runtime`]; os TypePaths históricos
/// (`viber::terrain::…`) estão congelados lá.
pub use bevy_ext_terrain_runtime as terrain;

// ── ponte transitória ao monólito `viber` (TODO(viber-exit)) ────────────
//
// A lista é exatamente o que este crate ainda consome do `viber`:
//
//   editor_camera→ `EditorCamState` (o editor nunca adormece conteúdo) — a
//                  cópia do `bevy_ext_engine` ainda lê `viber::` (incompleta)
//   luau         → host (`LuaScriptHost`), componentes de script, eventos,
//                  input/sfx/toast, ownership, `DEFAULT_ACTIVATION_RADIUS`
//                  — ONDA PARALELA `exit2/luau` (não tocar nesta linha)
//   project      → `Project`/`MAIN_SCENE`/`ProjectConfig` (descoberta de cenas)
//   prop_tint    → `TransmissiveGltf`
//   render_lod   → `MeshLod`, `AutoCull`, `DEFAULT_STATIC_CULL`
//   scene        → `SceneSource`, doc/ops/sync/live (edição no editor)
//   sky          → `SkyMaterial`, `build_sky` (domo `<Sky>`)
//   spawner      → grupos de spawn/exclusões (`PendingSpawnGroups`, …)
//   textures     → `WorldTiledTextures` (UVs world-space)
//   ui           → UI declarativa (`UiTag`, `StyleSheet`, `build_ui_tree`)
//   vinst        → `.vinst` pré-gerado dos grupos estáticos
//   vitals       → `Hostile`/`Health`/`health_fraction` (templates de spawner)
//   cinematic    → `CinematicPlayback`/`StopCinematic`/`CinematicLibrary` —
//                  o IR `ParsedWorld.cinematics` tipa o `CinematicSpec` do
//                  monólito; mover para cá exige o shim no monólito, que é
//                  ciclo enquanto a ponta do `luau` existir
//   ragdoll      → `RagdollDeath` (templates de spawner) — a cópia do
//                  `bevy_ext_physics` leu `corpse`/`animation`/`ik` do
//                  monólito e voltou para cá nesta onda; re-trocar quando
//                  esses módulos saírem
//   worldsys     → dia/tempo/fronteira/interior/biomas (`EngineConfigs`, …)
//                  — a cópia do `bevy_ext_engine::worldsys` é parcial (14
//                  refs `viber::`); o `bevy_ext_elements` é o ambiente NOVO
//                  e não tem estes tipos
//   xml          → IR XML (`XmlNode`, `NodeOrigin`, `include::load_world*`)
pub use viber::{
    cinematic, editor_camera, foliage, grass, luau, project, prop_tint, ragdoll, render_lod,
    scene, sky, spawner, textures, ui, vinst, vitals, worldsys, xml,
};
