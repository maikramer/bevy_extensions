//! Procura por nome e vitais GENÉRICAS de entidade — portado do monólito
//! (exit4/F4; a escrita de vitais `entity_damage`/`entity_heal`/
//! `entity_set_max_hp` e `cinematic` seguem na F6, com os seus comandos).
//!
//! Leitura por snapshot publicado ([`crate::ScriptHostState`], sem head de
//! frame); escrita por comando (aplicada pós-frame — o HP chega às leituras
//! no frame seguinte, igual ao resto da API).

use std::sync::{Arc, RwLock};

use mlua::{Lua, Table, Value};

use crate::ScriptGlobals;
use mlua::IntoLua;

use crate::commands::entity_from_id;
use crate::ScriptEntity;

/// Bits da entidade alvo: `id` explícito OU a entidade corrente.
pub fn resolve_entity(lua: &Lua, id: Option<i64>) -> mlua::Result<bevy::prelude::Entity> {
    match id {
        Some(bits) => entity_from_id(bits),
        None => lua
            .app_data_ref::<ScriptEntity>()
            .and_then(|e| e.0)
            .ok_or_else(|| mlua::Error::runtime("viber.entity_* fora de on_update (sem id)")),
    }
}

/// Chave exata primeiro; senão a primeira que contém. A escolha por substring
/// é a PRIMEIRA em ordem lexicográfica — a iteração do `HashMap` é arbitrária
/// (e muda quando a tabela rehasha, com entidades a nascer/morrer), pelo que
/// `viber.find("door")` devolvia portas diferentes entre frames. É a mesma
/// ordem estável que o `find_all` já promete. `min()` (e não um Vec ordenado):
/// mesmo resultado, sem materializar todas as chaves casadas por chamada.
fn exact_or_substring<'a>(
    map: &'a std::collections::HashMap<String, Vec<i64>>,
    name: &str,
) -> Option<&'a Vec<i64>> {
    if let Some(v) = map.get(name) {
        return Some(v);
    }
    let best = map.keys().filter(|key| key.contains(name)).min()?;
    map.get(best)
}

/// Instala o grupo `entity` (lado de leitura da F4) na tabela `viber`.
pub fn install(lua: &Lua, api: &Table, shared: Arc<RwLock<ScriptGlobals>>) -> mlua::Result<()> {
    // viber.entity_hp(id?) -> ok, cur, max — snapshot do frame. Sem id = self.
    let hp_shared = shared.clone();
    api.set(
        "entity_hp",
        lua.create_function(move |lua, id: Option<i64>| {
            let bits = match id {
                Some(bits) => bits,
                None => match lua.app_data_ref::<ScriptEntity>().and_then(|e| e.0) {
                    Some(e) => e.to_bits() as i64,
                    None => return Ok((false, 0.0, 0.0)),
                },
            };
            match hp_shared.read().ok().and_then(|g| g.hp_snapshot.get(&bits).copied()) {
                Some((cur, max)) => Ok((true, cur, max)),
                None => Ok((false, 0.0, 0.0)),
            }
        })?,
    )?;

    // viber.entity_name(id?) -> string | nil — o NOME de mundo da entidade
    // (o mesmo que `viber.find` procura; spawns de spawner sem `<Entity
    // name>` não têm). Sem id = a própria entidade (do mapa publicado).
    let name_shared = shared.clone();
    api.set(
        "entity_name",
        lua.create_function(move |lua, id: Option<i64>| {
            let read = name_shared.read().ok();
            match id {
                None => Ok(lua
                    .app_data_ref::<ScriptEntity>()
                    .and_then(|e| e.0)
                    .and_then(|entity| {
                        read.as_ref()
                            .and_then(|g| g.names.get(&(entity.to_bits() as i64)).cloned())
                    })),
                Some(bits) => Ok(read.and_then(|g| g.names.get(&bits).cloned())),
            }
        })?,
    )?;

    // viber.set_scale(s[, id]) — escala uniforme (0.05..20).
    let scale_shared = shared.clone();
    api.set(
        "set_scale",
        lua.create_function(move |lua, (scale, id): (f32, Option<i64>)| {
            if !scale.is_finite() || !(0.05..=20.0).contains(&scale) {
                return Err(mlua::Error::runtime(
                    "viber.set_scale: escala tem de estar em 0.05..20",
                ));
            }
            let entity = resolve_entity(lua, id)?;
            let _ = &scale_shared; // (os próximos comandos de entidade usam o
            // mesmo padrão; aqui o comando não lê o snapshot)
            crate::push_command(
                &scale_shared,
                crate::commands::ScriptCommand::SetScale { entity, scale },
            );
            Ok(())
        })?,
    )?;

    // viber.set_scale(s[, id]) — escala uniforme (0.05..20).
    let scale_shared = shared.clone();
    api.set(
        "set_scale",
        lua.create_function(move |lua, (scale, id): (f32, Option<i64>)| {
            if !scale.is_finite() || !(0.05..=20.0).contains(&scale) {
                return Err(mlua::Error::runtime(
                    "viber.set_scale: escala tem de estar em 0.05..20",
                ));
            }
            let entity = resolve_entity(lua, id)?;
            crate::push_command(
                &scale_shared,
                crate::commands::ScriptCommand::SetScale { entity, scale },
            );
            Ok(())
        })?,
    )?;

    // viber.self_id() -> bits da entidade que corre o script | nil
    api.set(
        "self_id",
        lua.create_function(move |lua, ()| {
            Ok(lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .map(|e| e.to_bits() as i64))
        })?,
    )?;

    // viber.find(nome) -> bits | nil — exato primeiro, substring depois
    // (mesma semântica do `viber.debug.find`).
    let find_shared = shared.clone();
    api.set(
        "find",
        lua.create_function(move |lua, name: String| {
            // Nome vazio não identifica ninguém: `contains("")` casava TODAS
            // as entidades e devolvia a primeira por ordem alfabética — um
            // prop vazio (`viber.find(viber.prop("home"))`) danificava ou
            // teleportava o alvo errado em silêncio.
            if name.is_empty() {
                return Ok(Value::Nil);
            }
            let Ok(g) = find_shared.read() else {
                return Ok(Value::Nil);
            };
            if let Some(bits) = exact_or_substring(&g.named_entities, &name)
                && let Some(first) = bits.first()
            {
                // i64 via IntoLua (Luau não tem inteiros — o Value::Integer
                // do mlua é i32 e os bits podem excedê-lo).
                return first.into_lua(&lua);
            }
            Ok(Value::Nil)
        })?,
    )?;

    // viber.find_all(nome) -> {bits...} — TODAS as correspondências, ordem
    // ESTÁVEL: nomes por ordem lexicográfica e, dentro de cada nome, os bits
    // por ordem (o snapshot já entrega vetores ordenados). Um script que
    // cacheia registos no arranque não pode ver ordens diferentes entre
    // frames/máquinas.
    let findall_shared = shared.clone();
    api.set(
        "find_all",
        lua.create_function(move |lua, name: String| {
            let out = lua.create_table()?;
            // Nome vazio não é um filtro: devolver o mundo INTEIRO (dezenas
            // de milhares de entidades no Vael) numa tabela Lua era um
            // alloc/lag por chamada — e nada no contrato promete "todos".
            if name.is_empty() {
                return Ok(out);
            }
            let Ok(g) = findall_shared.read() else {
                return Ok(out);
            };
            let mut keys: Vec<&String> = g
                .named_entities
                .keys()
                .filter(|key| *key == &name || key.contains(&name))
                .collect();
            keys.sort();
            let mut i = 1u32;
            for key in keys {
                for bits in &g.named_entities[key.as_str()] {
                    out.raw_set(i, *bits)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;
    Ok(())
}
