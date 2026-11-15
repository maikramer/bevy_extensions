# bevy_ext_bridge_protocol

Contrato do bridge do `vael-tools`: constantes de métodos, cliente HTTP
JSON-RPC std-only e descoberta de sessões/engines (`engine.json`).

```rust
use bevy_ext_bridge_protocol::{BridgeClient, resolve_port};

let port = resolve_port(None, Some("world.bsn".as_ref()));
let client = BridgeClient::localhost(port);
let pong = client.ping()?;
```

O cliente fala o mesmo transporte do `bevy_remote` (`POST /`, JSON-RPC 2.0,
`Connection: close`) e não depende de Bevy, `reqwest` ou `tokio`.
