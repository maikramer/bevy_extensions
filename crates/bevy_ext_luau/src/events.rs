//! Grupo `viber.events()` + fan-out engine→Lua (portado de
//! `viber::luau::events`, exit4/F2).
//!
//! Pull model: os produtores (UI declarativa, combate, cinemáticas…) empurram
//! [`ScriptGameEvent`]s para a fila [`ScriptEventQueue`] (contrato do
//! `bevy_ext_core::scripting`); o sistema [`fan_out_events`] copia cada evento
//! para a fila de CADA script subscrito (quem chamou `viber.events()` pelo
//! menos uma vez — a presença da chave no registry da VM É a subscrição) e o
//! script drena com `viber.events()` → array `{type=..., ...}`.
//!
//! No host do monólito (uma VM partilhada) o fan-out corria sobre o handle
//! cru; no BMS (um contexto por script) o [`fan_out_events`] sistema itera
//! [`ScriptContexts`] e aplica a MESMA lógica por contexto — a conversão para
//! tabela ([`script_game_event_to_table`]) é o corpo do `to_table` que o
//! `bevy_ext_ui::ui::host` tinha junto ao tipo.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use bevy::prelude::*;
use bevy_ext_core::scripting::{ScriptEventQueue, ScriptGameEvent};
use bevy_mod_scripting::core::script::ScriptContexts;
use bevy_mod_scripting::lua::{LuaContext, LuaScriptingPlugin};
use mlua::{Lua, Table};
use parking_lot::Mutex;

/// Marca atómica por VM indicando se `viber.events()` já foi chamado (subscrito).
#[derive(Clone)]
struct VmEventsSubscribed(Arc<AtomicBool>);

fn ensure_events_flag(lua: &Lua) -> Arc<AtomicBool> {
    if let Some(flag) = lua.app_data_ref::<VmEventsSubscribed>() {
        return flag.0.clone();
    }
    let has_subs = lua
        .named_registry_value::<Table>("viber_events")
        .is_ok_and(|t| !t.is_empty());
    let flag = Arc::new(AtomicBool::new(has_subs));
    lua.set_app_data(VmEventsSubscribed(flag.clone()));
    flag
}

/// Transbordo da fila por script: eventos NOVOS são descartados com um aviso
/// 1× por path — um script que não drena não cria lag na engine.
pub const EVENT_QUEUE_CAP: usize = 64;

/// App data do contexto: o path do script que o criou (`viber.events`
/// subscreve por path). Semeado pelo `init_context` a partir do
/// [`bevy_mod_scripting::prelude::ScriptAttachment`].
#[derive(Debug, Clone)]
pub struct ScriptPath(pub String);

/// Converte o evento para a tabela que `viber.events()` devolve ao script.
/// O corpo é o MESMO do `to_table` que vivia no `bevy_ext_ui::ui::host`
/// junto ao tipo (exit4/F2: o tipo ficou no core — sem mlua lá — e a
/// conversão acompanha o host de scripting).
pub fn script_game_event_to_table(lua: &Lua, event: &ScriptGameEvent) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.raw_set("type", event.kind())?;
    match event {
        ScriptGameEvent::Kill { name, entity } => {
            t.raw_set("name", name.clone())?;
            t.raw_set("entity", *entity)?;
        }
        ScriptGameEvent::Hit {
            entity,
            amount,
            faction,
            x,
            y,
            z,
        } => {
            t.raw_set("entity", *entity)?;
            t.raw_set("amount", *amount)?;
            t.raw_set("faction", faction.clone())?;
            t.raw_set("x", *x)?;
            t.raw_set("y", *y)?;
            t.raw_set("z", *z)?;
        }
        ScriptGameEvent::PlayerDied => {}
        ScriptGameEvent::Collect { item, amount } => {
            t.raw_set("item", item.clone())?;
            t.raw_set("amount", *amount)?;
        }
        ScriptGameEvent::QuestDone { id } => {
            t.raw_set("id", id.clone())?;
        }
        ScriptGameEvent::LevelUp { level } => {
            t.raw_set("level", *level)?;
        }
        ScriptGameEvent::UiAction { name, arg } => {
            t.raw_set("name", name.clone())?;
            t.raw_set("arg", arg.clone())?;
        }
        ScriptGameEvent::CinematicEnd { id, skipped } => {
            t.raw_set("id", id.clone())?;
            t.raw_set("skipped", *skipped)?;
        }
        ScriptGameEvent::SceneLoaded { scene, entry } => {
            t.raw_set("scene", scene.clone())?;
            t.raw_set("entry", entry.clone())?;
        }
        ScriptGameEvent::SceneUnloading { scene } => {
            t.raw_set("scene", scene.clone())?;
        }
    }
    Ok(t)
}

/// Instala `viber.events()` na tabela `viber` de UMA VM.
pub(crate) fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
    ensure_events_flag(lua);
    api.set(
        "events",
        lua.create_function(|lua, ()| {
            let path_ref = lua.app_data_ref::<ScriptPath>();
            let path = path_ref.as_ref().map_or("", |p| p.0.as_str());
            let queues: Table = lua.named_registry_value("viber_events")?;
            // 1.ª chamada CRIA a fila (a presença da chave É a subscrição) —
            // erro aqui era o script nunca subscrever e nunca receber nada.
            let queue: Table = match queues.raw_get::<Table>(path) {
                Ok(q) => q,
                Err(_) => {
                    let fresh = lua.create_table()?;
                    queues.raw_set(path, fresh.clone())?;
                    if let Some(flag) = lua.app_data_ref::<VmEventsSubscribed>() {
                        flag.0.store(true, Ordering::Release);
                    }
                    fresh
                }
            };
            let count = queue.raw_len();
            let out = lua.create_table()?;
            if count == 0 {
                // Fila vazia: não alocar outra tabela para substituir `queue`.
                return Ok(out);
            }
            for i in 1..=count {
                out.raw_set(i, queue.raw_get::<mlua::Value>(i)?)?;
            }
            let fresh = lua.create_table()?;
            queues.raw_set(path, fresh)?;
            Ok(out)
        })?,
    )?;
    Ok(())
}

/// Fan-out em UMA VM (a lógica do monólito, contexto-a-contexto no BMS):
/// cada evento vira tabela Lua e entra na fila de cada script subscrito.
/// Transbordo (cap [`EVENT_QUEUE_CAP`]) descarta o evento NOVO e avisa 1×
/// por path.
pub(crate) fn fan_out_vm(lua: &Lua, incoming: &[ScriptGameEvent], dropped: &mut HashSet<String>) {
    if incoming.is_empty() {
        return;
    }
    let queues: Table = match lua.named_registry_value("viber_events") {
        Ok(t) => t,
        Err(e) => {
            log::warn!(target: "viber::luau", "fan-out de eventos: {e}");
            return;
        }
    };
    for pair in queues.pairs::<String, Table>() {
        let (path, queue) = match pair {
            Ok(p) => p,
            Err(e) => {
                log::warn!(target: "viber::luau", "fan-out de eventos: {e}");
                continue;
            }
        };
        for event in incoming {
            if queue.raw_len() >= EVENT_QUEUE_CAP {
                if dropped.insert(path.clone()) {
                    log::warn!(target: "viber::luau",
                        "viber.events: fila de '{path}' cheia ({EVENT_QUEUE_CAP}) — a descartar eventos novos");
                }
                break;
            }
            let Ok(table) = script_game_event_to_table(lua, event) else {
                continue;
            };
            if let Err(e) = queue.push(table) {
                log::warn!(target: "viber::luau", "fan-out de eventos: {e}");
                break;
            }
        }
    }
}

/// Sistema: copia a fila [`ScriptEventQueue`] para todos os contextos BMS
/// vivos. Corre no PreUpdate, ANTES do despacho de callbacks do BMS — um
/// evento chega no mesmo frame a quem drena (o pull do `viber.events()` é
/// do próprio script, em qualquer momento do seu on_update).
pub(crate) fn fan_out_events(
    contexts: Option<Res<ScriptContexts<LuaScriptingPlugin>>>,
    queue: Option<ResMut<ScriptEventQueue>>,
    mut subs: Local<HashMap<usize, (Weak<Mutex<LuaContext>>, Arc<AtomicBool>)>>,
) {
    let _probe = bevy_ext_core::perf_probe::Probe::new("luau.events");
    let Some(mut queue) = queue else {
        return;
    };
    if queue.0.is_empty() {
        return;
    }
    // A fila é CONSUMIDA aqui (o `mem::take` do host do monólito): sem isto
    // cada evento era re-entregue em todos os frames — o `scene_loaded`
    // limpava o registo de hostis a cada frame depois de uma troca de cena
    // (chefe a "despertar" em loop, criaturas sem registo) e um `kill`
    // pagava vezes sem conta.
    let incoming = std::mem::take(&mut queue.0);
    let incoming = &incoming;
    let mut dropped = HashSet::new();
    let Some(contexts) = contexts else {
        return;
    };
    let read = contexts.read();
    let mut live_contexts = 0usize;
    for (_, context) in read.first_resident_from_each_context() {
        let Some(available) = context.as_available_context() else {
            continue;
        };
        live_contexts += 1;
        let ptr = Arc::as_ptr(available) as usize;
        if let Some((weak, flag)) = subs.get(&ptr)
            && weak.strong_count() > 0
            && std::ptr::eq(weak.as_ptr(), Arc::as_ptr(available))
            && !flag.load(Ordering::Acquire)
        {
            continue;
        }
        let Some(guard) = available.try_lock() else {
            continue;
        };
        let lua: &Lua = &**guard;
        let flag = ensure_events_flag(lua);
        let has_subs = flag.load(Ordering::Acquire);
        subs.insert(ptr, (Arc::downgrade(available), flag));
        if has_subs {
            fan_out_vm(lua, incoming, &mut dropped);
        }
    }
    if subs.len() > live_contexts + 64 {
        subs.retain(|_, (weak, _)| weak.strong_count() > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ext_core::scripting::ScriptGameEvent;

    fn vm() -> Lua {
        let lua = Lua::new();
        let api = lua.create_table().expect("tabela viber");
        install(&lua, &api).expect("instala events");
        lua.globals().set("viber", api).expect("globals");
        lua.set_named_registry_value("viber_events", lua.create_table().expect("fila"))
            .expect("registry");
        lua
    }

    #[test]
    fn events_sem_subscricao_devolve_vazio() {
        let lua = vm();
        let ok: bool = lua
            .load("return #viber.events() == 0 and #viber.events() == 0")
            .eval()
            .expect("dreno duplo");
        assert!(ok, "a 1.ª chamada subscreve e vem vazia; a 2.ª também");
    }

    #[test]
    fn fan_out_entra_na_fila_do_script_subscrito() {
        let lua = vm();
        // Semeia o path do contexto (o init_context publica o asset path).
        lua.set_app_data(ScriptPath("npcs/townsfolk.lua".into()));
        let _ = lua.load("viber.events()").exec(); // subscreve

        let incoming = vec![
            ScriptGameEvent::Kill {
                name: "Lobo".into(),
                entity: 9,
            },
            ScriptGameEvent::UiAction {
                name: "buy".into(),
                arg: "potion".into(),
            },
        ];
        let mut dropped = HashSet::new();
        fan_out_vm(&lua, &incoming, &mut dropped);

        let (types, entities, labels): (Vec<String>, Vec<i64>, Vec<String>) = lua
            .load(
                r#"
                local types, entities, labels = {}, {}, {}
                for _, e in ipairs(viber.events()) do
                  table.insert(types, e.type)
                  table.insert(entities, e.entity or -1)
                  table.insert(labels, e.name or e.arg or '')
                end
                return types, entities, labels
            "#,
            )
            .eval()
            .expect("dreno");
        assert_eq!(
            (types.as_slice(), entities.as_slice(), labels.as_slice()),
            (
                ["kill".to_string(), "ui_action".to_string()].as_slice(),
                [9i64, -1].as_slice(),
                ["Lobo".to_string(), "buy".to_string()].as_slice()
            )
        );
        // Sem subscrição de outro path: drenar de novo dá vazio.
        let empty: usize = lua.load("return #viber.events()").eval().expect("vazio");
        assert_eq!(empty, 0);
    }

    #[test]
    fn fan_out_events_consome_a_fila() {
        // Sem contextos vivos o sistema ainda consome: um evento nunca é
        // re-entregue no frame seguinte (o `scene_loaded` repetia-se em
        // todos os frames e limpava o registo de hostis).
        let mut app = App::new();
        app.insert_resource(ScriptEventQueue(vec![ScriptGameEvent::SceneLoaded {
            scene: "main".into(),
            entry: String::new(),
        }]));
        app.add_systems(Update, fan_out_events);
        app.update();
        assert!(app.world().resource::<ScriptEventQueue>().0.is_empty());
    }

    #[test]
    fn fan_out_capa_e_descarta_os_novos() {
        let lua = vm();
        lua.set_app_data(ScriptPath("x.lua".into()));
        let _ = lua.load("viber.events()").exec();
        let mut dropped = HashSet::new();
        for _ in 0..(EVENT_QUEUE_CAP + 5) {
            fan_out_vm(
                &lua,
                &[ScriptGameEvent::Collect {
                    item: "ouro".into(),
                    amount: 1,
                }],
                &mut dropped,
            );
        }
        let count: usize = lua.load("return #viber.events()").eval().expect("cap");
        assert_eq!(count, EVENT_QUEUE_CAP);
        assert!(dropped.contains("x.lua"), "avisa 1× por path");
    }

    #[test]
    fn to_table_cobre_todas_as_variantes() {
        let lua = Lua::new();
        let events = vec![
            ScriptGameEvent::Kill {
                name: "A".into(),
                entity: 1,
            },
            ScriptGameEvent::Hit {
                entity: 2,
                amount: 3.0,
                faction: "bando".into(),
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            ScriptGameEvent::PlayerDied,
            ScriptGameEvent::Collect {
                item: "moeda".into(),
                amount: 5,
            },
            ScriptGameEvent::QuestDone { id: "q1".into() },
            ScriptGameEvent::LevelUp { level: 2 },
            ScriptGameEvent::UiAction {
                name: "n".into(),
                arg: "a".into(),
            },
            ScriptGameEvent::CinematicEnd {
                id: "c".into(),
                skipped: true,
            },
            ScriptGameEvent::SceneLoaded {
                scene: "s".into(),
                entry: "e".into(),
            },
            ScriptGameEvent::SceneUnloading { scene: "s".into() },
        ];
        for event in &events {
            let table = script_game_event_to_table(&lua, event).expect("tabela");
            let kind: String = table.raw_get("type").expect("type");
            assert_eq!(kind, event.kind());
        }
    }
}
