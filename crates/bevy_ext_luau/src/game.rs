//! Estado de JOGO partilhado (`viber.game()`), módulos (`viber.load`) e
//! estado por contexto (`viber.state()`).
//!
//! O BMS cria um contexto (uma VM Luau) por `ScriptAttachment`, portanto o
//! estado de jogo não pode viver numa tabela de VM como no Viber:
//! - `viber.game()` devolve um PROXY (tabela real com metamétodos) sobre um
//!   `BTreeMap<String, GameValue>` plano partilhado por todas as VMs (o
//!   padrão do esqueleto: `RwLock` estático). Primitivos (bool/número/
//!   string) leem e escrevem no mapa — é o estado persistível, como o
//!   `viber_game` do Viber que o save serializava. Tabelas e funções são de
//!   SESSÃO: ficam só na VM que as criou (o Vael guarda `game.enemies = {}`
//!   etc.) e não entram no mapa. `pairs(proxy)` e `for k, v in proxy do`
//!   veem primitivos + overlay: cada `viber.game()` sincroniza o proxy com
//!   o mapa (patches e remoções de outras VMs ficam visíveis na chamada
//!   seguinte, como o estado world-scoped do Viber).
//! - `viber.state()` devolve uma tabela real da própria VM (por entidade no
//!   BMS: um contexto por `(entidade, script)`). No Viber era partilhada
//!   entre todos os scripts da mesma entidade; a fusão por entidade fica
//!   para a integração. As sementes da engine (`home`/`picks` do spawn)
//!   também ficam para lá.
//!
//! Sistemas Bevy publicam patches (load de save, cheats…) com
//! [`publish_game_patch`] e leem o estado plano com [`game_snapshot`]; o
//! sistema [`apply_game_patches`] drena a fila no PreUpdate.
//!
//! exit4/F3: este módulo é agora o ÚNICO `game.rs` (o gémeo do monólito
//! ficou no caminho histórico `viber::luau::game` como shim). Recebeu do
//! monólito: `viber.load_json`, `evict_module` (hot-reload/bridge),
//! `json_to_lua`, e a interop JSON do save (`game_to_json`/
//! `restore_game_json`) — sobre o store, não sobre uma tabela de VM. O load
//! SUBSTITUI o estado: bump de [`GameStore::generation`] faz cada proxy
//! descartar o seu overlay de sessão (a tabela de VM do Viber era limpa por
//! inteiro no `json_to_game`; sem isto as tabelas de sessão sobreviviam ao
//! carregar).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use mlua::{Function, IntoLua, Lua, MultiValue, Table, Value};

/// Valor PLANO do estado de jogo (o que o save persiste; o resto é sessão).
#[derive(Debug, Clone, PartialEq)]
pub enum GameValue {
    /// Remove a chave (o `null` de um patch e o `nil` vindo do Lua).
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
}

impl GameValue {
    /// Valor Lua → estado plano. `None` = não primitivo (overlay de sessão).
    fn from_lua(value: &Value) -> Option<Self> {
        match value {
            Value::Nil => Some(Self::Null),
            Value::Boolean(b) => Some(Self::Bool(*b)),
            // mlua devolve Integer para números integrais mesmo em Luau
            // (só há doubles na VM).
            Value::Number(n) => Some(Self::Num(*n)),
            Value::Integer(i) => Some(Self::Num(*i as f64)),
            Value::String(s) => Some(Self::Str(s.to_string_lossy())),
            _ => None,
        }
    }

    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            Self::Null => Ok(Value::Nil),
            Self::Bool(b) => b.into_lua(lua),
            Self::Num(n) => n.into_lua(lua),
            Self::Str(s) => s.into_lua(lua),
        }
    }
}

/// Mapa partilhado + fila de patches vindos de sistemas Bevy.
#[derive(Debug, Default)]
pub struct GameStore {
    pub(crate) map: BTreeMap<String, GameValue>,
    patches: VecDeque<Vec<(String, GameValue)>>,
    /// Incrementado quando o estado é SUBSTITUÍDO (`restore_game_json`, o
    /// load de save). Os proxies comparam e descartam o overlay de sessão —
    /// no Viber o `json_to_game` limpava a tabela da VM por inteiro.
    pub(crate) generation: u64,
}

impl GameStore {
    /// Enfia um patch (`Null` remove a chave na aplicação).
    pub fn publish_patch(&mut self, patch: Vec<(String, GameValue)>) {
        if !patch.is_empty() {
            self.patches.push_back(patch);
        }
    }

    /// Drena a fila de patches para o mapa. Corre no PreUpdate, antes dos
    /// scripts lerem `viber.game()`.
    pub fn apply_patches(&mut self) {
        while let Some(patch) = self.patches.pop_front() {
            for (key, value) in patch {
                match value {
                    GameValue::Null => {
                        self.map.remove(&key);
                    }
                    other => {
                        self.map.insert(key, other);
                    }
                }
            }
        }
    }

    /// Snapshot plano (persistência/diagnóstico). O overlay de sessão
    /// (tabelas/funções) não aparece — só o que o save persistiria.
    pub fn snapshot(&self) -> BTreeMap<String, GameValue> {
        self.map.clone()
    }

    /// SUBSTITUI o estado plano pelo mapa dado (o load de um save). Bump de
    /// geração: os proxies descartam o overlay de sessão no acesso seguinte.
    pub fn replace(&mut self, kv: BTreeMap<String, GameValue>) {
        self.map = kv;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// Pasta `scripts/` do mundo para `viber.load`/`viber.load_json`. Fonte
/// OMISSA do processo (a integração publica em [`ScriptHostState::scripts_dir`]
/// e o `publish_globals` copia para cá; o host do monólito semeia o app data
/// por VM em `LuaScriptHost::new`).
static SCRIPTS_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// App data POR VM: sobrepõe o omissão do processo (os testes criam VMs com
/// dirs próprios em paralelo; o static do processo seria corrida).
#[derive(Debug, Clone, Default)]
pub struct ScriptsDir(pub Option<PathBuf>);

/// Fixa a pasta de scripts do processo (host/integração).
pub fn set_scripts_dir(dir: Option<PathBuf>) {
    if let Ok(mut slot) = SCRIPTS_DIR.write() {
        *slot = dir;
    }
}

/// A pasta efectiva desta VM: app data primeiro, omissão do processo depois.
fn read_scripts_dir(lua: &Lua) -> Option<PathBuf> {
    if let Some(dir) = lua.app_data_ref::<ScriptsDir>().and_then(|d| d.0.clone()) {
        return Some(dir);
    }
    SCRIPTS_DIR.read().ok().and_then(|dir| dir.clone())
}

static GAME_STORE: OnceLock<Arc<RwLock<GameStore>>> = OnceLock::new();

/// O store real do processo (integrador e REPL); os testes injetam o seu.
pub fn game_store() -> Arc<RwLock<GameStore>> {
    GAME_STORE
        .get_or_init(|| Arc::new(RwLock::new(GameStore::default())))
        .clone()
}

/// Publica um patch no store do processo (load de save, cheat, demo…).
pub fn publish_game_patch(patch: Vec<(String, GameValue)>) {
    if let Ok(mut store) = game_store().write() {
        store.publish_patch(patch);
    }
}

/// Snapshot plano do store do processo.
pub fn game_snapshot() -> BTreeMap<String, GameValue> {
    game_store()
        .read()
        .map(|store| store.snapshot())
        .unwrap_or_default()
}

/// Sistema: aplica os patches publicados ao store do processo. Corre no
/// PreUpdate, antes dos scripts lerem `viber.game()`.
pub fn apply_game_patches() {
    if let Ok(mut store) = game_store().write() {
        store.apply_patches();
    }
}

/// Serializa o estado de jogo do STORE para JSON PLANO — só primitivos
/// (bool/número/string) são persistíveis; o overlay de sessão não existe
/// aqui (tabelas/funções nunca chegaram ao mapa). Números integrais saem
/// como inteiros JSON (o estado da tabela de VM do Viber chegava ao save
/// como `Integer`; o JSON do save não muda).
pub fn game_to_json_store(store: &Arc<RwLock<GameStore>>) -> BTreeMap<String, serde_json::Value> {
    let Ok(store) = store.read() else {
        return BTreeMap::new();
    };
    store
        .map
        .iter()
        .map(|(key, value)| (key.clone(), game_value_to_json(value)))
        .collect()
}

/// Snapshot JSON do store do PROCESSO (o save do monólito/adapter não tem
/// o handle do store).
pub fn game_to_json_process() -> BTreeMap<String, serde_json::Value> {
    game_to_json_store(&game_store())
}

/// `GameValue` → JSON: integral → `i64`, o resto `f64` (NaN/inf caem — o
/// `from_f64` do serde devolve None, como no walk da tabela do Viber).
fn game_value_to_json(value: &GameValue) -> serde_json::Value {
    match value {
        GameValue::Bool(flag) => serde_json::Value::Bool(*flag),
        GameValue::Num(number) => {
            if number.fract() == 0.0
                && *number >= -(2f64.powi(53))
                && *number <= 2f64.powi(53)
            {
                serde_json::Value::Number(serde_json::Number::from(*number as i64))
            } else {
                serde_json::Number::from_f64(*number)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            }
        }
        GameValue::Str(text) => serde_json::Value::String(text.clone()),
        GameValue::Null => serde_json::Value::Null,
    }
}

/// `serde_json::Value` → valor Luau. Números inteiros que cabem em i64
/// entram como inteiros; o resto como float. `null` → nil.
pub fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        serde_json::Value::Null => mlua::Value::Nil,
        serde_json::Value::Bool(flag) => mlua::Value::Boolean(*flag),
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                mlua::Value::Integer(int)
            } else if let Some(float) = number.as_f64() {
                mlua::Value::Number(float)
            } else {
                mlua::Value::Nil
            }
        }
        serde_json::Value::String(text) => mlua::Value::String(lua.create_string(text)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table()?;
            for (index, item) in items.iter().enumerate() {
                table.raw_set(index + 1, json_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table()?;
            for (key, item) in map {
                table.raw_set(key.as_str(), json_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
    })
}

/// Repõe o JSON do save no STORE (load): SUBSTITUI o estado — chaves que só
/// existem na sessão não sobrevivem (bump de geração descarta os overlays).
/// Valores não primitivos no ficheiro (editado à mão) são ignorados — nunca
/// partem o load. Portado do `json_to_game` do monólito (exit4/F3).
pub fn restore_game_json(kv: &BTreeMap<String, serde_json::Value>) {
    let map: BTreeMap<String, GameValue> = kv
        .iter()
        .filter_map(|(key, value)| {
            let gv = match value {
                serde_json::Value::Bool(flag) => GameValue::Bool(*flag),
                serde_json::Value::Number(number) => {
                    GameValue::Num(number.as_f64().unwrap_or(0.0))
                }
                serde_json::Value::String(text) => GameValue::Str(text.clone()),
                _ => return None,
            };
            Some((key.clone(), gv))
        })
        .collect();
    if let Ok(mut store) = game_store().write() {
        store.replace(map);
    }
}

/// COMPAT (exit4/F3): a assinatura que o jogo (Vael) chamava quando o
/// estado vivia na tabela `viber_game` da VM. O `lua` ficou na assinatura
/// por compatibilidade de chamadas — o estado agora vive no STORE do
/// processo, e a escrita é imediata (o monólito escrevia com `raw_set`
/// sem fila). `Null` remove a chave; não-primitivos são ignorados.
pub fn apply_patch(
    _lua: &Lua,
    patch: &std::collections::BTreeMap<String, serde_json::Value>,
) {
    let map: BTreeMap<String, GameValue> = patch
        .iter()
        .filter_map(|(key, value)| {
            let gv = match value {
                serde_json::Value::Bool(flag) => GameValue::Bool(*flag),
                serde_json::Value::Number(number) => {
                    GameValue::Num(number.as_f64().unwrap_or(0.0))
                }
                serde_json::Value::String(text) => GameValue::Str(text.clone()),
                _ => return None,
            };
            Some((key.clone(), gv))
        })
        .collect();
    if let Ok(mut store) = game_store().write() {
        for (key, value) in map {
            match value {
                GameValue::Null => {
                    store.map.remove(&key);
                }
                other => {
                    store.map.insert(key, other);
                }
            }
        }
    }
}

/// COMPAT (exit4/F3): a assinatura que o jogo chamava para serializar o
/// estado da VM — agora é o snapshot do store do processo (o `lua` fica
/// só por compatibilidade de chamadas). A variante que recebe o store é
/// [`game_to_json_store`].
pub fn game_to_json(_lua: &Lua) -> BTreeMap<String, serde_json::Value> {
    game_to_json_process()
}

/// Tira `path` do cache do `viber.load` (hot-reload de um módulo). Devolve
/// `true` se estava carregado. Portado do monólito — opera no registry da
/// VM dada (o host do monólito e os contextos BMS usam a mesma chave
/// `viber_modules`).
pub fn evict_module(lua: &Lua, path: &str) -> bool {
    let Ok(modules) = lua.named_registry_value::<Table>("viber_modules") else {
        return false;
    };
    let cached = modules
        .raw_get::<mlua::Value>(path)
        .is_ok_and(|v| !matches!(v, mlua::Value::Nil));
    if cached {
        let _ = modules.raw_remove(path);
    }
    cached
}

/// Limpa o estado do processo (TESTES: o store é partilhado pelo processo —
/// cada teste semeia o seu sem interferir nos outros).
#[doc(hidden)]
pub fn reset_game_state_for_tests() {
    if let Ok(mut store) = game_store().write() {
        *store = GameStore::default();
    }
}

/// Instala `viber.game()`, `viber.state()`, `viber.load` e `viber.load_json`
/// na tabela `viber`. (exit4/F3: o grupo `game` é um só — o monólito chama
/// esta instalação pelo shim `viber::luau::game`.)
pub fn install(lua: &Lua, viber: &Table, store: Arc<RwLock<GameStore>>) -> mlua::Result<()> {
    let proxy = build_game_proxy(lua, store)?;

    // viber.game() — o MESMO proxy em todas as chamadas (`local game =
    // viber.game()` guarda identidade, não snapshot). Primitivos vivem no
    // mapa: as leituras via __index e as iterações veem sempre o estado
    // actual, incluindo patches e escritas de outras VMs.
    let game_proxy = proxy.clone();
    viber.set(
        "game",
        lua.create_function(move |_, ()| Ok(game_proxy.clone()))?,
    )?;

    // `pairs(proxy)` no Luau não consulta metamétodos (o `for .. in` sim):
    // envolvemos o `pairs` da VM para delegar em `__iter` quando existir.
    // Os restantes pares/tabelas continuam no `pairs` cru.
    if !lua.named_registry_value::<bool>("viber_pairs_wrapped").unwrap_or(false) {
        lua.load(
            r#"local rawpairs = pairs
pairs = function(obj)
  local mt = type(obj) == "table" and getmetatable(obj) or nil
  if mt and mt.__iter then return mt.__iter(obj) end
  return rawpairs(obj)
end"#,
        )
        .set_name("=viber.pairs")
        .exec()?;
        lua.set_named_registry_value("viber_pairs_wrapped", true)?;
    }

    // viber.state() — tabela real da própria VM (por entidade no BMS).
    let state = lua.create_table()?;
    lua.set_named_registry_value("viber_state", state.clone())?;
    viber.set(
        "state",
        lua.create_function(move |_, ()| Ok(state.clone()))?,
    )?;

    // viber.load(path) — corre um chunk de `scripts/` num env próprio UMA
    // vez por VM e devolve o `return` (cache em `viber_modules`). A chave
    // do cache é o caminho RESOLVIDO (sem o prefixo histórico `scripts/`,
    // `.`/`//` normalizados) — a mesma grafia em todas as chamadas.
    // PENDENTE na integração: evict no hot-reload dos módulos.
    viber.set(
        "load",
        lua.create_function(move |lua, path: String| {
            // Só caminhos RELATIVOS dentro de `scripts/` — `..` ou raiz
            // absoluta liam (e executavam) qualquer ficheiro do disco.
            let inside = std::path::Path::new(&path)
                .components()
                .all(|c| {
                    matches!(
                        c,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                });
            if path.is_empty() || !inside {
                return Err(mlua::Error::runtime(format!(
                    "viber.load('{path}'): só caminhos relativos a scripts/ (sem '..')"
                )));
            }
            let key: String = std::path::Path::new(path.strip_prefix("scripts/").unwrap_or(&path))
                .components()
                .filter_map(|component| match component {
                    std::path::Component::Normal(part) => {
                        Some(part.to_string_lossy().into_owned())
                    }
                    // `..`/raiz já foram rejeitados acima; `.` cai.
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            if key.is_empty() {
                return Err(mlua::Error::runtime(format!(
                    "viber.load('{path}'): caminho sem ficheiro"
                )));
            }
            let modules: mlua::Table = lua.named_registry_value("viber_modules")?;
            if let Ok(cached) = modules.raw_get::<mlua::Value>(key.clone())
                && !cached.is_nil()
            {
                return Ok(cached);
            }
            let dir = read_scripts_dir(lua).ok_or_else(|| {
                mlua::Error::runtime(
                    "viber.load sem scripts_dir (nem app data por VM, nem publicado no ScriptHostState)",
                )
            })?;
            let full = dir.join(&key);
            let code = std::fs::read_to_string(&full).map_err(|error| {
                mlua::Error::runtime(format!("viber.load('{path}'): {error}"))
            })?;
            // Env sandbox próprio (mesma forma do chunk de entidade) — o
            // módulo não pode clobber globals de quem o carrega.
            let env = lua.create_table()?;
            let mt = lua.create_table()?;
            mt.set("__index", lua.globals())?;
            env.set_metatable(Some(mt))?;
            let chunk = lua
                .load(&code)
                .set_name(format!("={path}"))
                .set_environment(env)
                .into_function()?;
            let value: mlua::Value = chunk.call(())?;
            // Módulo sem `return` (só mexe em `viber.game()`) cacheia `true`,
            // como o `require` — com `nil` o cache falhava e o módulo
            // re-corria a CADA chamada.
            let value = match value {
                mlua::Value::Nil => mlua::Value::Boolean(true),
                other => other,
            };
            modules.raw_set(key, value.clone())?;
            Ok(value)
        })?,
    )?;

    // viber.load_json(caminho) -> valor. Caminho relativo à pasta do mundo
    // (pai de scripts/). Sem `..`. Objectos/arrays/números/strings/bool/null.
    viber.set(
        "load_json",
        lua.create_function(|lua, path: String| {
            let relative = std::path::Path::new(&path);
            let inside = relative.components().all(|c| {
                matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
            if path.is_empty() || !inside {
                return Err(mlua::Error::runtime(format!(
                    "viber.load_json('{path}'): só caminhos relativos à pasta do mundo (sem '..')"
                )));
            }
            let scripts =
                read_scripts_dir(lua).ok_or_else(|| mlua::Error::runtime("viber.load_json sem scripts_dir"))?;
            let world = scripts.parent().ok_or_else(|| {
                mlua::Error::runtime("viber.load_json: scripts/ sem pasta de mundo")
            })?;
            let full = world.join(&path);
            let text = std::fs::read_to_string(&full)
                .map_err(|e| mlua::Error::runtime(format!("viber.load_json('{path}'): {e}")))?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| mlua::Error::runtime(format!("viber.load_json('{path}'): {e}")))?;
            json_to_lua(lua, &value)
        })?,
    )?;

    // Cache dos módulos do `viber.load` (por VM, sobrevive a hot-reload do
    // script como o `viber_modules` do Viber); evictado por [`evict_module`]
    // (hot-reload/bridge).
    lua.set_named_registry_value("viber_modules", lua.create_table()?)?;
    Ok(())
}

/// O proxy: tabela SEMPRE vazia + metamétodos que atravessam para o
/// overlay de sessão (tabela escondida da mesma VM) e para o mapa
/// partilhado. O overlay nunca vive em cru no proxy — uma leitura cru
/// saltaria o `__index` e o wipe por geração nunca dispara. O overlay é
/// descartado quando o store muda de geração (`GameStore::replace` — o
/// load de um save substitui o estado, como o `json_to_game` que limpava
/// a tabela da VM no Viber).
fn build_game_proxy(lua: &Lua, store: Arc<RwLock<GameStore>>) -> mlua::Result<Table> {
    let proxy = lua.create_table()?;
    // Overlay de sessão (tabelas/funções) — por VM, escondido no registry.
    let overlay = lua.create_table()?;
    lua.set_named_registry_value("viber_game_overlay", overlay.clone())?;
    let mt = lua.create_table()?;

    // Geração vista por este proxy (partilhada pelos metamétodos —
    // a descoberta num deles descarta o overlay para todos).
    let seen_generation = Arc::new(std::sync::atomic::AtomicU64::new(
        store.read().map(|s| s.generation).unwrap_or(0),
    ));

    // __index — overlay de sessão primeiro, mapa partilhado depois.
    let index_store = store.clone();
    let index_gen = seen_generation.clone();
    let index_overlay = overlay.clone();
    mt.set(
        "__index",
        // O 1.º argumento do `__index` é a própria tabela — consome-a antes
        // da chave.
        lua.create_function(move |lua, (_proxy, key): (Table, Value)| -> mlua::Result<Value> {
            sync_generation(&index_store, &index_gen, &index_overlay);
            let Value::String(key) = &key else {
                return Ok(Value::Nil);
            };
            let name = key.to_string_lossy();
            if let Ok(value) = index_overlay.raw_get::<mlua::Value>(name.as_str())
                && !value.is_nil()
            {
                return Ok(value);
            }
            let value = index_store
                .read()
                .ok()
                .and_then(|s| s.map.get(&name).cloned());
            match value {
                Some(GameValue::Null) | None => Ok(Value::Nil),
                Some(gv) => gv.into_lua(lua),
            }
        })?,
    )?;

    // __newindex — primitivos escrevem no mapa (nil remove); tabelas e
    // funções ficam no overlay de sessão desta VM (invisível ao save).
    let newindex_store = store.clone();
    let newindex_gen = seen_generation.clone();
    let newindex_overlay = overlay.clone();
    mt.set(
        "__newindex",
        lua.create_function(
            move |_, (_proxy, key, value): (Table, Value, Value)| -> mlua::Result<()> {
                sync_generation(&newindex_store, &newindex_gen, &newindex_overlay);
                let Value::String(key) = &key else {
                    return Err(mlua::Error::runtime(
                        "viber.game(): as chaves têm de ser strings",
                    ));
                };
                let name = key.to_string_lossy();
                match GameValue::from_lua(&value) {
                    // Primitivo: só o mapa partilhado (e limpa o overlay —
                    // `game.k = nil` depois de `game.k = {}` remove tudo).
                    Some(gv) => {
                        let _ = newindex_overlay.raw_remove(name.as_str());
                        if let Ok(mut s) = newindex_store.write() {
                            match gv {
                                GameValue::Null => {
                                    s.map.remove(&name);
                                }
                                other => {
                                    s.map.insert(name, other);
                                }
                            }
                        }
                    }
                    // Tabela/função: overlay de sessão, só nesta VM.
                    None => newindex_overlay.raw_set(name, value)?,
                }
                Ok(())
            },
        )?,
    )?;

    // __iter — iteração genérica (`for k, v in viber.game() do` e o `pairs`
    // envolvido): funde o overlay de sessão (cru) com as chaves do mapa
    // partilhado. Os valores vêm de `t[k]` (via __index).
    let iter_store = store;
    let iter_gen = seen_generation;
    let iter_overlay = overlay;
    mt.set(
        "__iter",
        lua.create_function(move |lua, proxy: Table| -> mlua::Result<Function> {
            sync_generation(&iter_store, &iter_gen, &iter_overlay);
            let mut keys: Vec<Value> = iter_overlay
                .pairs::<Value, Value>()
                .flatten()
                .map(|(k, _)| k)
                .collect();
            if let Ok(s) = iter_store.read() {
                for key in s.map.keys() {
                    keys.push(Value::String(lua.create_string(key)?));
                }
            }
            // Overlay de sessão ganha na colisão (1.ª ocorrência).
            let mut seen = std::collections::HashSet::new();
            keys.retain(|k| match k {
                Value::String(s) => seen.insert(s.to_string_lossy()),
                other => seen.insert(format!("{other:?}")),
            });
            let mut next = 0usize;
            lua.create_function_mut(move |lua, ()| -> mlua::Result<MultiValue> {
                let Some(key) = keys.get(next).cloned() else {
                    return Ok(MultiValue::new());
                };
                next += 1;
                let value: Value = proxy.get(key.clone())?;
                Ok(MultiValue::from_vec(vec![
                    key.into_lua(lua)?,
                    value.into_lua(lua)?,
                ]))
            })
        })?,
    )?;

    proxy.set_metatable(Some(mt))?;
    Ok(proxy)
}

/// Descarta o overlay de sessão quando o store mudou de geração (o
/// `GameStore::replace` do load substitui o estado — no Viber o
/// `json_to_game` limpava a tabela da VM por inteiro).
fn sync_generation(
    store: &Arc<RwLock<GameStore>>,
    seen: &std::sync::atomic::AtomicU64,
    overlay: &Table,
) {
    let Ok(store) = store.read() else {
        return;
    };
    let seen_generation = seen.load(std::sync::atomic::Ordering::Relaxed);
    if store.generation != seen_generation {
        seen.store(store.generation, std::sync::atomic::Ordering::Relaxed);
        drop(store);
        // O overlay cru é pequeno (só tabelas/funções de sessão): remove por
        // chave — o mlua 0.12 não tem `raw_clear`.
        let stale: Vec<Value> = overlay
            .pairs::<Value, Value>()
            .flatten()
            .map(|(k, _)| k)
            .collect();
        for key in stale {
            let _ = overlay.raw_remove(key);
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn store() -> Arc<RwLock<GameStore>> {
        Arc::new(RwLock::new(GameStore::default()))
    }

    /// O store de `viber.game()` é do PROCESSO — os testes que o tocam tomam
    /// este lock (correr em paralelo apagava estado uns dos outros).
    static PROCESS_STORE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fresh_store() -> Arc<RwLock<GameStore>> {
        Arc::new(RwLock::new(GameStore::default()))
    }

    #[test]
    fn json_round_trip_só_primitivos() {
        let _lock = PROCESS_STORE_TEST_LOCK.lock().expect("lock");
        let store = store();
        let other = fresh_store();
        {
            let mut guard = store.write().expect("store");
            guard.publish_patch(vec![
                ("n".into(), GameValue::Num(7.0)),
                ("s".into(), GameValue::Str("x".into())),
                ("b".into(), GameValue::Bool(true)),
            ]);
            guard.apply_patches();
        }
        let json = game_to_json_store(&store);
        assert_eq!(json.get("n"), Some(&json!(7)), "integral sai como inteiro");
        assert_eq!(json.get("s"), Some(&json!("x")));
        assert_eq!(json.get("b"), Some(&json!(true)));

        restore_game_json(&game_to_json_store(&other) /* vazio */);
        // restore sobre o PROCESSO: semear por patch e ver no snapshot.
        let mut kv = BTreeMap::new();
        kv.insert("n".to_string(), json!(7));
        kv.insert("s".to_string(), json!("x"));
        kv.insert("b".to_string(), json!(true));
        restore_game_json(&kv);
        let snapshot = game_snapshot();
        assert_eq!(snapshot.get("n"), Some(&GameValue::Num(7.0)));
        assert_eq!(snapshot.get("s"), Some(&GameValue::Str("x".into())));
        assert_eq!(snapshot.get("b"), Some(&GameValue::Bool(true)));
    }

    #[test]
    fn restore_substitui_e_bumpa_a_geração() {
        let _lock = PROCESS_STORE_TEST_LOCK.lock().expect("lock");
        let store = store();
        {
            let mut s = store.write().expect("store");
            s.map.insert("stale".into(), GameValue::Num(1.0));
            s.map.insert("keep".into(), GameValue::Str("old".into()));
        }
        let gen_before = store.read().expect("store").generation;
        let mut kv = BTreeMap::new();
        kv.insert("keep".to_string(), json!("new"));
        restore_game_json(&kv);
        // restore opera no store do PROCESSO — o store local não muda.
        assert_eq!(store.read().expect("store").generation, gen_before);
        let process = game_store();
        let gen_before_process = process.read().expect("store").generation;
        restore_game_json(&kv);
        let snapshot = game_snapshot();
        assert!(!snapshot.contains_key("stale"), "load substitui");
        assert_eq!(snapshot.get("keep"), Some(&GameValue::Str("new".into())));
        assert!(
            process.read().expect("store").generation > gen_before_process,
            "bump de geração descarta overlays"
        );
        reset_game_state_for_tests();
    }

    #[test]
    fn overlay_de_sessão_morre_no_load() {
        let store = store();
        let lua = Lua::new();
        let viber = lua.create_table().expect("viber");
        install(&lua, &viber, store.clone()).expect("instala");
        lua.globals().set("viber", viber).expect("global");
        lua.load("viber.game().tabela = { 1, 2 }")
            .exec()
            .expect("overlay");
        lua.load("viber.game().primitivo = 5")
            .exec()
            .expect("primitivo");
        // Load: substitui o estado (bump de geração) — o overlay morre.
        // (O `restore_game_json` do processo é o mesmo caminho; aqui o store
        // é injetado, logo a substituição directa.)
        let mut kv = BTreeMap::new();
        kv.insert("novo".to_string(), GameValue::Num(1.0));
        store.write().expect("store").replace(kv);
        let overlay_morreu: bool = lua
            .load("local g = viber.game() return g.tabela == nil and g.novo == 1")
            .eval()
            .expect("wipe");
        assert!(overlay_morreu, "tabela de sessão sobreviveu ao load");
    }

    #[test]
    fn evict_module_confirma_e_remove() {
        let lua = Lua::new();
        install(&lua, &lua.create_table().expect("viber"), store()).expect("instala");
        // Sem módulo: false.
        assert!(!evict_module(&lua, "x.lua"));
        // Semear o cache e evictar.
        let modules: mlua::Table = lua.named_registry_value("viber_modules").expect("cache");
        modules
            .raw_set("x.lua", lua.create_string("modulo").expect("valor"))
            .expect("semear");
        assert!(evict_module(&lua, "x.lua"));
        assert!(!evict_module(&lua, "x.lua"), "2.ª vez já não está");
    }

    #[test]
    fn json_to_lua_inteiros_arrays_e_null() {
        let lua = Lua::new();
        let value = json!({ "a": 5, "b": 1.5, "c": null, "d": [1, "dois"] });
        let table = json_to_lua(&lua, &value).expect("conversão");
        let table = table.as_table().expect("tabela").clone();
        assert_eq!(
            table.raw_get::<mlua::Integer>("a").expect("a"),
            5,
            "integral entra como Integer"
        );
        assert_eq!(table.raw_get::<f64>("b").expect("b"), 1.5);
        assert!(table.raw_get::<mlua::Value>("c").expect("c").is_nil());
        let array: mlua::Table = table.raw_get("d").expect("d");
        assert_eq!(array.raw_len(), 2);
    }
}
