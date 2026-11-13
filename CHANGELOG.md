# Changelog

## Unreleased

- New `bevy_ext_audio`: the Viber runtime audio stack — SFX one-shots
  (`SfxEvent`/`SfxLoudEvent` with distance attenuation and pitch jitter),
  ambient loops (water/waterfall/rain), typed kira buses with live
  `mixer_sync`, stacking per-bus ducking, reverb presets, BGM layers with
  zone crossfade and combat music. The host publishes `AudioPaths`,
  `AudioWorldState` and `HostWaterBodies` (a trait over terrain water); the
  kira backend and channels stay host-side. Shared types are re-exported
  from `bevy_ext_core`, `viber::*` reflection paths frozen.

## 0.1.0 — 2026-11-12

First release of the monorepo.

- Renamed `bevy_engine_core` → `bevy_ext_core`, `bevy_world_xml` →
  `bevy_ext_world`, `bevy_save_slots` → `bevy_ext_save`. Historical
  `#[type_path]` values (`viber::*`, `bevy_world_xml::bsn`) are frozen so
  existing `.bsn` files, saves and BRP clients keep working.
- New `bevy_ext_gltf`: meshopt, `KHR_mesh_quantization`,
  `KHR_texture_basisu` lifting, UASTC `RRR`, GLB bounds and world-tiled
  textures.
- New `bevy_voxel_terrain`: `VoxelField`, CSG mods, caves/arches/bridges,
  column index and Transvoxel meshing over a host `HeightField`.
- New `bevy_ext_terrain`: Bevy scene contract for the voxel terrain and the
  `.ahgt` heightfield adapter. Held for 0.1.1 while the scene contract
  stabilizes; the 0.1.0 release publishes the other five crates.
- `bevy_ext_world` now carries the full XML/BSN/project/include/emit layer.
- `bevy_ext_save`: tests for slot paths, legacy migration, corrupt JSON,
  atomic write/rename and slot listing (2 → 8 tests).
- `bevy_ext_gltf`: two small real GLBs in `tests/fixtures/` exercise meshopt,
  quantization and KHR_texture_basisu in CI without the Vael checkout.
- Release workflow runs fmt/clippy/test/doc as a preflight and validates the
  tag against `[workspace.package].version`; `tools/publish.sh` is idempotent
  and waits for the exact version in the index.
