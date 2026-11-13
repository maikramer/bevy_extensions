//! Host de scripting do Vael sobre `bevy_mod_scripting` (Luau).
//!
//! O BMS substitui o host Luau caseiro do Viber: gere contextos, assets e
//! callbacks. Este crate instala a tabela `viber.*` em cada contexto através
//! de um `ContextInitializer` e mantém um estado de frame num `RwLock`
//! estático (as funções Lua não recebem `World` directamente).
//!
//! A API é migrada por slices: `time`, `player_position`, `scene_name`,
//! `log` e `toast` são o primeiro. As restantes funções entram aqui e o
//! adaptador do Vael passa a consumir este host em vez de `viber::luau`.

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock, RwLock};

use bevy::prelude::*;
use bevy_ext_core::ScriptToast;
use bevy_mod_scripting::bindings::InteropError;
use bevy_mod_scripting::core::BMSScriptingInfrastructurePlugin;
use bevy_mod_scripting::lua::LuaContext;
use bevy_mod_scripting::prelude::*;
use mlua::{IntoLua, Lua, MultiValue};

/// Estado do frame visível para os scripts. O jogo preenche-o; as funções
/// Lua leem-no sem acesso directo ao ECS.
#[derive(Resource, Debug, Default, Clone)]
pub struct ScriptHostState {
    pub time: f64,
    pub player: Option<Vec3>,
    pub scene: Option<String>,
}

#[derive(Debug, Default)]
struct ScriptGlobals {
    time: f64,
    player: Option<Vec3>,
    scene: Option<String>,
    toasts: VecDeque<String>,
}

static SCRIPT_GLOBALS: OnceLock<Arc<RwLock<ScriptGlobals>>> = OnceLock::new();

fn globals() -> Arc<RwLock<ScriptGlobals>> {
    SCRIPT_GLOBALS
        .get_or_init(|| Arc::new(RwLock::new(ScriptGlobals::default())))
        .clone()
}

/// Host do Vael. Requer `AssetPlugin` (o BMS carrega scripts como assets).
pub struct VaelScriptingPlugin;

impl Plugin for VaelScriptingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScriptHostState>()
            // O BMS precisa da infraestrutura de contextos; o plugin Lua é
            // configurado com o inicializador da tabela `viber`.
            .add_plugins(BMSScriptingInfrastructurePlugin::default())
            .add_plugins(
                LuaScriptingPlugin::default().add_context_initializer(init_context),
            )
            .add_systems(PreUpdate, publish_globals)
            .add_systems(Update, drain_toasts);
    }
}

fn init_context(
    _attachment: &ScriptAttachment,
    context: &mut LuaContext,
) -> Result<(), InteropError> {
    install_viber_api_with(context, globals()).map_err(InteropError::external)
}

fn publish_globals(state: Res<ScriptHostState>) {
    if let Ok(mut globals) = globals().write() {
        globals.time = state.time;
        globals.player = state.player;
        globals.scene.clone_from(&state.scene);
    }
}

fn drain_toasts(mut toasts: Option<MessageWriter<ScriptToast>>) {
    let Some(writer) = toasts.as_mut() else {
        return;
    };
    let shared = globals();
    let Ok(mut globals) = shared.write() else {
        return;
    };
    while let Some(text) = globals.toasts.pop_front() {
        writer.write(ScriptToast(text));
    }
}

/// Instala a tabela `viber` num contexto Lua. Público para o adaptador do
/// Vael reutilizar a mesma API quando precisar de um contexto de REPL.
pub fn install_viber_api(lua: &Lua) -> mlua::Result<()> {
    install_viber_api_with(lua, globals())
}

fn install_viber_api_with(
    lua: &Lua,
    globals: Arc<RwLock<ScriptGlobals>>,
) -> mlua::Result<()> {
    let viber = lua.create_table()?;

    let time_globals = globals.clone();
    viber.set(
        "time",
        lua.create_function(move |_, ()| {
            Ok(time_globals
                .read()
                .map(|globals| globals.time)
                .unwrap_or(0.0))
        })?,
    )?;

    let pos_globals = globals.clone();
    viber.set(
        "player_position",
        lua.create_function(move |lua, ()| {
            let player = pos_globals.read().ok().and_then(|globals| globals.player);
            match player {
                Some(p) => Ok(MultiValue::from_vec(vec![
                    p.x.into_lua(lua)?,
                    p.y.into_lua(lua)?,
                    p.z.into_lua(lua)?,
                ])),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    let scene_globals = globals.clone();
    viber.set(
        "scene_name",
        lua.create_function(move |_, ()| {
            Ok(scene_globals
                .read()
                .ok()
                .and_then(|globals| globals.scene.clone()))
        })?,
    )?;

    viber.set(
        "log",
        lua.create_function(|_, message: String| {
            log::info!("[lua] {message}");
            Ok(())
        })?,
    )?;

    let toast_globals = globals;
    viber.set(
        "toast",
        lua.create_function(move |_, message: String| {
            if let Ok(mut globals) = toast_globals.write() {
                globals.toasts.push_back(message);
            }
            Ok(())
        })?,
    )?;

    lua.globals().set("viber", viber)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viber_table_le_o_estado_do_frame() {
        let lua = Lua::new();
        let globals = Arc::new(RwLock::new(ScriptGlobals {
            time: 12.5,
            player: Some(Vec3::new(1.0, 2.0, 3.0)),
            scene: Some("main".into()),
            toasts: VecDeque::new(),
        }));
        install_viber_api_with(&lua, globals.clone()).expect("instala viber");

        let time: f64 = lua.load("return viber.time()").eval().expect("time");
        assert_eq!(time, 12.5);

        let position: (f32, f32, f32) = lua
            .load("return viber.player_position()")
            .eval()
            .expect("position");
        assert_eq!(position, (1.0, 2.0, 3.0));

        let scene: String = lua
            .load("return viber.scene_name()")
            .eval()
            .expect("scene");
        assert_eq!(scene, "main");

        lua.load("viber.toast('olá')").exec().expect("toast");
        let globals = globals.read().expect("globals");
        assert_eq!(globals.toasts.front().map(String::as_str), Some("olá"));
    }

    #[test]
    fn player_position_e_nil_sem_heroi() {
        let lua = Lua::new();
        install_viber_api_with(&lua, globals()).expect("instala viber");
        let missing: bool = lua
            .load("return viber.player_position() == nil")
            .eval()
            .expect("position");
        assert!(missing);
    }
}
