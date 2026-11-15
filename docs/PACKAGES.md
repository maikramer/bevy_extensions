# PACKAGES.md — monorepo `bevy_extensions`

Estado: 0.1.0 (2026-11-12). O repo é o lar durável dos pacotes; o Viber fica
como app/fachada transitória até o Vael deixar de o usar.

> Standard de engenharia (Vael + este repo):
> [`../../Vael/docs/DEV-STANDARD-STUDY.md`](../../Vael/docs/DEV-STANDARD-STUDY.md).

## Release 0.1.0

- Repositório: https://github.com/maikramer/bevy_extensions (`main` `074b533`).
- Testes: **209** — core 3, world 69+5 BSN, gltf 28, voxel 98, terrain 4,
  save 2. `cargo fmt`/`clippy --workspace --all-targets -D warnings` verdes.
- Dry-runs de publicação verdes: `bevy_ext_core`, `bevy_ext_world`,
  `bevy_ext_gltf`, `bevy_voxel_terrain`. `bevy_ext_terrain`/`bevy_ext_save`
  passam `package --list`; o publish deles só resolve depois de
  `bevy_voxel_terrain`/`bevy_ext_world` estarem no índice.
- **Publicação no crates.io pendente de token.** Depois de `cargo login`,
  correr `tools/publish.sh`; ou definir o secret `CARGO_REGISTRY_TOKEN` no
  GitHub e empurrar a tag `v0.1.0` (o workflow `release.yml` faz a ordem e o
  GitHub Release).
- `bevy_voxel_terrain_jackdaw` é WIP concorrente (API do Jackdaw) e fica
  fora do workspace/release 0.1.0.

## Pacotes 0.1.0

| Pacote | Conteúdo | Depende de |
|---|---|---|
| `bevy_ext_core` | `PlayerBody`, `MenusOpen`, `ScriptToast`, `AudioMixerSettings`, `UiSet`, `TerrainQuery`/`NavQuery`, `rng`, `script_kind` | bevy |
| `bevy_ext_world` | XML/BSN/vinst, parser, `include`/`emit`, `project`/`root_of`, `DeltaPatch` | bevy, `jackdaw_bsn` |
| `bevy_ext_gltf` | GLB decoder: meshopt, `KHR_mesh_quantization`, `KHR_texture_basisu`, UASTC RRR; bounds e texturas tiled | bevy, `meshopt` |
| `bevy_voxel_terrain` | `VoxelField`, mods CSG, caves/arches/bridges, `ModIndex`, Transvoxel, `HeightField` | bevy, `transvoxel` |
| `bevy_ext_terrain` | contrato de cena do voxel: `VoxelTerrain`/features, adapter `.ahgt` | `bevy_voxel_terrain` |
| `bevy_ext_save` | `SaveGame`, slots, paths, IO atómico e migração legacy | `bevy_ext_world` |
| `bevy_ext_grass` | relva GPU sobre `bevy_grass`: contrato `GrassTerrain`/`GrassFieldLayout`, bake de máscaras, `GrassSettingsSpec`/`GrassStyleSpec`, trampling avian3d | bevy, `bevy_grass`, `avian3d` |

`bevy_ext_world` tem uma feature opcional `terrain` (dep `bevy_ext_terrain`):
o loader devolve os componentes `VoxelTerrain`/`VoxelCave`/`VoxelArch`/
`VoxelBridge` à vista DSL do Viber, e o `bsn_migrate --terrain` faz a
migração inversa (âncora + `<Cave>` → componente) com gate de ida-e-volta por
valor. Desligada por defeito — a ordem de publicação core→world→… não muda;
o Viber e o `viber_tools_core` liguem-na. O `bsn_equiv` precisa dela para
comparar mundos com features em componente.

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
- Ordem: `bevy_ext_core` → `bevy_ext_grass` → `bevy_ext_world` →
  `bevy_ext_gltf` → `bevy_voxel_terrain` → `bevy_ext_terrain` →
  `bevy_ext_save`.
- `tools/publish.sh` faz a ordem, espera pelo índice e suporta `--dry-run`.
- Os nomes `bevy_ext_*`/`bevy_voxel_terrain` estão livres no crates.io a
  2026-11-12; reservar/publicar 0.1.0 antes de anunciar.

## Fecho progressivo do Viber

Estado a 2026-11-14: o Vael main ainda corre dentro de
`viber::app::build_app` (257 refs `viber::` em Rust, 32 módulos; 1.807
chamadas `viber.*` em Lua). O editor Jackdaw já não depende do `viber` e —
desde 2026-11-14 — **o jogo também não**: input (`jackdaw_input` +
`ExternalActionState`), animação (`jackdaw_animation_runtime` + `AnimationSet`),
edição viva (`editor_link`/`live_edit`/`jackdam_remote`), câmara/áudio specs
tipadas. A composição própria da App e a unificação dos gémeos estão
validadas nas branches `integrate2/all` (monorepo) e `exit2/app-assembly`
(Vael), à espera da água aterrar. O mapa vivo por peça é o
`Vael/docs/EXIT-MAP.md`; as fases F0–F4 no estudo do Vael. A ordem de corte:

| Wave | Objetivo | Gate | Estado |
|---|---|---|---|
| 2 | Vael usa `bevy_ext_core`/`world`/`save` diretamente; troca imports de contrato | `rg 'viber::'` a descer | ✅ (merge nas branches exit2) |
| 3 | Player/câmara/input no Vael; `player_camera:false` | QA de movimento/interiores/câmara/save | ✅ + Jackdaw toma input/animação/edição viva |
| 4 | Portar Lua para Rust; apagar `viber.vael`, installers e `mlua` | `rg 'viber\.' scripts` = 0 | em curso (qa.rs/`F2` do estudo; FSM 4b, diálogo 4d) |
| A | Aterrar `integrate2/all` + `exit2/app-assembly`: composição própria (`src/app.rs` do Vael), gémeos unificados (vitais/input_map/a11y/locale/dialogue no core; camera-glue/editor_camera/feedback/hud-assets no engine; save-bridge no `bevy_ext_save`; netplay reactivado) | 335 testes do Vael + QA vivo (feito nas branches) | 🔄 bloqueada na água do monólito |
| B/C | Consumir em vez de copiar: streaming/config/interact/spawn_gate(**avian**)/attachment/audio_space/save-adapter/hud-font; depois postfx(349)/nav(72)/roads/spawn(633, avian+gradmesh)/**ui data layer (troca atómica)** | check+test workspace; TypePaths intactos | 🔄 em curso (worktrees `exit3/*`) |
| D | Terreno/água → `bevy_ext_terrain_runtime` + `bevy_ext_water` (já avian) | portar DEPOIS de a água aterrar | 🚫 bloqueada |
| E | Port avian das cópias Rapier (`bevy_ext_physics`) | 50 testes no crate | 🔄 em curso |
| F | luau host → `bevy_ext_luau` (BMS) por call-sites (ui 256, load 149, toast 108, game 96…); `viber.ecs` morre (0 usos); `viber.lua` → adapter BRP; `viber.scene --live` reformado (Jackdaw ganhou) | testes portados; jogo sem `mlua` cru | ⏳ |
| 5 (H) | Remover `viber` do `Cargo.toml`; arquivar o monólito | `cargo tree -i viber` vazio; jogo main/menu/interiores/save/combate | 🚫 pós A+D+F |

Não remover `viber` do `Cargo.toml` antes do runtime/player/Lua; não desligar
`player_camera` antes do QA; não mudar os `TypePath` de wire na 0.x; trocas
de módulos de runtime são ATÓMICAS (o incidente dos gémeos de 2026-11-14:
`Option<Res<T>>` de um gémeo sem runtime falha em silêncio).

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

## Extensões oficiais do Jackdaw (2026-11-12)

O Vael deixou de usar o editor próprio (`vael-editor`) como fluxo. A autoria
é no **Jackdaw oficial** (`jd open .`); o que é nosso entra como extensão
`.jdext` assinada, num projeto standalone em `extensions/`.

| Projeto | O que é | Estado |
|---|---|---|
| `crates/bevy_ext_terrain` | Componentes runtime-safe `VoxelTerrain`/`VoxelCave`/`VoxelArch`/`VoxelBridge`, leitor `.ahgt` e `build_voxel_field` | 0.1.0, 4 testes |
| `extensions/bevy_voxel_terrain_jackdaw` | Extensão oficial: painel `Vael: Voxel Terrain`, operadores `vael.voxel.*`/`vael.preview.*`, preview progressivo do mundo (heightfield + paint, caixas, modelos reais e um mesh único para os distantes) | 0.1.12, `.jdext` instalado e carregado |

Ciclo: `jd build` → `jd extension pack` → `jd extension install
<bundle>.jdext --trust-publisher` → reiniciar o editor (extensões com
`app_plugin` ativam no arranque seguinte). `jackdaw check-extensions
--expect <id>` valida sem abrir janela.

Limite conhecido: o editor oficial regista o asset source antes de qualquer
extensão e o stock loader não lê `EXT_meshopt_compression`/KTX2. O preview
do mundo descodifica os GLBs do pool para `assets/public/.jackdaw-cache/glb`
e mostra o resultado; as cenas glTF reais ficam limitadas ao tier próximo. O
resto do mundo é caixa (perto) ou o mesh fundido (longe).
