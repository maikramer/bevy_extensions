//! Runtime de cena dos mundos declarativos: a troca de cena in-process, o
//! spawner do mundo (entity IR → entidades Bevy), o portão de spawn
//! (`<SpawnGate>`) e os anéis de streaming.
//!
//! Extraído do monólito `viber` (fase de saída — `refactor/bevy-native`).
//! O crate é o **hub** da migração: consome os contratos já extraídos
//! ([`bevy_ext_core`], [`bevy_ext_scene`]) e ainda depende do monólito
//! `viber` para o que lá fica (parser do mundo, terreno, física, spawner de
//! grupos, Luau, UI/HUD, céu, navmesh). Cada linha da ponte em baixo é um
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

pub mod recipes;
pub mod scene_loader;
pub mod spawn_gate;
pub mod streaming;

// ── ponte transitória ao monólito `viber` (TODO(viber-exit)) ────────────
//
// Os módulos em baixo são re-exportados INTEIROS para os corpos movidos
// continuarem a escrever `crate::terrain::…`/`crate::physics::…` sem diff
// na lógica. A lista é exatamente o que este crate ainda consome do `viber`:
//
//   ambient      → `SfxEvent`/`AuthoredShadowLight` (portal, luz autoral)
//   animation    → `AnimatedScene` (glTF com clips)
//   app          → `RunMode` (Edit silencia portais/trocas)
//   camera       → `behind_yaw_deg`, `camera_offset`, `third_person_camera`,
//                  `DEFAULT_FOLLOW_LAG`/`DEFAULT_TURN_LAG`/
//                  `DEFAULT_MIN_TERRAIN_DISTANCE`
//   cinematic    → `CinematicPlayback`/`StopCinematic`/`CinematicLibrary`
//   config       → `GameConfig` (bgm/texturas), `load` (testes)
//   editor_camera→ `EditorCamState` (o editor nunca adormece conteúdo)
//   foliage      → `FoliageCandidate` (batch de vegetação)
//   grass        → `GrassField` (reset por cena)
//   hud          → `spawn_hud`, `HudAssets`
//   interact     → `InteractionFocus` (reset + vencedor do portal),
//                  `scaled_range`
//   luau         → host (`LuaScriptHost`), componentes de script, eventos,
//                  input/sfx/toast, ownership, `DEFAULT_ACTIVATION_RADIUS`
//   menus        → `MenusOpen` (menus roubam o input dos portais)
//   meshopt      → `load_gltf`, `asset_file_exists` (ladder implícita)
//   music        → loops kira, `MusicLayerTag`, overrides por cena,
//                  `AudioMixerSettings` (re-export do core)
//   nav          → `reset_for_scene`
//   particles    → `AmbientFx` (lifecycle de FX ambiente)
//   physics      → colisores (`ColliderShape`, `PendingCollider`, …),
//                  `VoxelCollider`, cache/estado por cena
//   physics_fx   → `Knockback` (re-export do core)
//   player       → `landing_position`, `hero_*`, `player_movement`,
//                  `Player`/`TeleportSettle` (re-export do core)
//   project      → `Project`/`MAIN_SCENE` (descoberta de cenas)
//   prop_tint    → `TransmissiveGltf`
//   ragdoll      → `RagdollDeath`
//   render_lod   → `MeshLod`, `AutoCull`, `DEFAULT_STATIC_CULL`
//   scene        → `SceneSource`, doc/ops/sync/live (edição no editor)
//   sky          → `SkyMaterial`, `build_sky` (domo `<Sky>`)
//   spawner      → grupos de spawn/exclusões (`PendingSpawnGroups`, …)
//   terrain      → specs das features, runtime, plugin/delta (reset por cena)
//   textures     → `WorldTiledTextures` (UVs world-space)
//   ui           → UI declarativa (`UiTag`, `StyleSheet`, `build_ui_tree`)
//   vinst        → `.vinst` pré-gerado dos grupos estáticos
//   vitals       → `Hostile`/`Health` (templates de spawner)
//   worldsys     → dia/tempo/fronteira/interior/biomas (`EngineConfigs`, …)
//   xml          → IR XML (`XmlNode`, `NodeOrigin`, `include::load_world*`)
pub use viber::{
    ambient, animation, app, camera, cinematic, config, editor_camera, foliage, grass, hud,
    interact, luau, menus, meshopt, music, nav, particles, physics, physics_fx, player, project,
    prop_tint, ragdoll, render_lod, scene, sky, spawner, terrain, textures, ui, vinst, vitals,
    worldsys, xml,
};
