# Changelog

## Não publicado

- `bevy_ext_fence` (new crate) + `bevy_ext_scene::FenceSpec`: a fence is an
  authored line — an `<Entity>` with `fence-points`/`fence-style` and optional
  overrides (`fence-*`, including `fence-open` for the leaf of a gate) — and
  the geometry is generated from rules: posts, rails, boards of random
  length, pickets, palisade, wattle, lattice, crossbuck, rope, gate. `layout`
  is pure and deterministic (splitmix64 seeded by the fence key); `mesh` is
  flat-shaded with metre UVs and vertex-colour weathering; one
  `StandardMaterial` per texture set reads the glTF ORM map (R occlusion, G
  roughness, B metallic at zero — Bevy 0.20's `metallic_roughness_texture`
  reads blue, so a grey roughness map in that slot would make the wood
  metallic); `FencePlugin` has an optional `physics` feature (one thin box per
  span, on a `RigidBody::Static`; a bare `Collider` was hit by rays but the
  hero walked through it — found in the live QA). `FenceStyle`/`FenceSettings`
  are Reflect components, authored in `world/settings/fences.bsn`. The loader
  lifts `fence-*` by key on any tag; `TYPE_PATHS` grows to 17. 41 tests +
  doctest. Found in the live QA and fixed: `fence-rails` above the preset's
  heights now distributes the rails evenly (it used to cap at the preset's
  count), and the gate angle is per instance.
- `extensions/bevy_ext_fence_jackdaw` (new): viewport preview built with the
  same crate (the shared types are read by reflection), a "Cercas" inspector
  category, a line gizmo and one `bevy_ext_fence.new.<style>` operator per
  preset. Builds, installs and loads (`jackdaw check-extensions`); the
  viewport preview has not been seen in an editor window yet.
- `bevy_ext_world::bsn::authors_tags` + the start-scene shader config: the
  menu start parsed the whole main scene (1.4-1.6 s of prefab resolution) only
  to read `<Sky>`/`<DayCycle>`/`<Water>`/`<Layer>` for the WGSL consts, and
  answered with the defaults when the world authors none (Vael's case: those
  moved to `bevy_elements`). A text scan over the document and every `.bsn` it
  references (RON `tag: "X"`, `IsA { source }`, `WorldProp { key: "src" }`, and
  `<Include>` for legacy `.xml`) answers in ~90 ms over 537 documents;
  `VIBER_MAIN_SHADER_CONFIG=parse` forces the old path. Measured: the menu
  boot's `main_scene.parse` went from 1577 ms to 87 ms. A guard test
  (`VAEL_ROOT`) fails if the world starts authoring one of the tags again.
- `bevy_ext_core::CharacterAnimator` gains the derived clip tables: the
  normalized names (`AnimState::ALL`/`index`, `resolve_normalized`) and the
  node each state resolves to, all computed once at bind. `node_for` is now an
  array read — it used to normalize the whole clip list on every call, and
  `play_state` runs per character per frame (two `String`s per clip per frame).
  `CharacterAnimator::new` builds the tables; `refresh_clip_tables` re-derives
  them after mutating the clip list. `#[reflect(ignore)]` on both fields.
- `bevy_ext_engine`: the hot-path env reads (`VIBER_LOD_SWAP_BUDGET`,
  `VIBER_LOD_SCAN_HZ`, `VIBER_FOLIAGE_BATCH`) are cached in a `OnceLock` —
  except under `cfg(test)`, where the environment changes mid-process and the
  cache would make a test's `set_var` sticky.
- New `bevy_ext_core::boot`: a process-wide boot trace (`start()`,
  `mark(name)`, `report()`). The loading veil's `mundo pronto em X s` only
  starts at the first `PostUpdate`; everything before it — the `.bsn` parse
  with prefab expansion, the world spawn, the terrain bootstrap, the flora
  pools — was invisible. The Vael log now prints the whole table once
  (`world.parse 1665 ms · terrain.bootstrap 4039 ms · first_frame 4916 ms ·
  veil 20825 ms · total 32153 ms`). `VAEL_BOOT_TRACE=0` silences the line; the
  measurement itself is a couple of `Instant::now()` per phase.
- New `bevy_ext_core::cinematic::StreamingBoost`/`LoadingStreaming`: the
  streaming boost now lives in one place. `bevy_ext_terrain_runtime` defined
  its own copy of `StreamingBoost` that nobody inserted, so its ×10 box budget
  (18 ms/frame) never ran; it now re-exports the core type. The terrain also
  boosts while `LoadingStreaming` is set (the veil is up) — measured, the
  post-bootstrap column wave held the veil for several seconds at the normal
  4-box budget. `VAEL_TERRAIN_BOOST=0` is the A/B arm.
- `bevy_ext_terrain_runtime`: the active streaming camera ignores offscreen
  `Camera3d` (flora impostor bakes, render-to-texture). "First camera in the
  query" changed when the 64 bake cameras died, which re-selected the LOD of
  the whole world right after the bakes — the second veil wave.
- `bevy_ext_terrain_runtime`: `to_bevy_mesh_owned` consumes the
  `ChunkMeshData` (the borrowed variant cloned 4-5 buffers per box, 943 boxes
  at Vael's boot) and the column collider bakes with `bake_into`, not `bake`.
- Dependencies: `cargo update` across the workspace (patch-only — the Bevy
  0.20.1 / Jackdaw 0.20.2 lines are already the latest): `bumpalo` 3.20.4,
  `luajit-src`, `notify-types` 2.3.1, `rand` 0.10.4,
  `voracious_radix_sort` 1.2.1 and `zstd` 0.13.5 (plus `litrs`, `log`,
  `ordered-float`, `ron`, `rustix`, `serde_json`, `smallvec`, `syn`,
  `tokio`, `uuid` and friends in the vendored
  `bevy_skeleton_modifiers` workspace). The remaining lockstep pins are
  upstream constraints, not ours: `geo 0.31` keeps `i_overlay` below 4.1,
  `mlua-sys 0.13` keeps `lua-src` below 551.1, and `parking_lot` stays on
  0.12 because the `Weak<Mutex<LuaContext>>` in `bevy_ext_luau` must be the
  same type as the one `bevy_mod_scripting` uses.
- `bevy_ext_gltf`: the decoder is now a **single pass**. Every size it needs is
  known from the JSON (`count × stride` per compressed view, `count ×
  components × 4` per widened accessor, `byteLength` per plain one), so the
  final offsets are assigned before any byte moves: one allocation, views
  decoded straight into it, widening written into a preallocated slice instead
  of four bytes at a time, one JSON parse instead of three (a substring probe
  answers "nothing to rewrite" without parsing), and no more deep clones of
  `bufferViews`/`accessors`. `decode_glb` returns `Cow`, so a plain GLB is no
  longer copied. Measured on the Vael pool (676 GLB, 2.3 GB): 2303 ms → 677 ms,
  p50 1.38 ms → 0.18 ms, p95 13.3 ms → 4.5 ms, peak RSS 122 MB → 69 MB; the
  in-process A/B against the old decoder is ×1.92.
- `bevy_ext_gltf` feature `legacy`: the pre-rewrite decoder kept as a
  byte-for-byte oracle (`tests` compare both on the fixtures and a pool sample;
  `examples/decode_bench --features legacy` A/Bs the whole pool). Off by
  default.
- `bevy_ext_gltf`: the asset reader streams non-glTF assets through instead of
  buffering every PNG/KTX2/OGG whole, shares one `Arc` of asset roots with
  `asset_file_exists` (the reader and the lookup used to diverge on a second
  registration) and memoizes existence checks (thousands of `stat`s per spawn
  become one per distinct URL).
- `bevy_ext_gltf::textures`: the mip chain is split from the sampler write, so
  a large image's chain is built on the compute pool and applied when ready
  (with a dimension/mip re-check for hot reloads); the box filter averages the
  four channels of a pixel as one `u32` (bit-identical to the per-channel
  version, tested). `BEVY_EXT_GLTF_MIPS=inline|off` for A/B.
- `bevy_ext_gltf`: `DecodeStats` + `GltfStatsPlugin` (`BEVY_EXT_GLTF_STATS=1`)
  report files, bytes in/out, CPU time, UASTC re-labels and unsupported files;
  the reader warns once, by name, on `KHR_draco_mesh_compression`, ETC1S/
  BasisLZ KTX2, sparse accessors and unknown `extensionsRequired` instead of
  letting them fail as an invisible mesh.
- `bevy_ext_gltf`: `GltfDecodeSettings` (`BEVY_EXT_GLTF_VALIDATE`,
  `_UASTC_RELABEL`, `_DEQUANTIZE`, `_MIPS`) makes the three workarounds
  explicit policy instead of literals; the package version goes back to the
  workspace one now that the monolith's lockfile constraint is gone.
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
