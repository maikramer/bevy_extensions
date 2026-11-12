# bevy_ext_terrain

Scene-side contract for the `bevy_voxel_terrain` core: the components a
`.bsn` scene carries, the `.ahgt` heightfield adapter, and the bridge from
authored features to `VoxelField`.

The core crate stays runtime-agnostic. This crate owns the parts that need
Bevy ECS/reflection and that both the game and the Jackdaw editor share:

- `VoxelTerrain` — the terrain entity: `.ahgt` path, extents, voxel size.
- `VoxelCave`, `VoxelArch`, `VoxelBridge` — child entities, one feature each,
  with `Reflect` mirror fields and conversions to the core specs.
- `AhgtHeightField` — reads the `.ahgt` u16 grid and implements `HeightField`.
- `VoxelFeature` / `build_voxel_field` — collect the authored children into a
  `VoxelField` over the heightfield.

```ron
// .bsn (concept)
voxel_terrain::VoxelTerrain {
    heightmap: "world.data/terrain.ahgt",
    world_size: 4000.0,
    max_height: 200.0,
}
```

## Feature status

`VoxelCave`, `VoxelArch` and `VoxelBridge` map to the specs the core already
exposes. Cliff bands are not in the core yet; the component arrives when the
core publishes `CliffBand`/`build_cliff_mods`.

## Bevy version

Built against Bevy 0.20.1 with `default-features = false` (ECS/reflect/math
only).

## License

MIT OR Apache-2.0.
