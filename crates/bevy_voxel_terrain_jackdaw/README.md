# bevy_voxel_terrain_jackdaw

Jackdaw editor extension for [`bevy_voxel_terrain`](../bevy_voxel_terrain) and
the scene components in [`bevy_ext_terrain`](../bevy_ext_terrain).

It adds, inside the Jackdaw editor:

- a **Vael: Voxel Terrain** dock panel listing the scene's `VoxelTerrain`
  entities and their authored features;
- buttons that add a `VoxelCave`/`VoxelArch`/`VoxelBridge` child node through
  the editor's own `entity.add.group` + `component.add` operators (undo and
  save included);
- a viewport **preview**: the heightfield plus the authored CSG mods are
  meshed with `build_voxel_mesh` around the viewport camera and drawn as
  `Mesh3d` children.

The preview is a single-LOD, camera-local patch; the game runtime owns
streaming, collisions, paint, water and roads.

## Why not `jackdaw_editor_terrain`?

The core Jackdaw terrain tools are heightmap tools (`TerrainDataStore` is a
grid of `f32` heights). Voxel terrain has a second dimension and a different
sidecar; this extension models the panel and preview on the same patterns but
does not pretend the heightmap store fits.

## Usage

Add the plugin after the editor's own plugins:

```rust
app.add_plugins(bevy_voxel_terrain_jackdaw::VoxelTerrainEditorPlugin);
```

Select a `VoxelTerrain` entity, then use the panel or the inspector. The
component type paths are stable (`bevy_ext_terrain::voxel::*`) and are the
ones written to `.bsn`.

## License

MIT OR Apache-2.0.
