# bevy_ext_nav

Navmesh navigation for Bevy games on large voxel worlds: one moving navmesh
tile baked in the background, landmass agents with local avoidance, and path
queries — with a hard rule that failure degrades to the old straight-line
behaviour, never to a character stuck.

The stack is `bevy_rerecast` (Recast voxeliser) + `landmass_rerecast` (bridge)
+ `bevy_landmass` (A*, RVO avoidance, per-polygon cost).

- `NavPlugin` — registers the generator, the archipelago and the bridge; the
  whole pipeline runs in `PostUpdate` between the locomotion producers and the
  consumer.
- `NavConfig` — tunables from `<NavMesh …>` in the world (`NavMeshTag`) and
  `VIBER_NAV*` env overrides (`VIBER_NAV=0` turns the stack off).
- `tile` — the single navmesh tile: `retile_navmesh`, retry back-off with
  sub-cell re-rasterisation, road/pad convex volumes, `NavTile`/`NavDirty`
  state.
- `backend` — the geometry backend: ground lattice sampled from the
  [`NavGround`] contract (topmost voxel surface, water/cliff holes) plus
  static Rapier colliders as obstacles.
- `agent` — `NavProfile` (civil/wild pricing), `NavArchipelago`, the
  push/pull bridge systems and `attach_nav_agents`.

## Contratos com o runtime

A crate não sabe que o mundo é voxel nem quem conduz os personagens:

- **`NavGround`** — o terreno: alturas (superfície renderizada e topo sólido
  do voxel), água, estradas, praças, falésias. O runtime implementa-o sobre o
  seu runtime de terreno e publica-o num `NavGroundHandle` (`Arc`); sem o
  handle a navegação fica adormecida.
- **`NavMeshTag`** — os atributos de `<NavMesh …>` que o assentamento do
  mundo já lê do XML.
- **`WorldSpawned`** — set onde o sistema que assenta o mundo se junta no
  `Startup` e no `SceneLoad`; a navegação aplica a tag depois dele.
- **`bevy_ext_core`** — a locomoção (`AiLocomotion`, `AiLocomotionSystems`),
  os marcadores de gameplay (`Player`, `Hostile`, `EnemyCreature`,
  `VoxelCollider`, `RagdollBodyOf`) e o schedule `SceneLoad`.

Pontos que faltam ao runtime do monólito `viber` para consumir esta crate
(marcados `TODO(viber-exit)` no código): implementar `NavGround` sobre
`TerrainRuntime` e inserir o `NavGroundHandle`, inserir o `NavMeshTag`,
juntar o spawn do mundo ao set `WorldSpawned` e re-exportar os contratos de
`bevy_ext_core` nos módulos originais.

## Bevy version

Built against Bevy 0.20.1 with `default-features = false` (std, asset and log
only).

## License

MIT OR Apache-2.0.
