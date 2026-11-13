# vael_scene_jackdaw

Extensão do Jackdaw para a cena do Vael.

- **Outliner**: ícones por tipo de nó (árvore/rocha pela colheita, casa,
  personagem, luz, partículas, portal, entrada, spawner, exclusão, estrada…).
- **Inspector**: `ColliderSpec`/`BodySpec` na categoria *Physics*,
  `Destructible`/`Harvestable` em *Gameplay* e o `WorldNode` (o DSL que ainda
  não é tipado) em *DSL Vael*.
- **Gizmos** (View → "Vael: gizmos da cena", ou o menu de overlays do
  viewport): colisores da seleção ou a menos de 70 m da câmara, anéis de
  `SpawnExclusion`/`ScenePortal auto-radius`/`PointLight radius` e regiões de
  `DynamicSpawner`.
- **Painel "Vael: Cena"**: contagens de componentes tipados, nós ainda em DSL,
  problemas (modelo ou malha de colisão em falta, 0 golpes, 0 unidades,
  destrutível sem colisor) e "Próximo problema" (seleciona e enquadra).

- **Painel "Vael: Assets"**: os modelos `GltfScene` da cena por categoria
  (Plantas, Rochas, Edifícios, Criaturas, Props) com o nº de instâncias.
  Escolhido um, "Colocar" põe uma cópia por clique no viewport e "Pincel"
  espalha `densidade` cópias no `raio` ao clicar/arrastar (rotação e escala
  aleatórias). Esc ou "Parar" sai. Cada cópia é `entity.duplicate` de uma
  instância existente (herda colisor, corpo, destrutível, colheita, LOD),
  sem a proveniência do original, assente no `.ahgt` do `Terrain`; o traço é
  um passo de undo. `vael.assets.place_at` faz o mesmo por parâmetros (MCP).
- **Validação ao gravar**: depois de cada `scene.save` revê a cena e corre o
  `vael.analyze.run`; File → "Vael: validar e gravar" recusa com problemas.

Os tipos (`bevy_ext_scene`, `WorldNode`) chegam pelo schema do jogo; a
extensão lê-os por reflexão e não os regista (ver AGENTS.md do Vael: só um
dylib pode registar um tipo partilhado).

```bash
jd build && jd extension pack && jd extension install --trust-publisher vael_scene_jackdaw-*.jdext
```
