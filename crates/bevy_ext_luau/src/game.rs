//! Estado de JOGO partilhado (`viber.game()`) e estado por contexto
//! (`viber.state()`).
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

use std::collections::{BTreeMap, VecDeque};
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
}

static GAME_STORE: OnceLock<Arc<RwLock<GameStore>>> = OnceLock::new();

/// O store real do processo (integrador e REPL); os testes injetam o seu.
pub(crate) fn game_store() -> Arc<RwLock<GameStore>> {
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

/// Instala `viber.game()` e `viber.state()` na tabela `viber`.
pub(crate) fn install(lua: &Lua, viber: &Table, store: Arc<RwLock<GameStore>>) -> mlua::Result<()> {
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
    Ok(())
}

/// O proxy: tabela real (cache) + metamétodos que atravessam para o mapa.
fn build_game_proxy(lua: &Lua, store: Arc<RwLock<GameStore>>) -> mlua::Result<Table> {
    let proxy = lua.create_table()?;
    let mt = lua.create_table()?;

    // __index — cache-miss no proxy: lê do mapa partilhado.
    let index_store = store.clone();
    mt.set(
        "__index",
        // O 1.º argumento do `__index` é a própria tabela — consome-a antes
        // da chave.
        lua.create_function(move |lua, (_proxy, key): (Table, Value)| -> mlua::Result<Value> {
            let Value::String(key) = &key else {
                return Ok(Value::Nil);
            };
            let name = key.to_string_lossy();
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
    // funções ficam só nesta VM (overlay de sessão, invisível ao save).
    let newindex_store = store.clone();
    mt.set(
        "__newindex",
        lua.create_function(
            move |_, (proxy, key, value): (Table, Value, Value)| -> mlua::Result<()> {
                let Value::String(key) = &key else {
                    return Err(mlua::Error::runtime(
                        "viber.game(): as chaves têm de ser strings",
                    ));
                };
                let name = key.to_string_lossy();
                match GameValue::from_lua(&value) {
                    // Primitivo: só o mapa partilhado. NADA fica em cru no
                    // proxy — um valor cru bloqueava a atribuição seguinte
                    // (`game.k = nil` não disparava __newindex) e a remoção
                    // nunca chegava ao mapa.
                    Some(gv) => {
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
                    None => proxy.raw_set(name, value)?,
                }
                Ok(())
            },
        )?,
    )?;

    // __iter — iteração genérica (`for k, v in viber.game() do` e o `pairs`
    // envolvido): funde o overlay de sessão (cru) com as chaves do mapa
    // partilhado. Os valores vêm de `t[k]` (via __index → mapa).
    mt.set(
        "__iter",
        {
            let iter_store = store;
            lua.create_function(move |lua, proxy: Table| -> mlua::Result<Function> {
                // `Table::pairs` do mlua 0.12 já é cru (lua_rawiter): vê só
                // o overlay de sessão do proxy.
                let mut keys: Vec<Value> = proxy
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
            })?
        },
    )?;

    proxy.set_metatable(Some(mt))?;
    Ok(proxy)
}
