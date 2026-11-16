//! REPL de debug (`viber.lua` do bridge) — uma instância Lua PERSISTENTE,
//! dedicada, com a MESMA tabela `viber` dos contextos de jogo.
//!
//! Era a VM partilhada do host antigo (`LuaScriptHost`) que o bridge
//! reutilizava; com o host aposentado, o bridge avalia contra ESTA instância:
//! - `install_viber_api` (a tabela `viber` completa sobre os globals/store
//!   ESTÁTICOS deste crate — `viber.state()` na REPL lê o estado real do
//!   jogo, publicado a cada frame);
//! - `viber.ui` quando a feature existe e o snapshot da UI saiu;
//! - os instaladores registados (`add_context_installer`): `viber.profiler`
//!   da engine e `viber.vael` do jogo (cofre/XP/quests/qa_snapshot).
//!
//! A instância vive num **recurso** ([`Repl`], criado à pressa pelo bridge na
//! primeira chamada): persiste entre chamadas no processo, e cada app de
//! teste tem a SUA — sem contaminação entre testes no mesmo binário.
//!
//! As globals sobrevivem entre chamadas (env própria com `__index` →
//! globals), isolada dos contextos de script: `viber.events()` da REPL
//! subscreve a fila `""` dela — não drena filas de scripts de jogo.
//!
//! Sem guard de instruções (Luau/mlua não expõe hooks): um `while true do
//! end` na REPL congela o frame — igual ao que um script de página faz ao
//! Chrome. Ferramenta de debug, risco aceite (o mesmo contrato do bridge).

use std::sync::Arc;

use bevy::prelude::*;
use mlua::{Lua, Value};

/// A instância Lua persistente da REPL (`viber.lua` do bridge). A tabela
/// `viber` e os instaladores registados instalam-se UMA vez, no `new` —
/// plugins que registam instaladores DEPOIS do primeiro `viber.lua` ficam de
/// fora desta instância (o bridge só é servido com o jogo a correr, já
/// depois de todos os builds; QA: uma instância do jogo de cada vez).
#[derive(Resource, Clone)]
pub struct Repl {
    lua: Arc<Lua>,
}

impl Default for Repl {
    fn default() -> Self {
        Self::new()
    }
}

impl Repl {
    /// Cria a Lua da REPL e instala tudo o que um contexto de jogo tem.
    pub fn new() -> Self {
        let lua = Lua::new();
        if let Err(error) = install(&lua) {
            log::warn!("[lua] REPL sem a API `viber` completa: {error}");
        }
        Self { lua: Arc::new(lua) }
    }

    /// A Lua partilhada (os app data `DebugView`/`DebugOps` do bridge vivem
    /// aqui; as globals da REPL também).
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Clone do `Arc` interno (o bridge guarda-o pelo bloco do eval).
    pub fn shared(&self) -> Arc<Lua> {
        self.lua.clone()
    }
}

/// Instala tudo o que um contexto de jogo tem, sobre uma `Lua` nua.
fn install(lua: &Lua) -> mlua::Result<()> {
    // A mesma tabela `viber` dos contextos BMS (globals/store estáticos
    // partilhados com o jogo a correr).
    crate::install_viber_api(lua)?;
    // Pasta de scripts desta VM (o `viber.load`/`decision.load` da REPL lê-a
    // do app data; o static do processo é a fonte publicada pelo integrador).
    if let Some(dir) = crate::game::process_scripts_dir() {
        lua.set_app_data(crate::game::ScriptsDir(Some(dir)));
    }
    // F2: `viber.ui` — o snapshot do `UiScriptState` (o jogo publica no
    // Startup; a REPL pode nascer antes, nesse caso fica sem `viber.ui`).
    #[cfg(feature = "ui")]
    if let Some(state) = crate::UI_SCRIPT_STATE.get() {
        bevy_ext_ui::ui::script::install_ui_api(lua, state)?;
    }
    // Instaladores registados de fora (profiler da engine, `viber.vael` do
    // jogo): um que falha não mata a REPL — os restantes ficam. A REPL não
    // tem attach (nenhum instalador aqui lê entidade/path de script).
    for installer in crate::extra_installers() {
        if let Err(error) = installer(lua) {
            log::warn!("[lua] REPL: falha a instalar API extra: {error}");
        }
    }
    Ok(())
}

/// Devolve a REPL da app (o mundo que serve o bridge), criando-a à pressa na
/// primeira chamada. O clone do `Arc` permite o bloco de avaliação segurar a
/// Lua sem manter o `&mut World` emprestado.
pub fn repl_or_init(world: &mut World) -> Arc<Lua> {
    if world.get_resource::<Repl>().is_none() {
        world.insert_resource(Repl::new());
    }
    world.resource::<Repl>().shared()
}

/// Avalia `code` numa REPL persistente: globals sobrevivem entre chamadas
/// (env própria com `__index` → globals; escritas novas ficam na env), e
/// `return expr` devolve o valor. Erro de compilação/execução → `Err`.
///
/// A instância é do CHAMADOR (o bridge passa a dela, criada por
/// [`repl_or_init`]); a env persistente vive no registry `"viber_repl_env"`
/// dessa Lua.
pub fn repl_eval(lua: &Lua, code: &str) -> Result<Value, String> {
    let env: mlua::Table = match lua.named_registry_value("viber_repl_env") {
        Ok(env) => env,
        Err(_) => {
            let env = lua
                .create_table()
                .and_then(|env| {
                    let mt = lua.create_table()?;
                    mt.set("__index", lua.globals())?;
                    env.set_metatable(Some(mt))?;
                    Ok(env)
                })
                .map_err(|e| format!("falha a criar env da REPL: {e}"))?;
            let _ = lua.set_named_registry_value("viber_repl_env", env.clone());
            env
        }
    };
    lua.load(code)
        .set_name("=repl")
        .set_environment(env)
        .into_function()
        .and_then(|chunk| chunk.call::<Value>(()))
        .map_err(|e| e.to_string())
}
