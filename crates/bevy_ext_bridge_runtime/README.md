# bevy_ext_bridge_runtime

Plugin Bevy que entra no jogo e publica o namespace `vael-tools/*` no
`bevy_remote` já usado pela bridge:

- `ping`, `frame_stats` (p95/p99/FPS), `hitches`, `memory` (RSS/HWM/threads),
  `tree`, `logs`, `input.key`;
- reusa o `RemotePlugin`/`RemoteHttpPlugin` existentes; se não existirem,
  cria o servidor na porta configurada.

```rust
app.add_plugins(bevy_ext_bridge_runtime::BridgeToolsPlugin::default());
```

O core não depende do monólito (`bevy_ext_viber_engine`); os adapters de
Lua/cena/eventos entram por features numa fase seguinte.
