# bevy_voxel_terrain

Voxel/SDF terrain for Bevy: the 3D shape layer that a heightfield cannot
represent.

The heightfield is **demoted to a term** of the signed-distance field:

```text
density(p) = base(p) ⊕ mods(p)      // negative = solid
base(p)    = p.y - height(p.x, p.z)
```

Everything the 2.5D terrain does well keeps working; the voxel layer adds
vertical cliffs, overhangs, caves, tunnels, bridges and arches. The CSG mods
(box/capsule/ellipsoid/oriented-box/round-cone/arch) are solids, not carves, so
a column can have more than one surface. `ModIndex` answers "does anything 3D
touch this column?" in O(1), and the Transvoxel mesher closes LOD seams with
transition cells.

```toml
[dependencies]
bevy_voxel_terrain = "0.1"
```

```rust
use bevy_voxel_terrain::{HeightField, VoxelField, build_voxel_mesh};
```

## What it contains

- `HeightField` — the base sampler trait; the host app owns the AHGT/procedural
  grid and implements it.
- `VoxelField` / `Span` — the layered SDF and column queries.
- `Mods` — `BoxMod`, `CapsuleMod`, `EllipsoidMod`, `HalfEllipsoidMod`,
  `OrientedBoxMod`, `RoundConeMod`, `ArchMod`, `ModOp` and `Bounds3`.
- `ModIndex` / `ChunkClass` — the O(1) column index.
- `CaveSpec`, `ArchSpec`, `BridgeSpec`/`BridgeStyle` — pure feature specs.
- `paths` — Chaikin smoothing, resampling and path length.
- `ChunkMeshData` / `TintParams` / `build_voxel_mesh` / `transition_sides` —
  the Transvoxel mesher and the shared height/slope tint.
- `VOXEL_CHUNK_CELLS` / `VoxelChunkParams` — chunk/LOD parameters.

The ECS spawner, colliders, water/road authoring and the concrete `BrushGrid`
stay in the host engine or in the scene layer (`bevy_ext_terrain`). This crate
is the runtime-agnostic core.

## Bevy version

Built against Bevy 0.20.1 with `default-features = false` (math only).

## License

MIT OR Apache-2.0.
