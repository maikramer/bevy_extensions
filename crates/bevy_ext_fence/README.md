# bevy_ext_fence

Cercas procedurais para Bevy 0.20: uma linha autorada no `.bsn` vira postes,
travessas, tábuas, paliçada, varas entretecidas, treliça, cruzetas, cordas ou
um portão — com o look de *Ofício Talhado* (facetas planas, chanfros a enxó,
desgaste na base).

## Uso

```rust
use bevy_ext_fence::FencePlugin;

app.add_plugins(FencePlugin);
// O chão (o jogo amostra o terreno; sem ele fica a cota autorada):
ground.0 = Some(std::sync::Arc::new(MyGround));
```

Uma cerca é um `<Entity>` com `bevy_ext_scene::FenceSpec`:

```ron
<Entity name="cerca-oeste" translation="-18.5 8.27 94.2"
        fence-style="pasture_rail" fence-points="-24.6 0 0; 5.9 0 0" />
```

`fence-style` referencia um [`FenceStyle`] (em `world/settings/fences.bsn`,
editável no inspector); os restantes `fence-*` são sobreposições da cerca:
`fence-kind`, `fence-height`, `fence-rails`, `fence-boards`,
`fence-board-length`, `fence-spacing`, `fence-material`, `fence-closed`,
`fence-seed`, `fence-collider`, `fence-follow`.

## Camadas

| Módulo | O que faz |
|---|---|
| `layout` | A linha vira postes e peças. Puro e determinístico (splitmix64 pela chave da cerca). |
| `mesh` | As peças viram malhas: facetas planas, UV em metros ao longo do veio, cor de vértice com o desgaste da base. |
| `material` | Um `StandardMaterial` por conjunto de texturas do pool (`albedo/normal/ao/roughness.ktx2`). |
| `plugin` | ECS: nasce em `Added<FenceSpec>`, refaz em `Changed`, em lotes; colisor Avian de caixas (feature `physics`). |

## Features

- `physics` (por omissão): colisor composto na entidade da cerca — uma caixa
  fina por vão (`fence-collider="segment"`), só postes (`"posts"`) ou nenhum.

## Testes

```bash
cargo test -p bevy_ext_fence
```
