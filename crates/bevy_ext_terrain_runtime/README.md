# bevy_ext_terrain_runtime

Runtime Bevy de terreno volumétrico: o heightfield é INPUT do SDF, toda a
superfície é colunas voxel meshadas por **transvoxel** (células de transição
de LOD, sem saias) com ladder de LOD por coluna e collider trimesh **por
coluna** assado no swap atómico do LOD.

- `TerrainFeaturesPlugin` — bootstrap one-shot: heightmap (PNG/`.ahgt`/
  procedural) → carve de features (pads, lagos, rios com cachoeiras, cuts,
  plateaus, estradas com pontes, cliffs 3D, `<RockFeatures>`) → mods do
  campo voxel → colunas dentro do raio de render, água e ribbons.
- `TerrainPlugin`/`TerrainSet` — ladder de LOD por coluna com histerese e
  clamp 2:1, construção staged sob orçamento de caixas por frame, cull por
  `render-distance`, respawn à aproximação, rebake por revisão de edição.
  O orçamento multiplica enquanto `bevy_ext_core::cinematic::StreamingBoost`
  vale (corte de cinemática, pós-respawn) ou `LoadingStreaming` (o véu está em
  cima) — `VAEL_TERRAIN_BOOST=0` tira só o de carga, para A/B.
  A câmara que ancora o streaming é a de JANELA (a de editor, quando existe):
  câmaras offscreen (bakes de impostores, render-to-texture) não contam nem
  forçam re-LOD quando nascem ou morrem.
- `TerrainRuntime` — registry de gameplay: `sample`/`sample_mesh_surface`/
  `surface_below`, `in_water`/`on_road`, água (`WaterBody`), estradas
  (`RoadPath`), splat blend de 13 camadas (`splat` + `layer_material`
  bindless), pintura (`paint`) e edições vivas (`delta` — overlay
  `DeltaGrid`, nunca muta a grelha).
- Colliders voxel: trimesh por coluna (`host::ColumnColliderBake`,
  `FIX_INTERNAL_EDGES`), banda de colisão com piso de LOD 0.

Depende de `bevy_voxel_terrain` (núcleo SDF/transvoxel), `bevy_ext_terrain`
(contracto de cena), `bevy_ext_core` (contratos `Player`/rng) e
`bevy_ext_gltf` (texturas world-tiled). Os `TypePath` históricos
`viber::terrain::…` estão congelados nos tipos reflectidos. Os pontos de
integração com o host (marcadores de cena, âncoras de streaming, céu/tempo,
partículas) estão isolados no módulo `host`, com o contrato de adoção
documentado em `TODO(viber-exit)`.

Parte da família `bevy_extensions` (lockstep 0.1.0, Bevy 0.20).
