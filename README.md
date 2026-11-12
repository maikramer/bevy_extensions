# bevy_extensions

Bevy 0.20 extensions that fill gaps the engine does not cover: a GLB decoder
for compressed/quantized assets, a voxel/SDF terrain layer, the world format
(XML/BSN/vinst), save slots and shared contracts.

| Crate | What it does | Status |
|---|---|---|
| [`bevy_ext_core`](crates/bevy_ext_core) | markers, schedule sets, shared resources and deterministic RNG | 0.1.0 |
| [`bevy_ext_world`](crates/bevy_ext_world) | XML/BSN/**vinst** world formats, project/scene discovery, include/emit | 0.1.0 |
| [`bevy_ext_gltf`](crates/bevy_ext_gltf) | GLB decoder: meshopt, `KHR_mesh_quantization`, `KHR_texture_basisu`, UASTC RRR; GLB bounds and world-tiled textures | 0.1.0 |
| [`bevy_voxel_terrain`](crates/bevy_voxel_terrain) | voxel/SDF terrain core: CSG mods, caves, arches, bridges and Transvoxel LOD meshing | 0.1.0 |
| [`bevy_ext_terrain`](crates/bevy_ext_terrain) | Bevy scene contract for the voxel terrain: `.ahgt` heightfield adapter and feature components | 0.1.0 |
| [`bevy_ext_save`](crates/bevy_ext_save) | save envelope, slots and atomic JSON I/O | 0.1.0 |

`bevy_ext_gltf` and `bevy_voxel_terrain` are the flagship packages: the stock
Bevy 0.20 loader does not decode meshopt/quantized GLBs, and there is no
heightfield+SDF voxel terrain with Transvoxel LOD seams on crates.io.

## Quick start

> A 0.1.0 está preparada mas ainda não foi publicada no crates.io. Até ao
> `cargo publish`, consome os crates por `path` (ou por `version` + `path`)
> a partir deste repositório.

```toml
[dependencies]
bevy_ext_gltf = "0.1"
bevy_voxel_terrain = "0.1"
```

The GLB reader must be registered before `AssetPlugin`:

```rust
let mut app = App::new();
bevy_ext_gltf::register_asset_source(&mut app, vec!["assets".into()]);
app.add_plugins(DefaultPlugins);
```

## Versioning

All crates are released in lockstep at `0.x`. A Bevy upgrade is a breaking
change of the family: bump the minor and update the compatibility table below.
The Vael currently consumes the crates by `path` (with `version`), because the
family is not on crates.io yet; the lockfile pins Bevy 0.20.1.

| Family | Bevy | MSRV |
|---|---|---|
| 0.1.x | 0.20.1 | 1.97.1 |

## Publication order

The 0.1.0 release publishes five crates: `bevy_ext_core` → `bevy_ext_world` →
`bevy_ext_gltf` → `bevy_voxel_terrain` → `bevy_ext_save`. `bevy_ext_save`
depends on `bevy_ext_world`, so it only resolves after the latter is on the
crates.io index. `bevy_ext_terrain` is held for 0.1.1 while its scene contract
stabilizes; the Jackdaw extension is not a crate.

## Repository layout

- `crates/` — the packages above.
- `extensions/bevy_voxel_terrain_jackdaw/` — the official Jackdaw editor
  extension, a standalone project built with `jd build`/`jd extension pack`;
  `.jdext` bundles are release assets, not git files.
- `docs/PACKAGES.md` — roadmap, TypePath compatibility and the Vael migration
  waves.
- `tools/publish.sh` — token check and ordered publish (dry-run supported).

## License

MIT OR Apache-2.0.
