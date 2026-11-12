# PACKAGES.md — monorepo `bevy_extensions`

Estado: 0.1.0 (2026-11-12). O repo é o lar durável dos pacotes; o Viber fica
como app/fachada transitória até o Vael deixar de o usar.

## Pacotes 0.1.0

| Pacote | Conteúdo | Depende de |
|---|---|---|
| `bevy_ext_core` | `PlayerBody`, `MenusOpen`, `ScriptToast`, `AudioMixerSettings`, `UiSet`, `TerrainQuery`/`NavQuery`, `rng`, `script_kind` | bevy |
| `bevy_ext_world` | XML/BSN/vinst, parser, `include`/`emit`, `project`/`root_of`, `DeltaPatch` | bevy, `jackdaw_bsn` |
| `bevy_ext_gltf` | GLB decoder: meshopt, `KHR_mesh_quantization`, `KHR_texture_basisu`, UASTC RRR; bounds e texturas tiled | bevy, `meshopt` |
| `bevy_voxel_terrain` | `VoxelField`, mods CSG, caves/arches/bridges, `ModIndex`, Transvoxel, `HeightField` | bevy, `transvoxel` |
| `bevy_ext_terrain` | contrato de cena do voxel: `VoxelTerrain`/features, adapter `.ahgt` | `bevy_voxel_terrain` |
| `bevy_ext_save` | `SaveGame`, slots, paths, IO atómico e migração legacy | `bevy_ext_world` |

`bevy_voxel_terrain` é a exceção deliberada ao prefixo: o sistema é único e o
nome está livre. `bevy_voxel_terrain_jackdaw` é WIP concorrente (plugin do
editor Jackdaw) e fica fora do workspace/release até compilar.

## Compatibilidade de wire/reflexão

Os `TypePath` históricos ficam **congelados** nesta linha 0.x:

- `viber::menus`, `viber::music`, `viber::save`, `viber::project`,
  `viber::terrain::delta` (tipos extraídos);
- `bevy_world_xml::bsn` (`WorldNode`/`WorldProp`) — usado por 86 `.bsn` do
  Vael (71.085 referências);
- `bevy_world_xml::xml` e `bevy_engine_core` (defaults congelados em
  `XmlNode`/`NodeOrigin`/`XmlDocument`/`PlayerBody`).

Mudar qualquer um destes paths é uma release breaking separada, com migração
de `.bsn`, saves e clientes BRP.

## Versionamento e publicação

- Versão lockstep de toda a família (`0.1.0`).
- Bevy 0.20.1; MSRV 1.97.1. Subir Bevy é breaking da família.
- Ordem: `bevy_ext_core` → `bevy_ext_world` → `bevy_ext_gltf` →
  `bevy_voxel_terrain` → `bevy_ext_terrain` → `bevy_ext_save`.
- `tools/publish.sh` faz a ordem, espera pelo índice e suporta `--dry-run`.
- Os nomes `bevy_ext_*`/`bevy_voxel_terrain` estão livres no crates.io a
  2026-11-12; reservar/publicar 0.1.0 antes de anunciar.

## Fecho progressivo do Viber

O Vael ainda corre dentro de `viber::app::build_app`: 151 referências
`viber::` em Rust (29 ficheiros) e 1.637 chamadas `viber.*` em 79 scripts
Luau (11.684 linhas). O editor Jackdaw já não depende do `viber` (usa
`bevy_ext_gltf` + `bevy_ext_world`). A ordem de corte é:

| Wave | Objetivo | Gate |
|---|---|---|
| 2 | Vael usa `bevy_ext_core`/`world`/`save` diretamente; anexa `PlayerBody`/`VaelScriptTags`; troca imports de contrato | `rg 'viber::'` a descer; jogo/testes iguais |
| 3 | Player/câmara/input no Vael; `player_camera:false`; teleporte/aterragem próprios | QA de movimento, interiores, câmara, save, minimapa, cinemáticas |
| 4 | Portar Lua: `rules.lua` → verbos (combat/abilities/hotbar/equipment) → FSM/inimigos/bosses/spawns/loot/story/áudio → NPC/diálogo/NLU; apagar `viber.vael`, installers e `mlua` | `rg 'viber\.' scripts` = 0; `LuaScriptHost`/`mlua` fora do jogo |
| 5 | Trocar `build_app` por `DefaultPlugins + pacotes + VaelPlugin`; repor features do Bevy; remover `viber` do `Cargo.toml`; arquivar o monólito | `cargo tree -i viber` vazio; jogo `main`/`menu`/interiores/save/combate |

Não remover `viber` do `Cargo.toml` antes do runtime/player/Lua; não desligar
`player_camera` antes do QA; não mudar os `TypePath` de wire na 0.x.

## Próximos pacotes (depois da 0.1.0)

`bevy_ext_terrain` completo (AHGT/heightmap/splat/roads/water/render/plugin),
`bevy_ext_nav`, `bevy_ext_streaming`, `bevy_ext_anim`, `bevy_ext_fx`,
`bevy_ext_ui`, `bevy_ext_audio`, `bevy_ext_luau`, `bevy_ext_bridge`.

O critério é o do Vael/Viber: só extrair ou publicar o que tem valor próprio;
o que já tem crate pronto (`bevy_hanabi`, `bevy_kira_audio`,
`leafwing-input-manager`, `bevy-inspector-egui`, `bevy_tweening`,
`bevy_yarnspinner`) deve ser adotado em vez de reimplementado. Terreno
heightfield/SDF, água de lagos/rios, decals, streaming/LOD, animação/IK, UI
XML, save slots e profiler game-aware continuam sem equivalente no crates.io
e justificam os pacotes da família.
