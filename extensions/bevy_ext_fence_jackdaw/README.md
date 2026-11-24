# bevy_ext_fence_jackdaw

Extensão do editor **Jackdaw 0.20** para o crate `bevy_ext_fence`: preview das
cercas no viewport, categoria no inspector e painel **Cercas**.

## O que faz

- **Preview no viewport**: cada entidade com `FenceSpec` ganha filhos
  ocultos (`EditorHidden`/`SkipSerialization`) com a malha que o jogo vai
  gerar — o mesmo `layout` + `mesh` do crate, com as texturas do pool. Mover
  um ponto ou mudar o estilo refaz a cerca (varredura de 0,5 s). Sem chão: o
  viewport do editor não tem o terreno do jogo, e os postes ficam à cota
  autorada (no jogo eles descem até ao chão).
- **Gizmo**: a cerca selecionada mostra a sua linha e os pontos.
- Categoria **Cercas** no inspector: `FenceSpec`, `FenceStyle` e
  `FenceSettings` com os widgets normais do editor.
- Painel **Cercas** (sidebar direita): estado e um botão por predefinição
  (`world/settings/fences.bsn`) que cria uma linha nova de 6 m com esse estilo.
- Cada escrita passa por `entity.add.empty`/`component.add`/`component.set`:
  entra no undo e é gravada no `.bsn`.

Os componentes são lidos por reflexão (pelo caminho do tipo): cada dylib tem a
sua cópia dos tipos e só um os regista no editor.

## Instalar

```bash
cd extensions/bevy_ext_fence_jackdaw
jd build
jd extension pack
jd extension install --trust-publisher bevy_ext_fence_jackdaw-*.jdext
```

Reiniciar o editor depois de instalar (a extensão declara `app_plugin`).

No projeto do jogo, ativar em `jackdaw.toml`:

```toml
[extensions]
bevy_ext_fence_jackdaw = true
```
