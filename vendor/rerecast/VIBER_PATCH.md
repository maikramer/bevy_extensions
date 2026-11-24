# rerecast 0.5.0 — patch do Viber

Cópia de `rerecast` 0.5.0 (crates.io, 2026-10-26, MIT OR Apache-2.0) ligada por
`[patch.crates-io]` no `Viber/Cargo.toml`. Só `src/contours.rs`, `src/poly_mesh.rs` e
`src/detail_mesh.rs` diferem do original; cada alteração está marcada com
`Viber patch:`.

1. **Triangulação falhada já não aborta a navmesh.** O Recast C++ devolve os
   triângulos feitos até ali (`return -ntris`) e o `rcBuildPolyMesh` só loga um
   warning — um contorno dobrado sobre si próprio deixa um buraco pequeno. O
   port Rust transforma isso em `PolygonNavmeshError::InvalidContour`, que
   deita fora a navmesh inteira. No `simple-rpg` o tile de 256 m do spawn
   falhava de forma determinística (qualquer deslocamento de grelha e qualquer
   `max_simplification_error`): nenhuma criatura tinha navmesh em toda a
   sessão.
2. **`len` das diagonais em `u32`.** O quadrado do comprimento era truncado com
   `as u16`: qualquer diagonal com mais de 255 células dava a volta e o ear
   clipping escolhia orelhas arbitrárias (triângulos compridos e finos). Com
   tiles de 256 m e células de ~0,5–1 m, isto é o caso normal, não a exceção.

Verificado a 2026-11-11: o upstream **0.5.0** (a versão publicada depois do
Bevy 0.20) continua com `as u16` e com o `Err(InvalidContour)` — nenhum dos
dois fixes entrou. Remover quando o upstream aceitar o equivalente (e apagar a
entrada `[patch.crates-io]`).

Deltas de vendor em relação ao crate publicado:

* `bevy_reflect` 0.20.0 → **0.20.1** e glam **0.33.2** (a versão do
  `bevy_math` 0.20.1; sem patch de git, o `^0.33` upstream já resolveria
  igual);
* `tests/` e as dev-dependencies removidas (nunca se constrói os testes do
  fork e eram elas que reancoravam o lock).

3. **Orientação do detail mesh garantida** (`src/detail_mesh.rs`,
   `clockwise_xz`). A triangulação do detail mesh deixa às vezes um sliver
   quase colinear com a orientação trocada por ruído de f32; o `landmass`
   valida todos os triângulos (`The triangle at index N in the height mesh is
   clockwise instead of counter-clockwise`) e deita fora a navmesh do tile
   INTEIRO — no Vael (2026-11-12) um tile ficava sem navegação em todas as
   sessões. O triângulo trocado vira-se (B↔C, flags AB↔CA).
4. **Triângulos invertidos fora da poly mesh** (`src/poly_mesh.rs`,
   `wound_right`). Com a triangulação recuperada (patch 1 e o diagonal
   "loose"), um triângulo invertido fundia-se num polígono côncavo e o
   `landmass` rejeitava o tile (`The polygon at index N is concave or has
   edges in clockwise order`). Os colineares podem participar na fusão para preservar vértices partilhados
   na borda. A fusão exige uma aresta em sentidos opostos e convexidade em
   coordenadas finais f32. Um triângulo de área zero que sobrar é descartado
   antes da construção das adjacências e do detail mesh.
5. `poly_mesh.rs`: mover `mesh.polygons.truncate` para depois do ciclo de
   `remove_vertex` (com guarda contra estouro se a checagem de convexidade
   impedir a re-fusão), corrigir subfluxo de `i -= 1` quando o vértice `0` é
   removido, corrigir a iteração circular de arestas em `remove_vertex` e
   `can_remove_vertex` (`prev(j, nv)` em vez de `nv - 1..`, avanço do ciclo
   de arestas e `edges.iter().take(nedges)`), e guardar `len < 3` em
   `convex_world_ring` e `npolys == 0` nos ciclos de fusão (`Task polled
   after completion` em tiles do mundo).
6. `detail_mesh.rs`: `DetailNavmesh::new` passava o buffer local de sub-malha
   `&verts` (`[Vec3A; 256]`) em vez de `&mesh.vertices` (`&[U16Vec3]`) para
   `HeightPatch::get_height_data` / `seed_array_with_poly_center`. Quando um
   polígono caía no seed pelo centro (`RegionId::NONE` ou região vazia no
   patch) com índice de vértice global `>= 256`, panicava com `index out of
   bounds: the len is 256 but the index is ...` (também reproduzido ao vivo no
   tile `#1`). Corrigida a assinatura para `&[U16Vec3]` passando `&mesh.vertices`,
   com guardas de borda em `chf.cells`, retorno antecipado quando não há span
   inicial e preenchimento/guarda de `hull` em `triangulate_hull` para
   `sample_distance <= 0`.
7. **`AREA_BORDER` igual dos dois lados de uma fronteira de regiões**
   (`src/contours.rs`, `walk_contour_build`) — defeito latente do upstream,
   **não** a causa do erro do Vael de 2026-11-23 (esse é o #8). O original
   compara a área do vizinho com a área do span onde o contorno **começou**. O
   `merge_and_filter_regions` funde regiões pequenas sem olhar à área, por isso
   uma região pode misturar células de estrada e de chão; na parede com o
   vizinho, o lado que começou na estrada marcava `AREA_BORDER` e o outro não, e
   o `simplify_contour` — que só tessella arestas exteriores ou de fronteira de
   área — deixava os dois lados com vértices diferentes. Compara-se agora com a
   área do span **actual**. No jogo as áreas são pintadas **antes** das regiões
   (que nascem com uma só área) e a fusão raramente as mistura, por isso isto
   não muda nenhum tile medido; num mundo sintético onde todas as regiões são
   mistas, a assimetria aparecia em ~43% dos tiles. Testes:
   `contours::viber_patch_tests`. O upstream **0.5.1** (2026-11-23) compara
   ainda com o span inicial (`src/contours/mod.rs`, `walk_contour_build`).
8. **Nenhuma aresta com mais de dois donos** (`src/poly_mesh.rs`,
   `cull_overused_edges`). É o que fez o `landmass` recusar tiles do Vael a
   2026-11-23 (`The edge made from vertices X and Y is used by more than two
   polygons`). O `simplify_contour` parte dos vértices obrigatórios de cada
   região — os pontos onde muda o VIZINHO dela — e refina cada corda pelo erro.
   Duas regiões que partilham uma parede partem de cordas diferentes quando a
   fronteira de uma delas é interrompida por terceiros; uma fica com um vértice
   a mais no troço. Se esse vértice é convexo, a orelha `(vizinho, vértice,
   vizinho)` tem a corda por base; quando não se funde com o corpo (este já
   chegou ao máximo de vértices), a corda fica com a orelha, com o corpo e com
   o polígono do vizinho. Depois de todos os polígonos estar montados, cada
   aresta em excesso perde o polígono de **menor área** até ter dois donos: é a
   fenda, com até `max_error` de profundidade, e a sua área já está coberta pelo
   vizinho — não abre buraco e as duas regiões ficam ligadas pela corda.
   Reproduzido com o tile real (capturado por `VAEL_NAV_DUMP`, canto
   `(-40.4, 54.8)`: a estrada 368 e o chão 365) e fechado com
   `poly_mesh::viber_patch_tests::an_edge_keeps_at_most_two_owners_losing_the_sliver`.
   As paredes simplificadas de forma diferente continuam a existir (o upstream
   não garante o contrário): quando não partem a validação, deixam as duas
   regiões desligadas nesse troço — é uma dívida da navegação, não deste patch.
