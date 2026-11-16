//! Props exportadas por instância: `viber.prop(nome)` / `viber.props()` —
//! portado do monólito (exit4/F4).
//!
//! O script declara o esquema com `--@export` ([`crate::exports`]); cada
//! entidade guarda os seus valores em `prop-<nome>` (dados autorais
//! publicados pela integração no [`crate::ScriptHostState::props`]). O valor
//! devolvido é o da instância, senão o default da anotação, senão `nil`.
//!
//! DIVERGÊNCIA documentada do monólito: no Viber o top-level corria UMA vez
//! por FICHEIRO (partilhado por todas as instâncias) e ler props aí era
//! ERRO. No BMS cada contexto é por `(entidade, script)` — o top-level corre
//! POR INSTÂNCIA, logo ler props no top-level é válido aqui (o guarda
//! `top_level` não tem aí razão de ser).

use std::sync::{Arc, RwLock};

use mlua::{Lua, Table};

use crate::events::ScriptPath;
use crate::exports::{ScriptExports, parse_exports};
use crate::ScriptGlobals;

pub fn install(lua: &Lua, api: &Table, shared: Arc<RwLock<ScriptGlobals>>) -> mlua::Result<()> {
    // viber.prop(nome) -> valor tipado | nil
    let prop_shared = shared.clone();
    api.set(
        "prop",
        lua.create_function(move |lua, name: String| {
            let (exports, props, path) = context(&prop_shared, lua, "viber.prop")?;
            if exports.get(&name).is_none() {
                return Err(mlua::Error::runtime(format!(
                    "viber.prop: '{name}' não é exportado por {path} (declare `--@export {name}: <tipo> = <default>`)"
                )));
            }
            // Resolve SÓ o prop pedido: o `resolve` inteiro materializava
            // todas as defs (e um Vec) em cada leitura — o getter corre por
            // frame por entidade.
            let Some(prop) = exports.resolve_one(&props, &name) else {
                return Ok(mlua::Value::Nil);
            };
            warn_invalid(&path, &prop);
            match prop.value {
                Some(value) => value.to_lua(lua),
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )?;

    // viber.props() -> { nome = valor, … } (todos os exports com valor)
    let props_shared = shared;
    api.set(
        "props",
        lua.create_function(move |lua, ()| {
            let (exports, props, path) = context(&props_shared, lua, "viber.props")?;
            let table = lua.create_table()?;
            for prop in exports.resolve(&props) {
                warn_invalid(&path, &prop);
                if let Some(value) = &prop.value {
                    table.set(prop.def.name.as_str(), value.to_lua(lua)?)?;
                }
            }
            Ok(table)
        })?,
    )?;
    Ok(())
}

type PropContext = (Arc<ScriptExports>, Arc<Vec<(String, String)>>, String);

/// O esquema (`--@export` do PRÓPRIO script, do mapa publicado por path) e
/// os valores da instância (do mapa publicado por bits).
fn context(
    shared: &Arc<RwLock<ScriptGlobals>>,
    lua: &Lua,
    func: &str,
) -> mlua::Result<PropContext> {
    let path = lua
        .app_data_ref::<ScriptPath>()
        .map(|p| p.0.clone())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| mlua::Error::runtime(format!("{func}: fora de um script")))?;
    let read = shared.read().ok();
    let exports = read
        .as_ref()
        .and_then(|g| g.exports.get(&path).cloned())
        .unwrap_or_else(|| exports_from_disk(lua, &path));
    let props = lua
        .app_data_ref::<crate::ScriptEntity>()
        .and_then(|e| e.0)
        .and_then(|entity| {
            read.as_ref()
                .and_then(|g| g.props.get(&(entity.to_bits() as i64)).cloned())
        })
        .unwrap_or_default();
    Ok((exports, props, path))
}

/// Esquema lido do PRÓPRIO ficheiro quando a integração ainda não o
/// publicou. O top-level de um contexto novo corre no frame em que o asset
/// carrega — antes do `parse_script_exports`/`publish_globals` do frame
/// seguinte — e um `viber.prop` aí dava "não é exportado" para um export
/// declarado (baú, sementes, diálogos; visto 2026-11-15). Cache por path.
fn exports_from_disk(lua: &Lua, path: &str) -> Arc<ScriptExports> {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    static CACHE: std::sync::LazyLock<RwLock<HashMap<String, Arc<ScriptExports>>>> =
        std::sync::LazyLock::new(Default::default);
    if let Some(hit) = CACHE.read().ok().and_then(|c| c.get(path).cloned()) {
        return hit;
    }
    let rel = Path::new(path);
    let stripped = rel.strip_prefix("scripts").unwrap_or(rel);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = crate::game::read_scripts_dir(lua) {
        candidates.push(dir.join(stripped));
        candidates.push(dir.join(rel));
    }
    candidates.push(rel.to_path_buf());
    let source = candidates
        .iter()
        .find_map(|candidate| std::fs::read_to_string(candidate).ok());
    let Some(source) = source else {
        // Sem ficheiro (testes, VM sem pasta): esquema vazio, sem cache.
        return Arc::new(parse_exports(""));
    };
    let parsed = Arc::new(parse_exports(&source));
    if let Ok(mut cache) = CACHE.write() {
        cache.insert(path.to_string(), parsed.clone());
    }
    parsed
}

/// Um `prop-*` inválido cai para o default — avisa 1× por script+prop+valor.
fn warn_invalid(path: &str, prop: &crate::exports::ResolvedProp<'_>) {
    let Some(error) = &prop.error else {
        return;
    };
    // Sem ctx mutável por chamada (o BMS não semeia um ring de avisos por
    // script): o warn repete por frame — o log crate dedup não; aceitável
    // porque um prop inválido é um erro de AUTORIA, corrigido no load.
    log::warn!(
        "[luau] {path}: prop-{} inválido ({error}) — a usar o default",
        prop.def.name
    );
}
