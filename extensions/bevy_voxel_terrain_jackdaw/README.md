# bevy_voxel_terrain_jackdaw

Jackdaw editor extension for [`bevy_voxel_terrain`](../../crates/bevy_voxel_terrain) and
the scene components in [`bevy_ext_terrain`](../../crates/bevy_ext_terrain).

It adds, inside the Jackdaw editor:

- a **Vael: Voxel Terrain** dock panel listing the scene's `VoxelTerrain`
  entities and their authored features, plus the state of the world preview;
- buttons that add a `VoxelCave`/`VoxelArch`/`VoxelBridge` child node through
  the editor's own `entity.add.group` + `component.add` operators (undo and
  save included);
- a viewport **preview** of the authored scene:
  - the `WorldNode` `Terrain` is drawn from its `.ahgt` heightfield, with the
    `.paint` layers (snow, stone, dirt roads, …) tinting the vertices;
  - every model near the viewport is an AABB box, swapped for the real decoded
    glTF scene while it is large enough on screen;
  - everything farther away is folded into one merged box mesh, so the whole
    world stays visible at overview for a single draw call.

The preview is progressive: the scene scan uses change ticks, and the
spawn/upgrade/despawn work has per-frame budgets. Detail follows the apparent
pixel size, so an orbit camera zoomed out does not try to spawn the world.

## World preview

The world preview is separate from the voxel/CSG preview in `preview.rs`
(which only runs for a `VoxelTerrain` component; the Vael world uses the
`WorldNode` `Terrain` instead).

- **Scan**: every 0.5 s, only `WorldNode`s whose component ticks changed are
  reflected. Descendants of a `<Scatter>` recipe are excluded: their identity
  `Transform` is a recipe placeholder, not a placed prop. The generated
  instances (`spawn/grp-*`) are siblings and stay previewable.
- **Bounds**: only the GLB's JSON chunk is read (`glb_bounds::read_json_chunk`)
  and the result is cached by source. LOD1 is used when the file is on disk;
  a fresh clone without the derived LODs falls back to the base model.
- **Tiers**: `Real` (decoded glTF) when the model's AABB covers at least
  ~14 px, `Box` above ~0.6 px, otherwise `Far` (the merged mesh). Hysteresis
  is 0.8×/1.25× so nodes do not flip at a threshold. Caps: 700 individual
  previews, 120 real scenes, 48 box spawns, 4 upgrades and 64 despawns per
  frame.
- **Terrain**: an adaptive grid (~8 m per vertex, 129²–513²) built on the
  background task pool. The `.paint` sidecar is read next to the `.ahgt`
  (`terrain.ahgt.paint`) and blended with the canonical flat layer colours.
- **Framing**: the editor camera starts at the world origin, which is *under*
  the Vael terrain (the city sits around y≈40). Once the terrain preview
  exists — or after 3 s, if the heightmap failed — the extension calls
  `view.frame_all` once so the scene opens on the world overview. The panel
  has an **Enquadrar mundo** button and Home does the same afterwards.

The preview entities carry `EditorHidden`, `SkipSerialization` and
`EditorEntity`, and are not part of the scene document, so they never enter
the saved `.bsn`.

QA gates (read once at startup):

| Variable | Default | Meaning |
|---|---|---|
| `VAEL_PREVIEW` | `1` | `0` disables the model previews (the terrain stays) |
| `VAEL_PREVIEW_REAL_PX` | `14` | promotion threshold to the real model |
| `VAEL_PREVIEW_BOX_PX` | `0.6` | threshold below which the node is `Far` |
| `VAEL_PREVIEW_MAX_LIVE` | `700` | individual previews cap |
| `VAEL_PREVIEW_MAX_REAL` | `120` | real glTF scene cap |

The View menu has `Vael: modelos reais / caixas` and `Vael: preview do mundo`;
Tools has rebuild entries for the voxel preview and the world preview. The
panel also shows the live counts and the terrain status.

## Why not `jackdaw_editor_terrain`?

The core Jackdaw terrain tools are heightmap tools (`TerrainDataStore` is a
grid of `f32` heights). Voxel terrain has a second dimension and a different
sidecar; this extension models the panel and preview on the same patterns but
does not pretend the heightmap store fits.

## Usage (official editor)

This is a standalone official Jackdaw extension project (`jackdaw_extension`,
its own `[workspace]` and `jackdaw.toml`). Build and install it as a signed
bundle:

```bash
jd build
jd extension pack
jd extension install bevy_voxel_terrain_jackdaw-*-x86_64-unknown-linux-gnu.jdext \
  --trust-publisher
jackdaw check-extensions --expect bevy_voxel_terrain_jackdaw
```

The extension declares an `app_plugin`, so the running editor must be
restarted once after install. Select a `VoxelTerrain` entity, then use the
**Vael: Voxel Terrain** panel or the inspector. The component type paths are
stable (`bevy_ext_terrain::voxel::*`) and are the ones written to `.bsn`.

The official editor's stock glTF loader does not understand the Vael pool's
`EXT_meshopt_compression`/`KHR_texture_basisu`. The world preview decodes
those GLBs in the background into `assets/public/.jackdaw-cache/glb` and loads
the decoded copy; the original asset still fails in the stock loader, which is
why the log shows the loader errors for the pool paths. The voxel preview
itself does not depend on the pool.

## License

MIT OR Apache-2.0.
