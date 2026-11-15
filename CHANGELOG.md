# Changelog

## Não publicado

- New `bevy_ext_grass`: GPU grass fields over `bevy_grass` with a
  game-provided `GrassTerrain`/`GrassFieldLayout`, async mask/heightmap baking
  (static 4 m rules + 2 m dynamic slope/altitude), authored
  `GrassSettingsSpec`/`GrassStyleSpec` components (Jackdaw-ready) and avian3d
  trampling (`GrassTrampler`). Companion editor extension
  `extensions/bevy_ext_grass_jackdaw` adds the "Relva" panel and inspector
  category.
- New `bevy_ext_world` optional feature `terrain` (dep `bevy_ext_terrain`):
  the BSN loader renders `VoxelTerrain`/`VoxelCave`/`VoxelArch`/
  `VoxelBridge`/`VoxelCliff` components back into the DSL view (`<Terrain>`
  attr merge, feature child entities become `<Cave>`/`<Arch>`/`<Bridge>`/
  `<Cliff>` nodes with `<Chamber>`/`<Shaft>` children), so editor-authored
  terrain reaches the game runtime. Off by default: the publish order stays
  core→world→…
- `bsn::compare`: cave/chamber/shaft/arch/cliff attrs compare by value with
  absent-⇔-spec-default awareness (`feature_attr_default`), booleans compare
  as booleans (`talus="1"` == `talus="true"`) and the arch `width`/`span`
  spellings normalize to the same key — the canonical component form
  (`"2"`, omitted defaults) is equivalent to the authored DSL (`"2.0"`,
  explicit defaults).
- `bsn_migrate --terrain` (requires the feature): lifts the `<Terrain>`
  anchor (`VoxelTerrain`) and `<Cave>`/`<Arch>`/`<Cliff>` nodes (caves with
  chambers/shafts) into components, only when the round-trip is value-exact;
  idempotent, with a per-node skip report.
- `bsn_equiv`: siblings pair by `(tag, name)` instead of the full canonical
  key — attr-set differences (canonical component form) no longer reorder
  siblings and mismatch pairs.
- New `bevy_ext_terrain::voxel::VoxelCliff`: scene-contract carrier for
  authored cliffs (the band still resolves host-side against the carved
  grid — no `VoxelFeature` impl, no CSG preview). `bevy_ext_terrain` also
  gains `register_types` for standalone loaders;
  `VoxelChamber`/`VoxelShaft` now default to the `CaveChamberSpec`/
  `CaveShaftSpec` values instead of zeros; `bsn::feature_attr_default`
  exposes the DSL defaults.
- New `bevy_ext_world` example `bsn_view`-style debug helper (`_debug_view`):
  prints the DSL view of a `.bsn` for bridge debugging.
- `bevy_ext_scene::primitives`: the primitive contract shared by the game, the
  headless tools and the Jackdaw editor — `Shape` (rounded boxes, cones,
  pyramids, tori, lathes, sweeps, extruded polygons), `MaterialSpec` with the
  semantic `Preset`, lenient attribute parsing, bounds and the
  `PrimitivePart` bridge. Render-free.
- New `bevy_ext_gradmesh`: the `bevy_gradmesh` render half (mesh with a
  per-object vertical gradient baked into vertex colours, surface presets,
  and — with the `physics` feature — exact Rapier colliders, trimesh for the
  procedural shapes). Used by the Vael game and the Jackdaw preview.
- `bevy_ext_world::xml::values`: `parse_vec3_list` and `parse_vec2_groups`
  for sweep paths and extruded-polygon holes.

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
