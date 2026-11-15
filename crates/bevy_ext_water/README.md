# bevy_ext_water

Água interactiva para Bevy, independente do jogo: **só depende de `bevy` e
`avian3d`**.

| Módulo | O que dá |
|---|---|
| `body` | corpos lago/rio (`WaterBody`), consulta `surface_y_at`, amostra `(surface, depth, flow, plunge)` e contorno orgânico de lago |
| `mirror` | espelhos CPU do shader: `surface_height_at` (a cota DESENHADA) e `wave_height_at` (detalhe das normais) |
| `waves` | solver iWave dispersivo em compute, janela rolante de 128 m, `domain_tex` `(depth, flow, plunge)`, impulsos `(cratera/esteira Kelvin)` |
| `material` | material de dois passes (Multiply/Add), uniformes e sync de ambiente |
| `fx` | contactos de entrada/vadeação/saída, anéis de superfície e `WaterSplashEvent` para as partículas do jogo |
| `buoyancy` | `FloatingBody`/`FloatState`, integrador próprio (translação + pitch/roll) e adapter de corpos rígidos avian |
| `obstacles` | SDF 2D de rochas/pilares, deflexão da corrente e halo de espuma |
| `source` | traço `WaterSource` que o jogo implementa + `WaterFocus`/`WaterObstacles` |

## Ligação num jogo

```rust
use bevy::prelude::*;
use bevy_ext_water::{WaterBody, WaterPlugin, WaterSource};

#[derive(Resource)]
struct MeuMundo {
    bodies: Vec<WaterBody>,
}

impl WaterSource for MeuMundo {
    fn bodies(&self) -> &[WaterBody] {
        &self.bodies
    }
    fn bed_y(&self, x: f32, z: f32) -> Option<f32> {
        // cota do leito no teu heightfield (None fora do terreno)
        None
    }
}

App::new()
    .insert_resource(MeuMundo { bodies: vec![] })
    .add_plugins(WaterPlugin::<MeuMundo>::default());
```

O jogo continua a ser dono de:

- **obstáculos**: converte os colliders dele (avian, Rapier, voxel, malha) em
  `WaterObstacle`s já cortados na lâmina e publica-os em `WaterObstacles`;
- **ambiente**: escreve `WaterEnvironment` (vento, chuva, sol/lua, céu,
  amplitude) por frame;
- **foco**: `WaterFocus` com o XZ do herói (sem isto a janela segue a câmara);
- **partículas**: consome `WaterSplashEvent` e traduz `SplashPreset` para o
  seu sistema;
- **marcadores**: põe `WaterBodied` nos corpos com pés que devem salpicar
  (barcos/bóias chegam só com `WaterKinematics`).

## Flutuação

`FloatingBody` descreve o casco (massa, meia-caixa, centro de massa,
deslocamento, arrasto). O integrador próprio faz 3D de translação + pitch/roll
(yaw é do leme) e encalha no leito. O adapter avian (`AvianFloating` +
`AvianBuoyancyPlugin`) aplica as MESMAS forças a um `RigidBody` dinâmico.

Nascer: `surface + body.rest_height()` (equilíbrio em água parada).

## Testes

```bash
cargo test -p bevy_ext_water
```
