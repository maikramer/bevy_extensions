# bevy_ext_grass_jackdaw

Extensão do editor **Jackdaw 0.20** para o crate `bevy_ext_grass`: painel
**Relva**, categoria no inspector e ícones dos componentes autorados.

## O que faz

- Painel **Relva** (sidebar direita): estado (ligada/desligada, densidade,
  qualidade, nº de paletas), botões ligar/desligar, densidade −/+, e
  **Nova paleta de bioma**.
- Categoria **Relva** no inspector para `GrassSettingsSpec` e
  `GrassStyleSpec` (todos os campos com os widgets normais do editor).
- Ícone de árvore nas entidades de settings/paleta no Outliner.
- Cada escrita passa por `component.set`/`component.add` do editor: entra no
  undo e é gravada no `.bsn` (`world/settings/grass.bsn`). O jogo a correr
  relê o documento, como nas definições de ambiente/flora.

Sem preview de lâminas no viewport nesta versão: o viewport do editor não tem
o terreno cozido (heightmap/máscaras) que o `bevy_ext_grass` precisa para
semear os campos.

## Instalar

```bash
cd extensions/bevy_ext_grass_jackdaw
jd build
jd extension pack
jd extension install --trust-publisher bevy_ext_grass_jackdaw-*.jdext
```

Reiniciar o editor depois de instalar (a extensão declara `app_plugin`).

No projeto do jogo, ativar em `jackdaw.toml`:

```toml
[extensions]
bevy_ext_grass_jackdaw = true
```

Abrir `world.bsn` e o painel **Relva** aparece na sidebar direita; as
entidades de `world/settings/grass.bsn` ficam na pasta **Definições** do
Outliner.

## Notas

- O tipo `GrassSettingsSpec`/`GrassStyleSpec` é o mesmo crate que o jogo usa
  (`crates/bevy_ext_grass`), por isso os `type_path` batem certo.
- Se outra extensão também registar esses tipos, a última a carregar fica com
  o `type_path`; o painel lê por reflexão pelo caminho, como as outras
  extensões do Vael.
