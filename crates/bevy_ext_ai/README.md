# bevy_ext_ai

AI service for Bevy games: the sense bus (sight/hearing), the single
velocity-space locomotion consumer, the simple creature FSM and entity
vitals.

The crate owns the *service* layer: producers state a desire
(`AiLocomotion::drive_to`, `SenseBus` events) and the crate provides the
one consumer that turns it into movement over whatever ground source the
game plugs in (`Ground`). Behaviour is yours: bring the FSM
(`AiPlugin`), or bring your own and take only the service
(`AiServicePlugin`) — the same choice the engine bootstrap makes with
`AiMode`.

```toml
[dependencies]
bevy_ext_ai = "0.1"
```

```rust
use bevy::prelude::*;
use bevy_ext_ai::{AiMode, EnemyCreature, Ground};

fn main() {
    let mut app = App::new();
    AiMode::Full.apply(&mut app); // sentidos + FSM + locomoção
    // Alturas: quem monta a App embrulha a sua fonte de chão (o runtime de
    // terreno do jogo) e a IA amostra por `GroundQuery`.
    app.insert_resource(Ground::new(my_terrain));
    app.world_mut().spawn(EnemyCreature::default());
}

# fn my_terrain() -> impl bevy_ext_ai::GroundQuery { todo!() }
```

## What it contains

- **Sentidos** — `SenseBus`/`SenseInbox`, cone de visão com orçamento de
  raios + LOS por raycast (Rapier), audição por limiar de loudness
  (`hears`), e a FSM de estados `Wander → Alert → Chase → Search`
  (`sense_step`).
- **Locomoção** — `AiLocomotion` (desejo de um frame: `drive`/`drive_to`/
  `halt`/`look_at`) e `apply_ai_locomotion`, o ÚNICO consumidor: rampa de
  aceleração, passo no plano XZ, snap do Y ao chão (`GroundQuery`) e yaw
  a `turn_rate`. `viber.move_towards` e a FSM partilham o contrato.
- **FSM simples** — `EnemyCreature` (stats autorais + estado),
  `WanderState`, seeds determinísticas (`enemy_seed`/`wander_target`) e a
  histerese de deaggro (`enemy_next_state`).
- **Vitais** — `Health`/`Hostile`, os guards `ensure_*_vitals` e as
  primitivas puras de dano (`apply_damage`, `radial_damage`).

Os tipos preservam os caminhos de reflexão históricos do monólito
(`viber::ai::…`, `viber::senses::…`, `viber::vitals::…`) com
`#[type_path]`, para o BRP/`viber.ecs` e os saves não mudarem após a
extração.
