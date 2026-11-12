# Changelog

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
  `.ahgt` heightfield adapter.
- `bevy_ext_world` now carries the full XML/BSN/project/include/emit layer.
