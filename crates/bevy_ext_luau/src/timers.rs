//! Timers para scripts (`viber.after`/`viber.every`/`viber.timer_cancel`) —
//! o mesmo desenho do monólito (`viber::luau::timers`): o estado vive em
//! registos LUA (`viber_timers`, `viber_timer_seq`, `viber_timer_cancelled`,
//! `viber_timer_collected`), as closures registam/cancelam sem tocar no
//! host, e o tick Rust ([`tick_timers`]) corre por contexto no início do
//! frame, ANTES dos `on_update`.
//!
//! Timers são world-scoped: correm independentemente do raio de ativação da
//! entidade (donos congelados continuam com timers a vencer — documentado).


use bevy::prelude::*;
use bevy_mod_scripting::core::script::ScriptContexts;
use mlua::{Function, Lua, Table};

use bevy_mod_scripting::lua::LuaScriptingPlugin;
use crate::globals;

/// Instala `viber.after`/`viber.every`/`viber.timer_cancel` na tabela
/// `viber` e semeia os registos do tick.
pub fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
    // Ids que o TICK já recolheu e ainda vai executar (a callback de um
    // deles pode cancelar outro). É o único estado em que o marcador de
    // `viber_timer_cancelled` é necessário: uma entrada ainda na fila é
    // removida e nunca vence, e um id já consumido/inexistente não tem o
    // que cancelar — marcá-lo deixava a tabela a crescer para sempre.
    lua.set_named_registry_value("viber_timer_collected", lua.create_table()?)?;
    lua.set_named_registry_value("viber_timer_cancelled", lua.create_table()?)?;
    let seq = lua.create_table()?;
    seq.raw_set("next", 1)?;
    lua.set_named_registry_value("viber_timer_seq", seq)?;
    lua.set_named_registry_value("viber_timers", lua.create_table()?)?;

    let register = |periodic: bool| {
        lua.create_function(move |lua, (secs, func): (f64, Function)| {
            if !secs.is_finite() || secs < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.after/every: segundos têm de ser finitos e >= 0",
                ));
            }
            let path = lua
                .app_data_ref::<crate::events::ScriptPath>()
                .map(|p| p.0.clone())
                .filter(|p| !p.is_empty())
                .ok_or_else(|| mlua::Error::runtime(
                    "viber.after/every fora de um script (sem path)",
                ))?;
            // O dono é a entidade do attach; fora de `on_update` (REPL) o
            // timer era aceite e descartado no 1.º tick, em silêncio — o
            // monólito devolve erro e aqui é igual.
            let owner: Option<i64> = lua
                .app_data_ref::<crate::ScriptEntity>()
                .and_then(|e| e.0)
                .map(|e| e.to_bits() as i64);
            let Some(owner) = owner else {
                return Err(mlua::Error::runtime("viber.after fora de on_update"));
            };
            let elapsed = crate::globals().read().map(|g| g.time).unwrap_or(0.0);
            let timers: Table = lua.named_registry_value("viber_timers")?;
            let seq: Table = lua.named_registry_value("viber_timer_seq")?;
            let id: i64 = seq.raw_get("next")?;
            seq.raw_set("next", id + 1)?;
            let entry = lua.create_table()?;
            entry.raw_set("at", elapsed + secs)?;
            if periodic {
                entry.raw_set("period", secs)?;
            }
            entry.raw_set("func", func)?;
            entry.raw_set("owner", owner)?;
            entry.raw_set("path", path.as_str())?;
            timers.raw_set(id, entry)?;
            Ok(id)
        })
    };
    api.set("after", register(false)?)?;
    api.set("every", register(true)?)?;

    // Cancela um handle (true se a entrada existia/estava a correr). Uma
    // entrada ainda na fila sai e nunca vence; uma que o tick JÁ recolheu
    // leva o marcador, senão a callback re-agendava.
    api.set(
        "timer_cancel",
        lua.create_function(|lua, id: i64| {
            let timers: Table = lua.named_registry_value("viber_timers")?;
            let queued = timers
                .raw_get::<mlua::Value>(id)
                .is_ok_and(|value| !matches!(value, mlua::Value::Nil));
            let _ = timers.raw_remove(id);
            let collected = lua
                .named_registry_value::<Table>("viber_timer_collected")
                .is_ok_and(|table| table.raw_get::<bool>(id).unwrap_or(false));
            if !queued && collected {
                let cancelled: Table = lua.named_registry_value("viber_timer_cancelled")?;
                cancelled.raw_set(id, true)?;
            }
            Ok(queued || collected)
        })?,
    )?;
    Ok(())
}

/// Semeia, por contexto, o tempo e o path do script (`viber_script_elapsed`/
/// `viber_script_path`): o que as closures de registo leem. `path` = o id do
/// script BMS (o caminho relativo a `scripts/`).
pub(crate) fn seed_script_identity(lua: &Lua, path: &str, elapsed: f64) {
    let _ = lua.set_named_registry_value("viber_script_path", path.to_string());
    let _ = lua.set_named_registry_value("viber_script_elapsed", elapsed);
}

/// Tick (início do frame, ANTES dos `on_update`): executa os timers vencidos
/// de CADA contexto com o mesmo seed do frame. Erros são pcall-style com
/// warn por path — um timer a falhar nunca derruba o frame; `every`
/// cancelado não re-agenda.
pub fn tick_timers(contexts: Option<Res<ScriptContexts<LuaScriptingPlugin>>>) {
    let Some(contexts) = contexts else {
        return;
    };
    let elapsed = globals().read().map(|g| g.time).unwrap_or(0.0);
    let read = contexts.read();
    for (id, context) in read.all_residents() {
        let Some(available) = context.as_available_context() else {
            continue;
        };
        let Some(guard) = available.try_lock() else {
            continue;
        };
        let lua: &Lua = &**guard;
        let _ = id;
        tick_vm(lua, elapsed);
    }
}

/// O tick de UMA VM (o mesmo algoritmo do monólito).
fn tick_vm(lua: &Lua, elapsed: f64) {
    let Ok(timers) = lua.named_registry_value::<Table>("viber_timers") else {
        return;
    };
    let Ok(cancelled) = lua.named_registry_value::<Table>("viber_timer_cancelled") else {
        return;
    };
    // Ids recolhidos por ESTE tick e ainda por executar: o `timer_cancel`
    // dentro de uma callback só os distingue de um id já consumido por estar
    // aqui (a entrada já saiu de `viber_timers`).
    let collected: Table = match lua.named_registry_value("viber_timer_collected") {
        Ok(table) => table,
        Err(_) => return,
    };
    let _ = collected.clear();

    // Snapshot dos vencidos: executar diretamente durante o `pairs` mutaria
    // a tabela (a callback pode registar/cancelar) — recolhe-se primeiro.
    let mut due = Vec::new();
    for pair in timers.clone().pairs::<i64, Table>().flatten() {
        let (id, entry) = pair;
        let at: f64 = entry.raw_get("at").unwrap_or(f64::INFINITY);
        if elapsed < at {
            continue;
        }
        let _ = collected.raw_set(id, true);
        let _ = timers.raw_remove(id);
        due.push((id, entry));
    }

    for (id, entry) in due {
        if cancelled.raw_get::<bool>(id).unwrap_or(false) {
            let _ = cancelled.raw_remove(id);
            continue;
        }
        let func: Function = match entry.raw_get("func") {
            Ok(func) => func,
            Err(_) => continue,
        };
        let path: String = entry.raw_get("path").unwrap_or_default();
        let owner: Option<i64> = entry.raw_get("owner").ok();
        // Seed do contexto do DONO: a callback vê o path (e o `viber.state`
        // funciona; a posição/props da entidade vêm do snapshot do frame).
        let _ = lua.set_named_registry_value("viber_script_path", path.clone());
        let _ = lua.set_named_registry_value("viber_script_elapsed", elapsed);
        let _ = lua.set_named_registry_value("viber_signal_sender", owner);
        let result = func.call::<()>(()).map(|()| {
            if let Some(period) = entry.raw_get::<f64>("period").ok() {
                let at: f64 = entry.raw_get("at").unwrap_or(elapsed);
                let mut next = at + period;
                while next <= elapsed {
                    next += period;
                }
                let _ = entry.raw_set("at", next);
                if cancelled.raw_get::<bool>(id).unwrap_or(false) {
                    let _ = cancelled.raw_remove(id);
                } else {
                    let _ = timers.raw_set(id, entry.clone());
                }
            }
        });
        if let Err(error) = result {
            log::warn!(target: "viber::luau", "timer '{path}'#{id}: {error}");
        }
    }
}
