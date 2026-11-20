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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::SystemTime;

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
    /// Versão das chaves planas (`!is_nested_key`): incrementada sempre que
    /// uma chave persistível muda, entra, sai ou o estado é substituído.
    flat_version: u64,
}

impl GameStore {
    /// Versão actual das chaves planas (`!is_nested_key`).
    pub fn version(&self) -> u64 {
        self.flat_version
    }

    /// Leitura directa de uma chave no store (zero clones).
    pub fn get(&self, key: &str) -> Option<&GameValue> {
        self.map.get(key)
    }

    /// `true` quando o store não tem nenhuma chave.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn bump_flat_version(&mut self) {
        self.flat_version = self.flat_version.wrapping_add(1);
    }

    fn apply_entry(&mut self, key: String, value: GameValue) -> bool {
        let flat = !is_nested_key(&key);
        let changed = match value {
            GameValue::Null => self.map.remove(&key).is_some(),
            other => match self.map.get_mut(&key) {
                Some(slot) if *slot == other => false,
                Some(slot) => {
                    *slot = other;
                    true
                }
                None => {
                    self.map.insert(key, other);
                    true
                }
            },
        };
        if changed && flat {
            self.bump_flat_version();
        }
        changed
    }

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
                self.apply_entry(key, value);
            }
        }
    }

    /// Snapshot plano (persistência/diagnóstico). O overlay de sessão
    /// (tabelas/funções) não aparece — só o que o save persistiria.
    pub fn snapshot(&self) -> BTreeMap<String, GameValue> {
        self.map
            .iter()
            .filter(|(key, _)| !is_nested_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// SUBSTITUI o estado plano pelo mapa dado (o load de um save). Bump de
    /// geração: os proxies descartam o overlay de sessão no acesso seguinte.
    pub fn replace(&mut self, kv: BTreeMap<String, GameValue>) {
        self.map = kv;
        self.generation = self.generation.wrapping_add(1);
        self.bump_flat_version();
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
pub(crate) fn read_scripts_dir(lua: &Lua) -> Option<PathBuf> {
    if let Some(dir) = lua.app_data_ref::<ScriptsDir>().and_then(|d| d.0.clone()) {
        return Some(dir);
    }
    SCRIPTS_DIR.read().ok().and_then(|dir| dir.clone())
}

/// A pasta de scripts publicada pelo integrador (omissão do processo), para
/// semear o app data POR VM em [`crate::init_context`]/REPL — o top-level de
/// um script (e a REPL) pede `viber.load`/`decision.load`/`nlu.load` ANTES do
/// primeiro `publish_globals`; só com o static isso dava "sem scripts_dir"
/// (visto no `hud.lua` a 2026-11-15).
pub(crate) fn process_scripts_dir() -> Option<PathBuf> {
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
    let _probe = bevy_ext_core::perf_probe::Probe::new("apply_game_patches"); // `VAEL_PERF_DIAG`
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

/// Versão actual das chaves planas do store do processo (`0` se falhar o lock).
pub fn game_version() -> u64 {
    game_store()
        .read()
        .map(|store| store.version())
        .unwrap_or(0)
}

/// Leitura pontual de uma chave do store do processo (sem clonar o mapa).
pub fn game_get(key: &str) -> Option<GameValue> {
    game_store()
        .read()
        .ok()
        .and_then(|store| store.get(key).cloned())
}

/// Leitura pontual em JSON de uma chave do store do processo.
pub fn game_get_json(key: &str) -> Option<serde_json::Value> {
    game_store()
        .read()
        .ok()
        .and_then(|store| store.get(key).map(game_value_to_json))
}

/// Leitura pontual booleana de uma chave do store do processo.
pub fn game_get_bool(key: &str) -> Option<bool> {
    game_store()
        .read()
        .ok()
        .and_then(|store| match store.get(key)? {
            GameValue::Bool(flag) => Some(*flag),
            _ => None,
        })
}

/// Executa `f` com acesso de leitura ao [`GameStore`] do processo num único lock.
pub fn with_game_store<R>(f: impl FnOnce(&GameStore) -> R) -> Option<R> {
    game_store().read().ok().map(|store| f(&store))
}

/// Sistema: aplica os patches publicados ao store do processo. Corre no
/// PreUpdate, antes dos scripts lerem `viber.game()`.
pub fn apply_game_patches() {
    let _probe = bevy_ext_core::perf_probe::Probe::new("luau.game_patches");
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
        // As tabelas partilhadas (`game.enemies`…) são de sessão: fora do save.
        .filter(|(key, _)| !is_nested_key(key))
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
pub fn game_value_to_json(value: &GameValue) -> serde_json::Value {
    match value {
        GameValue::Bool(flag) => serde_json::Value::Bool(*flag),
        GameValue::Num(number) => {
            if number.fract() == 0.0 && *number >= -(2f64.powi(53)) && *number <= 2f64.powi(53) {
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
                serde_json::Value::Number(number) => GameValue::Num(number.as_f64().unwrap_or(0.0)),
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
pub fn apply_patch(_lua: &Lua, patch: &std::collections::BTreeMap<String, serde_json::Value>) {
    apply_patch_json(patch);
}

/// Compat SEM VM: o mesmo [`apply_patch`] para quem não tem handle mlua
/// (o jogo em modo BMS — os caminhos Rust do estado nunca tocam em Lua).
pub fn apply_patch_json(patch: &std::collections::BTreeMap<String, serde_json::Value>) {
    let map: BTreeMap<String, GameValue> = patch
        .iter()
        .filter_map(|(key, value)| {
            let gv = match value {
                // `null` REMOVE a chave (o `__newindex` do proxy mapeia o
                // `nil` para `Null`). Antes caía no `_ => None` e o patch de
                // remoção era silenciosamente descartado — o gate do
                // `state_sync` ficava preso no jogo depois de um save
                // escrito com a autoridade ligada.
                serde_json::Value::Null => GameValue::Null,
                serde_json::Value::Bool(flag) => GameValue::Bool(*flag),
                serde_json::Value::Number(number) => GameValue::Num(number.as_f64().unwrap_or(0.0)),
                serde_json::Value::String(text) => GameValue::Str(text.clone()),
                _ => return None,
            };
            Some((key.clone(), gv))
        })
        .collect();
    if let Ok(mut store) = game_store().write() {
        for (key, value) in map {
            store.apply_entry(key, value);
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

/// A tabela `viber.game()` da VM dada — o MESMO proxy que o Lua usa.
///
/// Os sistemas Rust do jogo leem/escrevem o estado por aqui desde que o
/// `viber_game` do registry morreu (exit4/F3): o estado plano vive no store
/// do processo e o proxy atravessa-o. Usar `get`/`set` (metamétodos), NUNCA
/// `raw_get`/`raw_set` — os primitivos estão atrás do `__index`/`__newindex`
/// e uma leitura crua via a tabela vazia. `set(k, Nil)` remove a chave.
pub fn game_proxy(lua: &Lua) -> mlua::Result<Table> {
    let viber: Table = lua
        .globals()
        .get("vael")
        .or_else(|_| lua.globals().get("viber"))?;
    let game: Function = viber.get("game")?;
    game.call(())
}

#[derive(Clone)]
struct GameStoreRef(Arc<RwLock<GameStore>>);

/// Snapshot dos primitivos de `viber.game()` **desta VM** (proxy incluído).
///
/// É o equivalente de [`game_to_json`] pela tabela da VM em vez do store do
/// PROCESSO: um módulo que lê e escreve pelo proxy fica sempre na mesma
/// fonte (apps com store próprio e testes com uma VM à mão — o
/// `game_to_json`/`apply_patch` COMPAT apontam sempre ao store do processo,
/// que numa VM de teste não é o mesmo).
///
/// Tabelas/funções (overlay de sessão) ficam de fora, como no save; `nil`
/// entra como `null` (o `GameStore::Null`).
pub fn game_snapshot_vm(lua: &Lua) -> BTreeMap<String, serde_json::Value> {
    if let Some(store) = lua.app_data_ref::<GameStoreRef>() {
        return game_to_json_store(&store.0);
    }
    let mut out = BTreeMap::new();
    let Ok(game) = game_proxy(lua) else {
        return out;
    };
    // O proxy guarda tudo atrás de metamétodos: o `pairs` cru do mlua não o
    // vê. Com `__iter` (o proxy do `install`) iteramos pela função; numa
    // tabela simples (teste) o `pairs` cru serve.
    let iter = game
        .metatable()
        .and_then(|mt| mt.get::<Option<Function>>("__iter").ok().flatten());
    if let Some(iter) = iter {
        let Ok(next) = iter.call::<Function>(game.clone()) else {
            return out;
        };
        loop {
            let Ok(values) = next.call::<MultiValue>(()) else {
                break;
            };
            if values.is_empty() {
                break;
            }
            let mut values = values.into_iter();
            let (Some(key), Some(value)) = (values.next(), values.next()) else {
                break;
            };
            insert_primitive_json(&mut out, key, value);
        }
    } else {
        for (key, value) in game.pairs::<Value, Value>().flatten() {
            insert_primitive_json(&mut out, key, value);
        }
    }
    out
}

/// `chave`/`valor` Lua → mapa JSON plano. Chaves não-string e valores
/// não-primitivos (tabelas/funções) ficam de fora.
fn insert_primitive_json(out: &mut BTreeMap<String, serde_json::Value>, key: Value, value: Value) {
    let Value::String(key) = key else {
        return;
    };
    let Ok(key) = key.to_str() else {
        return;
    };
    let json = match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(flag) => serde_json::Value::Bool(flag),
        Value::Integer(number) => serde_json::Value::Number(serde_json::Number::from(number)),
        Value::Number(number) => match serde_json::Number::from_f64(number) {
            Some(number) => serde_json::Value::Number(number),
            None => return,
        },
        Value::String(text) => match text.to_str() {
            Ok(text) => serde_json::Value::String(text.to_string()),
            Err(_) => return,
        },
        _ => return,
    };
    out.insert(key.to_string(), json);
}

type FileStamp = (Option<SystemTime>, u64);

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

static SCRIPT_SOURCE_CACHE: OnceLock<RwLock<HashMap<PathBuf, (FileStamp, Arc<str>)>>> =
    OnceLock::new();
static JSON_VALUE_CACHE: OnceLock<RwLock<HashMap<PathBuf, (FileStamp, Arc<serde_json::Value>)>>> =
    OnceLock::new();

fn script_source_cache() -> &'static RwLock<HashMap<PathBuf, (FileStamp, Arc<str>)>> {
    SCRIPT_SOURCE_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn json_value_cache() -> &'static RwLock<HashMap<PathBuf, (FileStamp, Arc<serde_json::Value>)>> {
    JSON_VALUE_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn load_cached_script(path: &Path) -> Result<Arc<str>, std::io::Error> {
    let stamp = file_stamp(path);
    if let Some(stamp) = stamp
        && let Ok(cache) = script_source_cache().read()
        && let Some((cached_stamp, src)) = cache.get(path)
        && *cached_stamp == stamp
    {
        return Ok(src.clone());
    }
    let code = std::fs::read_to_string(path)?;
    let arc: Arc<str> = Arc::from(code);
    if let Some(stamp) = stamp
        && let Ok(mut cache) = script_source_cache().write()
    {
        cache.insert(path.to_path_buf(), (stamp, arc.clone()));
    }
    Ok(arc)
}

fn load_cached_json(path: &Path) -> Result<Arc<serde_json::Value>, String> {
    let stamp = file_stamp(path);
    if let Some(stamp) = stamp
        && let Ok(cache) = json_value_cache().read()
        && let Some((cached_stamp, val)) = cache.get(path)
        && *cached_stamp == stamp
    {
        return Ok(val.clone());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let arc = Arc::new(value);
    if let Some(stamp) = stamp
        && let Ok(mut cache) = json_value_cache().write()
    {
        cache.insert(path.to_path_buf(), (stamp, arc.clone()));
    }
    Ok(arc)
}

/// Tira `path` do cache do `viber.load` (hot-reload de um módulo). Devolve
/// `true` se estava carregado. Portado do monólito — opera no registry da
/// VM dada (o host do monólito e os contextos BMS usam a mesma chave
/// `viber_modules`).
pub fn evict_module(lua: &Lua, path: &str) -> bool {
    if let Some(dir) = read_scripts_dir(lua)
        && let Ok(mut cache) = script_source_cache().write()
    {
        cache.remove(&dir.join(path));
    }
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
    lua.set_app_data(GameStoreRef(store.clone()));
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
    if !lua
        .named_registry_value::<bool>("viber_pairs_wrapped")
        .unwrap_or(false)
    {
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
    // NÃO pisa um `state` já instalado: o host do monólito registou o dele
    // (per-entity, `viber_states[entidade]`) ANTES de chamar este install, e
    // o per-VM aqui colidia as chaves de scripts de entidades diferentes no
    // mesmo `st` (o `st.cd` da hotbar virava a tabela de cool-downs das
    // habilidades e o `math.max` rebentava no `on_update`).
    if !viber.contains_key("state")? {
        let state = lua.create_table()?;
        lua.set_named_registry_value("viber_state", state.clone())?;
        viber.set(
            "state",
            lua.create_function(move |_, ()| Ok(state.clone()))?,
        )?;
    }

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
            let code = load_cached_script(&full).map_err(|error| {
                mlua::Error::runtime(format!("viber.load('{path}'): {error}"))
            })?;
            // Env sandbox próprio (mesma forma do chunk de entidade) — o
            // módulo não pode clobber globals de quem o carrega.
            let env = lua.create_table()?;
            let mt = lua.create_table()?;
            mt.set("__index", lua.globals())?;
            env.set_metatable(Some(mt))?;
            let chunk = lua
                .load(&*code)
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
            let scripts = read_scripts_dir(lua)
                .ok_or_else(|| mlua::Error::runtime("viber.load_json sem scripts_dir"))?;
            let world = scripts.parent().ok_or_else(|| {
                mlua::Error::runtime("viber.load_json: scripts/ sem pasta de mundo")
            })?;
            let full = world.join(&path);
            let value = load_cached_json(&full)
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
    // Overlay de sessão (FUNÇÕES) — por VM, escondido no registry. As
    // tabelas já não vivem aqui: são partilhadas pelo store (ver
    // [`nested_proxy`]).
    let overlay = lua.create_table()?;
    lua.set_named_registry_value("viber_game_overlay", overlay.clone())?;
    // Cache de identidade dos proxies aninhados (valores fracos): o mesmo
    // caminho devolve a mesma tabela enquanto alguém a segurar.
    let cache = lua.create_table()?;
    let cache_mt = lua.create_table()?;
    cache_mt.set("__mode", "v")?;
    cache.set_metatable(Some(cache_mt))?;
    lua.set_named_registry_value("viber_game_nested", cache)?;
    let mt = lua.create_table()?;

    // Geração vista por este proxy (partilhada pelos metamétodos —
    // a descoberta num deles descarta o overlay para todos).
    let seen_generation = Arc::new(std::sync::atomic::AtomicU64::new(
        store.read().map(|s| s.generation).unwrap_or(0),
    ));

    // __index — overlay de sessão primeiro, mapa partilhado depois (um
    // primitivo, ou uma tabela partilhada → proxy aninhado).
    let index_store = store.clone();
    let index_gen = seen_generation.clone();
    let index_overlay = overlay.clone();
    mt.set(
        "__index",
        // O 1.º argumento do `__index` é a própria tabela — consome-a antes
        // da chave.
        lua.create_function(
            move |lua, (_proxy, key): (Table, Value)| -> mlua::Result<Value> {
                let Value::String(key) = &key else {
                    return Ok(Value::Nil);
                };
                let borrowed = key.to_str().ok();
                let owned;
                let name: &str = match &borrowed {
                    Some(s) => s,
                    None => {
                        owned = key.to_string_lossy();
                        &owned
                    }
                };
                if !index_overlay.is_empty() {
                    sync_generation(&index_store, &index_gen, &index_overlay);
                    if let Ok(value) = index_overlay.raw_get::<mlua::Value>(name)
                        && !value.is_nil()
                    {
                        return Ok(value);
                    }
                }
                read_path(lua, &index_store, name)
            },
        )?,
    )?;

    // __newindex — primitivos e TABELAS escrevem no store partilhado (nil
    // remove); funções ficam no overlay de sessão desta VM.
    let newindex_store = store.clone();
    let newindex_gen = seen_generation.clone();
    let newindex_overlay = overlay.clone();
    mt.set(
        "__newindex",
        lua.create_function(
            move |_, (_proxy, key, value): (Table, Value, Value)| -> mlua::Result<()> {
                let Value::String(key) = &key else {
                    return Err(mlua::Error::runtime(
                        "viber.game(): as chaves têm de ser strings",
                    ));
                };
                let borrowed = key.to_str().ok();
                let owned;
                let name: &str = match &borrowed {
                    Some(s) => s,
                    None => {
                        owned = key.to_string_lossy();
                        &owned
                    }
                };
                if matches!(
                    value,
                    Value::Function(_) | Value::UserData(_) | Value::Thread(_)
                ) {
                    sync_generation(&newindex_store, &newindex_gen, &newindex_overlay);
                    newindex_overlay.raw_set(name, value)?;
                    return Ok(());
                }
                if !newindex_overlay.is_empty() {
                    sync_generation(&newindex_store, &newindex_gen, &newindex_overlay);
                    let _ = newindex_overlay.raw_remove(name);
                }
                write_path(&newindex_store, name, &value)
            },
        )?,
    )?;

    // __iter — iteração genérica (`for k, v in viber.game() do` e o `pairs`
    // envolvido): funde o overlay de sessão com as chaves do mapa (as raízes
    // das tabelas partilhadas uma vez cada). Os valores vêm de `t[k]`.
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
                    let root = key.split([PATH_SEP, TABLE_MARK]).next().unwrap_or(key);
                    keys.push(Value::String(lua.create_string(root)?));
                }
            }
            // Overlay de sessão ganha na colisão (1.ª ocorrência).
            let mut seen = std::collections::HashSet::new();
            keys.retain(|k| match k {
                Value::String(s) => seen.insert(s.to_string_lossy()),
                other => seen.insert(format!("{other:?}")),
            });
            iter_keys(lua, proxy, keys)
        })?,
    )?;

    proxy.set_metatable(Some(mt))?;
    Ok(proxy)
}

/// Separador de segmentos do caminho de uma folha de tabela partilhada.
const PATH_SEP: char = '\u{1F}';
/// Marcador "a tabela existe" (`caminho` + marca): uma tabela vazia partilhada.
const TABLE_MARK: char = '\u{1E}';

/// `true` para chaves internas das tabelas partilhadas (fora do save).
pub fn is_nested_key(key: &str) -> bool {
    key.contains(PATH_SEP) || key.contains(TABLE_MARK)
}

/// Segmento de caminho de uma chave Lua de tabela aninhada: `s<texto>` ou
/// `n<número>`. Outras chaves (tabelas, booleanos…) não são suportadas.
fn encode_segment(key: &Value) -> Option<String> {
    match key {
        Value::String(text) => Some(format!("s{}", text.to_string_lossy())),
        Value::Integer(i) => Some(format!("n{i}")),
        Value::Number(n) if n.is_finite() => {
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                Some(format!("n{}", *n as i64))
            } else {
                Some(format!("n{n}"))
            }
        }
        _ => None,
    }
}

fn decode_segment(lua: &Lua, segment: &str) -> mlua::Result<Value> {
    match segment.split_at_checked(1) {
        Some(("n", number)) => Ok(match number.parse::<i64>() {
            Ok(int) => Value::Integer(int),
            Err(_) => Value::Number(number.parse::<f64>().unwrap_or(0.0)),
        }),
        Some(("s", text)) => Ok(Value::String(lua.create_string(text)?)),
        _ => Ok(Value::String(lua.create_string(segment)?)),
    }
}

fn child_path(parent: &str, segment: &str) -> String {
    let mut out = String::with_capacity(parent.len() + segment.len() + 4);
    out.push_str(parent);
    out.push(PATH_SEP);
    out.push_str(segment);
    out
}

fn append_child_path(parent: &str, key: &Value) -> Option<String> {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(parent.len() + 24);
    out.push_str(parent);
    out.push(PATH_SEP);
    match key {
        Value::String(text) => {
            out.push('s');
            if let Ok(s) = text.to_str() {
                out.push_str(&s);
            } else {
                out.push_str(&text.to_string_lossy());
            }
        }
        Value::Integer(i) => {
            out.push('n');
            let _ = write!(out, "{i}");
        }
        Value::Number(n) if n.is_finite() => {
            out.push('n');
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                let _ = write!(out, "{}", *n as i64);
            } else {
                let _ = write!(out, "{n}");
            }
        }
        _ => return None,
    }
    Some(out)
}

/// O caminho é uma tabela partilhada (marcador ou alguma folha por baixo)?
fn is_table_at(map: &BTreeMap<String, GameValue>, path: &str) -> bool {
    let mut probe = String::with_capacity(path.len() + 4);
    probe.push_str(path);
    probe.push(TABLE_MARK);
    if map.contains_key(probe.as_str()) {
        return true;
    }
    probe.truncate(path.len());
    probe.push(PATH_SEP);
    map.range::<str, _>((
        std::ops::Bound::Included(probe.as_str()),
        std::ops::Bound::Unbounded,
    ))
    .next()
    .is_some_and(|(key, _)| key.starts_with(probe.as_str()))
}

/// Remove o valor no caminho e toda a subárvore. Devolve `true` se removeu
/// uma chave plana (`!is_nested_key`).
fn remove_subtree(map: &mut BTreeMap<String, GameValue>, path: &str) -> bool {
    let mut flat_removed = map.remove(path).is_some() && !is_nested_key(path);
    let mut probe = String::with_capacity(path.len() + 4);
    probe.push_str(path);
    probe.push(TABLE_MARK);
    map.remove(probe.as_str());
    probe.truncate(path.len());
    probe.push(PATH_SEP);
    let doomed: Vec<String> = map
        .range::<str, _>((
            std::ops::Bound::Included(probe.as_str()),
            std::ops::Bound::Unbounded,
        ))
        .take_while(|(key, _)| key.starts_with(probe.as_str()))
        .map(|(key, _)| key.clone())
        .collect();
    for key in doomed {
        if !is_nested_key(&key) {
            flat_removed = true;
        }
        map.remove(&key);
    }
    flat_removed
}

/// Caminho de um proxy aninhado (`None` = tabela comum).
fn proxy_path(table: &Table) -> Option<String> {
    table
        .metatable()
        .and_then(|mt| mt.raw_get::<Option<String>>("__game_path").ok().flatten())
}

/// Escreve `value` no caminho, substituindo o que lá estava. Tabelas comuns
/// são copiadas folha a folha (o mesmo `t` passa a viver no store e a ser
/// visto por todas as VMs); um proxy copia a subárvore do caminho dele.
fn write_path(store: &Arc<RwLock<GameStore>>, path: &str, value: &Value) -> mlua::Result<()> {
    // Via rápida escalar: a maioria das escritas (`playtime_secs`, `hp`,
    // `enemies[id].x`, `eco_pulled = true`) escreve ou limpa uma única folha
    // primitiva sem tabela. Evita alocar `Vec`s, `String`s ou tomar write-lock
    // quando o valor já é idêntico.
    match value {
        Value::Table(_) => {}
        other => {
            let gv = GameValue::from_lua(other);
            match gv {
                None | Some(GameValue::Null) => {
                    if let Ok(s) = store.read()
                        && !s.map.contains_key(path)
                        && !is_table_at(&s.map, path)
                    {
                        return Ok(());
                    }
                    if let Ok(mut s) = store.write()
                        && remove_subtree(&mut s.map, path)
                    {
                        s.bump_flat_version();
                    }
                    return Ok(());
                }
                Some(gv) => {
                    if let Ok(s) = store.read()
                        && s.map.get(path) == Some(&gv)
                    {
                        return Ok(());
                    }
                    if let Ok(mut s) = store.write() {
                        if let Some(slot) = s.map.get_mut(path) {
                            if *slot != gv {
                                *slot = gv;
                                if !is_nested_key(path) {
                                    s.bump_flat_version();
                                }
                            }
                            return Ok(());
                        }
                        if is_table_at(&s.map, path) {
                            remove_subtree(&mut s.map, path);
                        }
                        s.map.insert(path.to_string(), gv);
                        if !is_nested_key(path) {
                            s.bump_flat_version();
                        }
                    }
                    return Ok(());
                }
            }
        }
    }

    // Recolhe ANTES do lock: ler uma tabela Lua não toca no store, mas um
    // proxy aninhado como valor exige ler o store.
    let mut leaves: Vec<(String, GameValue)> = Vec::new();
    let mut tables: Vec<String> = Vec::new();
    let mut copy_from: Option<String> = None;
    if let Value::Table(table) = value {
        if let Some(source) = proxy_path(table) {
            if source == path {
                return Ok(());
            }
            copy_from = Some(source);
        } else {
            let mut visited = std::collections::HashSet::new();
            collect_table(table, path, &mut leaves, &mut tables, &mut visited)?;
        }
    }
    let Ok(mut s) = store.write() else {
        return Ok(());
    };
    let copied: Vec<(String, GameValue)> = match &copy_from {
        Some(source) => {
            let prefix_leaf = format!("{source}{PATH_SEP}");
            let mark = format!("{source}{TABLE_MARK}");
            let mut out = vec![(format!("{path}{TABLE_MARK}"), GameValue::Bool(true))];
            for (key, gv) in s.map.range(source.clone()..) {
                if key == &mark {
                    continue;
                }
                let Some(rest) = key.strip_prefix(&prefix_leaf) else {
                    if key.starts_with(source.as_str()) {
                        continue;
                    }
                    break;
                };
                out.push((format!("{path}{PATH_SEP}{rest}"), gv.clone()));
            }
            out
        }
        None => Vec::new(),
    };
    let mut flat_changed = remove_subtree(&mut s.map, path);
    for table_path in tables {
        s.map
            .insert(format!("{table_path}{TABLE_MARK}"), GameValue::Bool(true));
    }
    for (key, gv) in leaves.into_iter().chain(copied) {
        if !is_nested_key(&key) {
            flat_changed = true;
        }
        s.map.insert(key, gv);
    }
    if flat_changed {
        s.bump_flat_version();
    }
    Ok(())
}

/// Folhas primitivas e sub-tabelas de uma tabela Lua comum (recursivo;
/// ciclos e chaves não suportadas são saltados; funções ficam de fora).
fn collect_table(
    table: &Table,
    path: &str,
    leaves: &mut Vec<(String, GameValue)>,
    tables: &mut Vec<String>,
    visited: &mut std::collections::HashSet<usize>,
) -> mlua::Result<()> {
    if !visited.insert(table.to_pointer() as usize) {
        return Ok(());
    }
    tables.push(path.to_string());
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Some(segment) = encode_segment(&key) else {
            continue;
        };
        let child = child_path(path, &segment);
        match &value {
            Value::Table(inner) => match proxy_path(inner) {
                // Um proxy dentro de uma tabela nova: copia o valor actual
                // dele (raro; a escrita seguinte a esse caminho trata-o).
                Some(_) => {}
                None => collect_table(inner, &child, leaves, tables, visited)?,
            },
            other => {
                if let Some(gv) = GameValue::from_lua(other)
                    && gv != GameValue::Null
                {
                    leaves.push((child, gv));
                }
            }
        }
    }
    Ok(())
}

/// Lê o caminho: primitivo, proxy aninhado (tabela partilhada) ou nil.
fn read_path(lua: &Lua, store: &Arc<RwLock<GameStore>>, path: &str) -> mlua::Result<Value> {
    let (value, table) = {
        let Ok(s) = store.read() else {
            return Ok(Value::Nil);
        };
        let value = s.map.get(path).cloned();
        let table = value.is_none() && is_table_at(&s.map, path);
        (value, table)
    };
    match value {
        Some(GameValue::Null) => Ok(Value::Nil),
        Some(gv) => gv.into_lua(lua),
        None if table => Ok(Value::Table(nested_proxy(lua, store, path.to_owned())?)),
        None => Ok(Value::Nil),
    }
}

/// Filhos directos de um caminho (chaves decodificadas, por ordem do mapa).
fn child_keys(lua: &Lua, store: &Arc<RwLock<GameStore>>, path: &str) -> mlua::Result<Vec<Value>> {
    let segments: Vec<String> = {
        let Ok(s) = store.read() else {
            return Ok(Vec::new());
        };
        let prefix = format!("{path}{PATH_SEP}");
        let mut out: Vec<String> = Vec::new();
        for (key, _) in s.map.range(prefix.clone()..) {
            let Some(rest) = key.strip_prefix(&prefix) else {
                break;
            };
            let segment = rest.split([PATH_SEP, TABLE_MARK]).next().unwrap_or(rest);
            if out.last().map(String::as_str) != Some(segment) {
                out.push(segment.to_string());
            }
        }
        out.dedup();
        out
    };
    segments
        .iter()
        .map(|segment| decode_segment(lua, segment))
        .collect()
}

/// Iterador `(k, v)` sobre `keys`, com os valores lidos por `t[k]`.
fn iter_keys(lua: &Lua, proxy: Table, keys: Vec<Value>) -> mlua::Result<Function> {
    let mut next = 0usize;
    lua.create_function_mut(move |lua, ()| -> mlua::Result<MultiValue> {
        loop {
            let Some(key) = keys.get(next).cloned() else {
                return Ok(MultiValue::new());
            };
            next += 1;
            let value: Value = proxy.get(key.clone())?;
            // Removido entretanto (outra escrita a meio da iteração): salta.
            if value.is_nil() {
                continue;
            }
            return Ok(MultiValue::from_vec(vec![key.into_lua(lua)?, value]));
        }
    })
}

/// Proxy de uma tabela PARTILHADA do `viber.game()` (`game.enemies`,
/// `game.enemies[id]`…). No Viber todas as entidades corriam numa VM e estas
/// tabelas eram de todos; no BMS cada script tem a sua VM e a tabela ficava
/// na VM que a criou — o combate do herói não via o registo das criaturas
/// (nenhum golpe acertava), o alerta de aggro e a esquiva não atravessavam
/// (2026-11-15). As folhas vivem no store do processo com chaves
/// `raiz␟seg␟…`; ficam fora do save (como as tabelas de sessão de antes) e
/// o load (`replace`) limpa-as.
fn nested_proxy(lua: &Lua, store: &Arc<RwLock<GameStore>>, path: String) -> mlua::Result<Table> {
    let cache: Table = lua.named_registry_value("viber_game_nested")?;
    if let Ok(Some(hit)) = cache.raw_get::<Option<Table>>(path.as_str()) {
        return Ok(hit);
    }
    let proxy = lua.create_table()?;
    let mt = lua.create_table()?;
    mt.raw_set("__game_path", path.as_str())?;

    let index_store = store.clone();
    let index_path = path.clone();
    mt.raw_set(
        "__index",
        lua.create_function(
            move |lua, (_proxy, key): (Table, Value)| -> mlua::Result<Value> {
                let Some(child) = append_child_path(&index_path, &key) else {
                    return Ok(Value::Nil);
                };
                read_path(lua, &index_store, &child)
            },
        )?,
    )?;

    let newindex_store = store.clone();
    let newindex_path = path.clone();
    mt.raw_set(
        "__newindex",
        lua.create_function(
            move |_, (_proxy, key, value): (Table, Value, Value)| -> mlua::Result<()> {
                let Some(child) = append_child_path(&newindex_path, &key) else {
                    return Err(mlua::Error::runtime(
                        "viber.game(): chave de tabela partilhada tem de ser string ou número",
                    ));
                };
                write_path(&newindex_store, &child, &value)
            },
        )?,
    )?;

    let iter_store = store.clone();
    let iter_path = path.clone();
    mt.raw_set(
        "__iter",
        lua.create_function(move |lua, proxy: Table| -> mlua::Result<Function> {
            let keys = child_keys(lua, &iter_store, &iter_path)?;
            iter_keys(lua, proxy, keys)
        })?,
    )?;

    let len_store = store.clone();
    let len_path = path.clone();
    mt.raw_set(
        "__len",
        lua.create_function(move |_, _proxy: Table| -> mlua::Result<i64> {
            let Ok(s) = len_store.read() else {
                return Ok(0);
            };
            let mut n = 0i64;
            loop {
                let child = child_path(&len_path, &format!("n{}", n + 1));
                if s.map.contains_key(&child) || is_table_at(&s.map, &child) {
                    n += 1;
                } else {
                    return Ok(n);
                }
            }
        })?,
    )?;

    proxy.set_metatable(Some(mt))?;
    cache.raw_set(path.as_str(), proxy.clone())?;
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

/// Lock de teste para o store partilhado do processo: os testes que semeiam
/// `viber.game()` não podem correr em paralelo (o static é por-processo).
///
/// NÃO é `#[cfg(test)]`: os testes do JOGO (crate `vael`) compilam o crate
/// como dependência, onde o `cfg(test)` do `bevy_ext_luau` está DESLIGADO —
/// o lock tem de existir nessa compilação também.
#[doc(hidden)]
pub mod test_support {
    pub static PROCESS_STORE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    /// este lock (correr em paralelo apagava estado uns dos outros). O lock
    /// é o MESMO que os testes do jogo usam (`test_support`): dois statics
    /// distintos não exclusavam nada.
    use super::test_support::PROCESS_STORE_TEST_LOCK;

    fn fresh_store() -> Arc<RwLock<GameStore>> {
        Arc::new(RwLock::new(GameStore::default()))
    }

    #[test]
    fn tabelas_do_game_sao_partilhadas_entre_vms() {
        let shared = fresh_store();
        let vm = || {
            let lua = Lua::new();
            let viber = lua.create_table().expect("viber");
            lua.globals().set("viber", viber.clone()).expect("global");
            lua.globals().set("vael", viber.clone()).expect("global");
            install(&lua, &viber, shared.clone()).expect("instala");
            lua
        };
        let a = vm();
        let b = vm();
        a.load(
            "local g = viber.game()
             g.enemies = {}
             g.enemies[5] = { kind = 'slime', hp = 10, pos = { x = 1 } }
             g.hero_dodge = {}
             g.hero_dodge.until_t = 3.5
             g.respawns = {}
             g.respawns[#g.respawns + 1] = { proto = 'p1' }
             g.respawns[#g.respawns + 1] = { proto = 'p2' }",
        )
        .exec()
        .expect("escreve");
        let (hp, kind, x, dodge, n): (f64, String, f64, f64, i64) = b
            .load(
                "local g = viber.game()
                 return g.enemies[5].hp, g.enemies[5].kind, g.enemies[5].pos.x,
                        g.hero_dodge.until_t, #g.respawns",
            )
            .eval()
            .expect("lê noutra VM");
        assert_eq!(
            (hp, kind.as_str(), x, dodge, n),
            (10.0, "slime", 1.0, 3.5, 2)
        );
        b.load("local e = viber.game().enemies[5]; e.hp = 3")
            .exec()
            .expect("muta");
        let (hp, count, same): (f64, i64, bool) = a
            .load(
                "local g = viber.game()
                 local c = 0
                 for id, e in pairs(g.enemies) do c = c + 1 end
                 return g.enemies[5].hp, c, g.enemies[5] == g.enemies[5]",
            )
            .eval()
            .expect("vê a mutação");
        assert_eq!((hp, count, same), (3.0, 1, true));
        // Substituir por primitivo apaga a subárvore; nil remove.
        a.load("viber.game().enemies[5] = nil")
            .exec()
            .expect("remove");
        let gone: bool = b
            .load("return viber.game().enemies[5] == nil and type(viber.game().enemies) == 'table'")
            .eval()
            .expect("removido");
        assert!(gone, "a entrada sai, a tabela vazia continua");
        // Fora do save: só os primitivos de topo.
        a.load("viber.game().gold = 7").exec().expect("gold");
        let json = game_to_json_store(&shared);
        assert_eq!(
            json.keys().cloned().collect::<Vec<_>>(),
            vec!["gold".to_string()]
        );
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
        lua.globals().set("viber", viber.clone()).expect("global");
        lua.globals().set("vael", viber.clone()).expect("global");
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

    #[test]
    fn game_proxy_le_e_escreve_no_store() {
        let lua = Lua::new();
        let viber = lua.create_table().expect("viber");
        lua.globals().set("viber", viber.clone()).expect("global");
        lua.globals().set("vael", viber.clone()).expect("global");
        install(&lua, &viber, fresh_store()).expect("instala");
        let game = game_proxy(&lua).expect("proxy");
        // `set` atravessa o `__newindex`; `get` o `__index` — os raw_* liam
        // a tabela crua e não viam nada.
        game.set("hp", 7).expect("escrever hp");
        game.set("nome", "herói").expect("escrever nome");
        assert_eq!(game.get::<i64>("hp").expect("hp"), 7);
        assert_eq!(game.get::<String>("nome").expect("nome"), "herói");
        // `nil` remove a chave (o `__newindex` mapeia para `Null`).
        game.set("hp", mlua::Value::Nil).expect("remover hp");
        assert!(game.get::<mlua::Value>("hp").expect("hp").is_nil());
        // O MESMO proxy em cada chamada (identidade, não snapshot).
        assert_eq!(
            game_proxy(&lua).expect("proxy").to_pointer(),
            game.to_pointer()
        );
    }

    #[test]
    fn state_instalado_pelo_host_nao_e_pisado() {
        let lua = Lua::new();
        let viber = lua.create_table().expect("viber");
        // O host do monólito instala `state` (per-entidade) ANTES do
        // `game::install`; o per-VM tem de o deixar em paz.
        let per_entity = lua.create_table().expect("estado");
        let per_entity_fn = per_entity.clone();
        viber
            .set(
                "state",
                lua.create_function(move |_, ()| Ok(per_entity_fn.clone()))
                    .expect("fn state"),
            )
            .expect("viber.state");
        lua.globals().set("viber", viber.clone()).expect("global");
        lua.globals().set("vael", viber.clone()).expect("global");
        install(&lua, &viber, fresh_store()).expect("instala");
        let called: mlua::Table = lua.load("return viber.state()").eval().expect("state");
        assert_eq!(called.to_pointer(), per_entity.to_pointer());
    }

    #[test]
    fn apply_patch_null_remove_a_chave() {
        let _lock = PROCESS_STORE_TEST_LOCK.lock().expect("lock");
        let lua = Lua::new();
        restore_game_json(&BTreeMap::new());
        let mut patch = BTreeMap::new();
        patch.insert("gate".to_string(), json!(true));
        apply_patch(&lua, &patch);
        assert_eq!(game_snapshot().get("gate"), Some(&GameValue::Bool(true)));
        patch.insert("gate".to_string(), serde_json::Value::Null);
        apply_patch(&lua, &patch);
        assert!(
            !game_snapshot().contains_key("gate"),
            "`null` no patch tem de remover a chave"
        );
    }

    #[test]
    fn game_snapshot_vm_le_o_proxy_e_ignora_tabelas() {
        let lua = Lua::new();
        let viber = lua.create_table().expect("viber");
        lua.globals().set("viber", viber.clone()).expect("global");
        lua.globals().set("vael", viber.clone()).expect("global");
        install(&lua, &viber, fresh_store()).expect("instala");
        lua.load(
            r#"local g = viber.game()
               g.hp = 7
               g.nome = "herói"
               g.ligado = true
               g.sumido = nil
               g.sessao = { nao = "persiste" }"#,
        )
        .exec()
        .expect("semear");
        let snapshot = game_snapshot_vm(&lua);
        assert_eq!(snapshot.get("hp"), Some(&serde_json::json!(7)));
        assert_eq!(snapshot.get("nome"), Some(&serde_json::json!("herói")));
        assert_eq!(snapshot.get("ligado"), Some(&serde_json::json!(true)));
        assert!(
            !snapshot.contains_key("sessao"),
            "tabela de sessão não entra"
        );
        assert!(!snapshot.contains_key("sumido"));
    }
}
