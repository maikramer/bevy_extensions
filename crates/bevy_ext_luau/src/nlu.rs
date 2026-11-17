//! `viber.nlu` — GLiNER2 como serviço de compreensão de texto. Portado do
//! monólito (exit4/F8; feature `nlu`, path-dep CelerAI — a MESMA que o
//! monólito tinha; os caminhos `viber::luau::nlu` resolvem por shim).
//!
//! O modelo (~145 MB, ONNX 4-bit) carrega numa **worker thread**; o jogo fala
//! com ela por canais. `submit` devolve um id e `poll` drena o resultado, para
//! que uma inferência de ~50–90 ms nunca bloqueie o frame.
//!
//! ```lua
//! viber.nlu.load("assets/public/nlu")       -- async; true se os ficheiros existem
//! local id = viber.nlu.submit("lobos")
//! local r  = viber.nlu.poll()               -- nil | {kind="loaded"|"parsed"|"error", ...}
//! ```
//!
//! Sem a feature `nlu` (ou sem o artefacto), `available()` devolve `false` e os
//! scripts do jogo caem no fluxo antigo.

use mlua::{Lua, Table};

#[cfg(feature = "nlu")]
mod imp {
    use std::collections::VecDeque;
    use std::path::{Component, Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use mlua::{Lua, Table};

    use crate::game::ScriptsDir;

    const ARTIFACT_FILES: &[&str] = &[
        "encoder.onnx",
        // O encoder 4-bit usa data externa (`save_model_to_file(...,
        // use_external_data_format=True)`); sem este ficheiro `available()`
        // mentia e o ORT só falava depois, no load.
        "encoder.onnx.data",
        "embedding.bin",
        "heads.bin",
        "tokenizer.json",
        "nlu_config.json",
    ];

    enum Request {
        Load {
            dir: PathBuf,
            reply: Sender<Response>,
        },
        Parse {
            id: u32,
            text: String,
            reply: Sender<Response>,
        },
    }

    enum Response {
        Loaded(Result<(), String>),
        Parsed {
            id: u32,
            result: Result<celer_nlu::Parsed, String>,
        },
    }

    /// Serviço partilhado do processo: evita spawnar 2 threads + carregar um
    /// modelo ONNX de 145 MB por cada VM BMS (centenas de entidades).
    struct SharedNluService {
        tx: Mutex<Sender<Request>>,
    }

    static SHARED_NLU: OnceLock<Result<SharedNluService, String>> = OnceLock::new();
    static SHARED_READY: AtomicBool = AtomicBool::new(false);
    static SHARED_LOAD_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

    fn shared_error_slot() -> &'static Mutex<Option<String>> {
        SHARED_LOAD_ERROR.get_or_init(|| Mutex::new(None))
    }

    fn shared_service() -> mlua::Result<&'static SharedNluService> {
        SHARED_NLU
            .get_or_init(|| {
                let (req_tx, req_rx) = mpsc::channel();
                std::thread::Builder::new()
                    .name("celer-nlu".into())
                    .spawn(move || worker(req_rx))
                    .map_err(|e| format!("viber.nlu: worker: {e}"))?;
                Ok(SharedNluService {
                    tx: Mutex::new(req_tx),
                })
            })
            .as_ref()
            .map_err(|e| mlua::Error::runtime(e.clone()))
    }

    #[derive(Clone)]
    struct NluState {
        reply_tx: Sender<Response>,
        rx: Arc<Mutex<Receiver<Response>>>,
        /// Respostas de pedidos `submit` que um `parse` bloqueante consumiu
        /// sem serem dele; `poll` drena-as antes do canal.
        pending: Arc<Mutex<VecDeque<Response>>>,
        next_id: Arc<AtomicU32>,
    }

    fn worker(rx: Receiver<Request>) {
        let mut runtime: Option<celer_nlu::Runtime> = None;
        let mut loaded_dir: Option<PathBuf> = None;
        while let Ok(req) = rx.recv() {
            match req {
                Request::Load { dir, reply } => {
                    let canonical = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
                    // Chamadas a `load` em VMs distintas (ou repetidas na
                    // mesma) partilham o mesmo runtime ONNX em memória.
                    if runtime.is_some() && loaded_dir.as_ref() == Some(&canonical) {
                        let _ = reply.send(Response::Loaded(Ok(())));
                        continue;
                    }
                    let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        celer_nlu::Runtime::load(&dir)
                    }));
                    match loaded {
                        Ok(Ok(rt)) => {
                            runtime = Some(rt);
                            loaded_dir = Some(canonical);
                            // Um load anterior pode ter falhado; o erro não
                            // pode ficar pendurado depois do sucesso.
                            *shared_error_slot().lock().unwrap() = None;
                            SHARED_READY.store(true, Ordering::Release);
                            let _ = reply.send(Response::Loaded(Ok(())));
                        }
                        Ok(Err(e)) => {
                            let msg = format!("{e:#}");
                            *shared_error_slot().lock().unwrap() = Some(msg.clone());
                            let _ = reply.send(Response::Loaded(Err(msg)));
                        }
                        Err(_) => {
                            let msg = "viber.nlu: panic ao carregar o modelo".to_string();
                            *shared_error_slot().lock().unwrap() = Some(msg.clone());
                            let _ = reply.send(Response::Loaded(Err(msg)));
                        }
                    }
                }
                Request::Parse { id, text, reply } => {
                    let result = match runtime.as_mut() {
                        Some(rt) => {
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                rt.parse(&text)
                            })) {
                                Ok(Ok(p)) => Ok(p),
                                Ok(Err(e)) => Err(format!("{e:#}")),
                                // Um panic na inferência não pode matar a worker
                                // para sempre (poll passaria a dar "worker
                                // terminou").
                                Err(_) => Err("viber.nlu: panic na inferência".to_string()),
                            }
                        }
                        None => Err("viber.nlu: modelo não carregado".to_string()),
                    };
                    let _ = reply.send(Response::Parsed { id, result });
                }
            }
        }
    }

    fn resolve_dir(lua: &Lua, rel: &str) -> mlua::Result<PathBuf> {
        let relative = Path::new(rel);
        let inside = relative
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if rel.is_empty() || !inside {
            return Err(mlua::Error::runtime(format!(
                "viber.nlu.load('{rel}'): só caminhos relativos à pasta do jogo (sem '..')"
            )));
        }
        let scripts = lua
            .app_data_ref::<ScriptsDir>()
            .and_then(|d| d.0.clone())
            .ok_or_else(|| mlua::Error::runtime("viber.nlu.load sem scripts_dir"))?;
        let world = scripts
            .parent()
            .ok_or_else(|| mlua::Error::runtime("viber.nlu.load: scripts/ sem pasta de jogo"))?;
        Ok(world.join(relative))
    }

    fn files_present(dir: &Path) -> bool {
        ARTIFACT_FILES.iter().all(|f| dir.join(f).is_file())
    }

    fn state(lua: &Lua) -> mlua::Result<NluState> {
        lua.app_data_ref::<NluState>()
            .map(|st| st.clone())
            .ok_or_else(|| mlua::Error::runtime("viber.nlu: estado ausente"))
    }

    fn parsed_to_table(lua: &Lua, parsed: &celer_nlu::Parsed) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        t.set("count", parsed.count)?;
        t.set("intent", parsed.intent.clone())?;
        t.set("intent_score", parsed.intent_score)?;
        let ents = lua.create_table()?;
        for (i, e) in parsed.entities.iter().enumerate() {
            let et = lua.create_table()?;
            et.set("label", e.label.clone())?;
            et.set("start", e.start)?;
            et.set("end", e.end)?;
            et.set("text", e.text.clone())?;
            et.set("score", e.score)?;
            ents.set(i + 1, et)?;
        }
        t.set("entities", ents)?;
        Ok(t)
    }

    fn response_to_value(lua: &Lua, resp: Response) -> mlua::Result<mlua::Value> {
        match resp {
            Response::Loaded(Ok(())) => {
                let t = lua.create_table()?;
                t.set("kind", "loaded")?;
                t.set("ok", true)?;
                Ok(mlua::Value::Table(t))
            }
            Response::Loaded(Err(e)) => {
                let t = lua.create_table()?;
                t.set("kind", "loaded")?;
                t.set("ok", false)?;
                t.set("error", e)?;
                Ok(mlua::Value::Table(t))
            }
            Response::Parsed { id, result: Ok(p) } => {
                let t = parsed_to_table(lua, &p)?;
                t.set("kind", "parsed")?;
                t.set("id", id)?;
                Ok(mlua::Value::Table(t))
            }
            Response::Parsed { id, result: Err(e) } => {
                let t = lua.create_table()?;
                t.set("kind", "error")?;
                t.set("id", id)?;
                t.set("error", e)?;
                Ok(mlua::Value::Table(t))
            }
        }
    }

    pub fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
        let (res_tx, res_rx) = mpsc::channel();

        lua.set_app_data(NluState {
            reply_tx: res_tx,
            rx: Arc::new(Mutex::new(res_rx)),
            pending: Arc::new(Mutex::new(VecDeque::new())),
            next_id: Arc::new(AtomicU32::new(1)),
        });

        let nlu = lua.create_table()?;

        // available([dir]) -> bool
        nlu.set(
            "available",
            lua.create_function(|lua, dir: Option<String>| {
                let Some(dir) = dir else {
                    let _ = state(lua)?;
                    return Ok(SHARED_READY.load(Ordering::Acquire));
                };
                let path = resolve_dir(lua, &dir)?;
                Ok(files_present(&path))
            })?,
        )?;

        // ready() -> bool
        nlu.set(
            "ready",
            lua.create_function(|lua, ()| {
                let _ = state(lua)?;
                Ok(SHARED_READY.load(Ordering::Acquire))
            })?,
        )?;

        // load(dir) -> ok, err
        nlu.set(
            "load",
            lua.create_function(|lua, dir: String| {
                let path = resolve_dir(lua, &dir)?;
                let st = state(lua)?;
                if !files_present(&path) {
                    return Ok((
                        false,
                        mlua::Value::String(lua.create_string(format!(
                            "viber.nlu: artefacto incompleto em {}",
                            path.display()
                        ))?),
                    ));
                }
                let svc = shared_service()?;
                let tx = svc
                    .tx
                    .lock()
                    .map_err(|_| mlua::Error::runtime("viber.nlu: worker parado"))?;
                tx.send(Request::Load {
                    dir: path,
                    reply: st.reply_tx,
                })
                .map_err(|_| mlua::Error::runtime("viber.nlu: worker parado"))?;
                Ok((true, mlua::Value::Nil))
            })?,
        )?;

        // submit(text) -> id, err
        nlu.set(
            "submit",
            lua.create_function(|lua, text: String| {
                let st = state(lua)?;
                if !SHARED_READY.load(Ordering::Acquire) {
                    return Ok((
                        mlua::Value::Nil,
                        mlua::Value::String(
                            lua.create_string("viber.nlu: modelo ainda não carregado")?,
                        ),
                    ));
                }
                let id = st.next_id.fetch_add(1, Ordering::Relaxed);
                let svc = shared_service()?;
                let tx = svc
                    .tx
                    .lock()
                    .map_err(|_| mlua::Error::runtime("viber.nlu: worker parado"))?;
                tx.send(Request::Parse {
                    id,
                    text,
                    reply: st.reply_tx,
                })
                .map_err(|_| mlua::Error::runtime("viber.nlu: worker parado"))?;
                Ok((mlua::Value::Integer(id as i64), mlua::Value::Nil))
            })?,
        )?;

        // poll() -> nil | {kind="loaded"|"parsed"|"error", ...}
        nlu.set(
            "poll",
            lua.create_function(|lua, ()| {
                let st = state(lua)?;
                // Resultados que um `parse` bloqueante viu e não eram dele.
                let queued = {
                    let mut pending = st.pending.lock().unwrap();
                    pending.pop_front()
                };
                if let Some(resp) = queued {
                    return response_to_value(lua, resp);
                }
                let rx = st.rx.lock().unwrap();
                match rx.try_recv() {
                    Ok(resp) => response_to_value(lua, resp),
                    Err(TryRecvError::Empty) => Ok(mlua::Value::Nil),
                    Err(TryRecvError::Disconnected) => {
                        Err(mlua::Error::runtime("viber.nlu: worker terminou"))
                    }
                }
            })?,
        )?;

        // parse(text) -> table, err — bloqueante; só QA/ferramentas.
        nlu.set(
            "parse",
            lua.create_function(|lua, text: String| {
                let st = state(lua)?;
                if !SHARED_READY.load(Ordering::Acquire) {
                    return Ok((
                        mlua::Value::Nil,
                        mlua::Value::String(
                            lua.create_string("viber.nlu: modelo ainda não carregado")?,
                        ),
                    ));
                }
                let id = st.next_id.fetch_add(1, Ordering::Relaxed);
                let svc = shared_service()?;
                {
                    let tx = svc
                        .tx
                        .lock()
                        .map_err(|_| mlua::Error::runtime("viber.nlu: worker parado"))?;
                    tx.send(Request::Parse {
                        id,
                        text,
                        reply: st.reply_tx,
                    })
                    .map_err(|_| mlua::Error::runtime("viber.nlu: worker parado"))?;
                }
                let rx = st.rx.lock().unwrap();
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok((
                            mlua::Value::Nil,
                            mlua::Value::String(lua.create_string("viber.nlu.parse: timeout")?),
                        ));
                    }
                    match rx.recv_timeout(left) {
                        Ok(Response::Parsed { id: rid, result }) if rid == id => {
                            return match result {
                                Ok(p) => Ok((
                                    mlua::Value::Table(parsed_to_table(lua, &p)?),
                                    mlua::Value::Nil,
                                )),
                                Err(e) => Ok((
                                    mlua::Value::Nil,
                                    mlua::Value::String(lua.create_string(e)?),
                                )),
                            };
                        }
                        Ok(resp) => {
                            // Resposta de outro pedido (submit em voo ou o
                            // evento "loaded"): fica para o `poll` em vez de
                            // se perder só porque este parse está a espera.
                            st.pending.lock().unwrap().push_back(resp);
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            return Ok((
                                mlua::Value::Nil,
                                mlua::Value::String(lua.create_string("viber.nlu.parse: timeout")?),
                            ));
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(mlua::Error::runtime("viber.nlu: worker terminou"));
                        }
                    }
                }
            })?,
        )?;

        // last_error() -> string | nil
        nlu.set(
            "last_error",
            lua.create_function(|lua, ()| {
                let _ = state(lua)?;
                let err = shared_error_slot().lock().unwrap().clone();
                Ok(err.map(|e| lua.create_string(e)).transpose()?)
            })?,
        )?;

        api.set("nlu", nlu)?;
        Ok(())
    }
}

#[cfg(not(feature = "nlu"))]
mod imp {
    use mlua::{Lua, Table};

    pub fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
        let nlu = lua.create_table()?;
        nlu.set(
            "available",
            lua.create_function(|_, _: Option<String>| Ok(false))?,
        )?;
        nlu.set("ready", lua.create_function(|_, ()| Ok(false))?)?;
        nlu.set(
            "load",
            lua.create_function(|lua, _: String| {
                Ok((
                    false,
                    mlua::Value::String(lua.create_string("viber.nlu: feature 'nlu' desligada")?),
                ))
            })?,
        )?;
        nlu.set(
            "submit",
            lua.create_function(|lua, _: String| {
                Ok((
                    mlua::Value::Nil,
                    mlua::Value::String(lua.create_string("viber.nlu: feature 'nlu' desligada")?),
                ))
            })?,
        )?;
        nlu.set("poll", lua.create_function(|_, ()| Ok(mlua::Value::Nil))?)?;
        nlu.set(
            "parse",
            lua.create_function(|lua, _: String| {
                Ok((
                    mlua::Value::Nil,
                    mlua::Value::String(lua.create_string("viber.nlu: feature 'nlu' desligada")?),
                ))
            })?,
        )?;
        nlu.set(
            "last_error",
            lua.create_function(|_, ()| Ok(mlua::Value::Nil))?,
        )?;
        api.set("nlu", nlu)?;
        Ok(())
    }
}

pub fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
    imp::install(lua, api)
}

#[cfg(all(test, feature = "nlu"))]
mod tests {
    /// Integração real, gated: `CELER_NLU_DIR=Vael/assets/public/nlu cargo test`.
    #[test]
    fn parses_one_word_when_model_present() {
        let Ok(dir) = std::env::var("CELER_NLU_DIR") else {
            return;
        };
        let mut rt = celer_nlu::Runtime::load(dir).expect("carregar artefacto");
        let p = rt.parse("lobos").expect("parse");
        assert!(
            p.entities.iter().any(|e| e.label == "criatura"),
            "esperava criatura em {:?}",
            p.entities
        );
        assert!(p.count >= 1);
    }
}
