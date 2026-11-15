//! Fila de eventos engine→Lua ([`ScriptGameEvent`]): estado do jogo que mudou
//! FORA dos scripts. Pull model — produtores empurram para
//! [`ScriptEventQueue`], o `luau_update` faz fan-out para filas por-path e o
//! script drena com `viber.events()`.

use std::collections::HashSet;

use bevy::prelude::*;
use mlua::{Lua, Table};

use super::ctx::ScriptCtx;

// O CONTRATO dos eventos vive agora no `bevy_ext_ui::ui::host` (exit3/wave-c):
// a UI declarativa extraída empurra `UiAction` para A MESMA fila que o
// `luau_update` drena (antes da troca eram DOIS tipos — as ações de UI nunca
// chegavam aos scripts). O `to_table` acompanha o tipo no host; o caminho
// histórico continua a resolver. O `EVENT_QUEUE_CAP` é do `bevy_ext_engine`.
pub use bevy_ext_ui::ui::host::{ScriptEventQueue, ScriptGameEvent};
pub use bevy_ext_engine::luau::EVENT_QUEUE_CAP;


/// Instala `viber.events()` na tabela `viber`.
pub(crate) fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
    api.set(
        "events",
        lua.create_function(|lua, ()| {
            let path = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.path.clone())
                .ok_or_else(|| mlua::Error::runtime("viber.events fora de script (sem path)"))?;
            let queues: Table = lua.named_registry_value("viber_events")?;
            // 1.ª chamada CRIA a fila (a presença da chave É a subscrição) —
            // erro aqui era o script nunca subscrever e nunca receber nada.
            let queue: Table = match queues.raw_get::<Table>(path.as_str()) {
                Ok(q) => q,
                Err(_) => {
                    let fresh = lua.create_table()?;
                    queues.raw_set(path.as_str(), fresh.clone())?;
                    fresh
                }
            };
            let count = queue.raw_len();
            let out = lua.create_table()?;
            for i in 1..=count {
                out.raw_set(i, queue.raw_get::<mlua::Value>(i)?)?;
            }
            let fresh = lua.create_table()?;
            queues.raw_set(path.as_str(), fresh)?;
            Ok(out)
        })?,
    )?;
    Ok(())
}

/// Fan-out (chamado do `luau_update` ANTES dos `on_update`): cada evento vira
/// tabela Lua e entra na fila de CADA script subscrito (quem chamou
/// `viber.events()` pelo menos uma vez — a presença da chave no registry É a
/// subscrição). Transbordo (cap [`EVENT_QUEUE_CAP`]) descarta o evento NOVO e
/// avisa 1× por path — um script que não drena não cria lag na engine.
pub(crate) fn fan_out_events(
    lua: &Lua,
    incoming: &[ScriptGameEvent],
    dropped: &mut HashSet<String>,
) {
    if incoming.is_empty() {
        return;
    }
    let queues: Table = match lua.named_registry_value("viber_events") {
        Ok(t) => t,
        Err(e) => {
            warn!(target: "viber::luau", "fan-out de eventos: {e}");
            return;
        }
    };
    for pair in queues.pairs::<String, Table>() {
        let (path, queue) = match pair {
            Ok(p) => p,
            Err(e) => {
                warn!(target: "viber::luau", "fan-out de eventos: {e}");
                continue;
            }
        };
        for event in incoming {
            if queue.raw_len() >= EVENT_QUEUE_CAP {
                if dropped.insert(path.clone()) {
                    warn!(target: "viber::luau",
                        "viber.events: fila de '{path}' cheia ({EVENT_QUEUE_CAP}) — a descartar eventos novos");
                }
                break;
            }
            let Ok(table) = event.to_table(lua) else {
                continue;
            };
            if let Err(e) = queue.push(table) {
                warn!(target: "viber::luau", "fan-out de eventos: {e}");
                break;
            }
        }
    }
}
