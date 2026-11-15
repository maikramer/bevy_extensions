# bevy_ext_grass

Relva GPU para Bevy 0.20 sobre [`bevy_grass`](https://crates.io/crates/bevy_grass):
o jogo injeta o terreno e o layout, o crate faz o bake das máscaras, os
estilos por bioma, o vento e o trampling avian3d.

## O que faz

- **Bake genérico**: `GrassTerrain` (altura, regras de sítio, bioma) e
  `GrassFieldLayout` (retângulos dos campos) são implementados pelo jogo; o
  crate coze uma grelha estática a 4 m (água/estrada/pad/falésia/exclusão) e
  heightmaps + máscaras `R32Float`/`Rg8Unorm` a 2 m no
  `AsyncComputeTaskPool`. Trocar o `GrassProvider` ou a `revision()` do
  terreno refaz o bake (com debounce para edições vivas).
- **Estilos autorados**: `GrassSettingsSpec` e `GrassStyleSpec` são
  componentes `Reflect` (`type_path = "bevy_ext_grass"`) prontos para o
  Jackdaw (`extensions/bevy_ext_grass_jackdaw`) e para `world/settings/*.bsn`.
- **Bevy**: plugin, `GrassSystems`, sombras do sol, `NoGrass` automático em
  câmaras de render-para-textura, `GrassTint` para o ciclo dia/noite e
  `bevy_grass::GrassWind` re-exportado para o vento do jogo.
- **avian3d** (feature `avian`, default): `GrassTrampler` + `Collider` viram
  um `GrassInteractorFromCollider`; `GrassTramplerDisc` cobre quem não tem
  collider.

## Ligação num jogo

```rust
use bevy_ext_grass::*;

struct MyTerrain; // implementa GrassTerrain (height/site/revision)

app.add_plugins(GrassSettingsSpecPlugin)
    .add_plugins(GrassPlugin::default())
    .insert_resource(GrassProvider(std::sync::Arc::new(MyTerrain)));

// opcional: pontos que dobram a relva
commands.spawn((GrassTrampler, avian3d::prelude::Collider::capsule(0.3, 1.3)));
```

Sem `GrassProvider` não há campos (cenas de interior ficam sem relva sem
código extra). Sem `GrassSettingsSpec`/`GrassStyleSpec` na cena, usa o estilo
default do crate e o tier `Medium`.

## Features

| Feature | O que liga |
|---|---|
| `avian` (default) | trampling por collider avian3d |
| `debug`/`GRASS_DEBUG=1` | logs do bake e PGMs das máscaras em `/tmp` |

## Testes

```bash
cargo test -p bevy_ext_grass
```

Cobrem a matemática dos campos, o bake com um terreno sintético (incluindo
regras bloqueadas e bioma errado), o mapeamento dos specs e os settings.
