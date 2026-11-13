# vael_jackdaw

Extensão do Jackdaw 0.20 para o Vael: três janelas na sidebar direita.

| Janela | O que faz | Preview no editor |
|--------|-----------|-------------------|
| **Vael: Ambiente** | hora e clima de arranque (`ElementsSettings`) | sol direcional na posição astronómica da hora/data/latitude |
| **Vael: PostFX** | liga/desliga efeitos e a qualidade do SSAO (`PostFxSettings`) | "lente do jogo": bloom, vinheta e aberração na câmara do viewport |
| **Vael: Flora** | espécies (`FloraSpecies`), nova espécie | fila de variantes reais (`bevy_flora`) e árvores nas estações `prop-flora` |

As definições são componentes `Reflect` (crates `bevy_ext_elements`,
`bevy_ext_postfx`, `bevy_ext_flora`) em `world/settings/*.bsn`; o inspector
edita qualquer campo e os painéis escrevem pelo operador `component.set`
(undo + `.bsn`). O jogo relê os documentos ao gravar.

O `WorldNode` é registado pela `bevy_voxel_terrain_jackdaw`; esta extensão
lê-o por reflexão (duas cópias do mesmo tipo em dylibs diferentes partem o
registo).

```bash
jd build
jd extension pack
jd extension install vael_jackdaw-*-x86_64-unknown-linux-gnu.jdext --trust-publisher
```

Ativar no projeto: `[extensions] vael_jackdaw = true` no `jackdaw.toml`.
