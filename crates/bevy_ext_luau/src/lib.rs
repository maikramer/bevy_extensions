//! Host de scripting do Vael sobre `bevy_mod_scripting` (Luau).
//!
//! O BMS substitui o host Luau caseiro do Viber: gere contextos, assets e
//! callbacks. Este crate instala a tabela `viber.*` em cada contexto através
//! de um `ContextInitializer` e mantém um estado de frame num `RwLock`
//! estático (as funções Lua não recebem `World` directamente).
//!
//! A API é migrada por slices a partir do `viber::luau` do Viber (referência
//! em `viber_ref/src/luau/`): estado de frame (`time`, `player_position`,
//! `scene_name`), estado de jogo partilhado (`game.rs`), input, comandos
//! enfileirados e loader de módulos. As funções com consumidor final na
//! engine (movimento, IA, vitais, `viber.vael.*`, NLU/decisão) ficam para a
//! integração do adaptador do Vael.

pub mod game;
pub mod keys;

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};

use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use bevy_ext_core::ScriptToast;
use bevy_mod_scripting::bindings::InteropError;
use bevy_mod_scripting::core::BMSScriptingInfrastructurePlugin;
use bevy_mod_scripting::lua::LuaContext;
use bevy_mod_scripting::prelude::*;
use mlua::{IntoLua, Lua, MultiValue};

pub use game::{GameValue, apply_game_patches, game_snapshot, publish_game_patch};

/// Entidade do attach corrente (scripts `EntityScript`); `None` em scripts
/// estáticos. As funções de percepção usam-na para se excluírem a si.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScriptEntity(pub Option<Entity>);

/// Estado do frame visível para os scripts. O jogo preenche-o; as funções
/// Lua leem-no sem acesso directo ao ECS.
#[derive(Resource, Debug, Default, Clone)]
pub struct ScriptHostState {
    pub time: f64,
    pub player: Option<Vec3>,
    pub scene: Option<String>,
    /// Entrada `<SceneEntry>` por onde se chegou à cena (`viber.scene_entry`).
    pub scene_entry: Option<String>,
    /// Id do `BiomeRegion` atual; `None` fora de biomas/interiores
    /// (`viber.biome`).
    pub biome: Option<String>,
}

/// Snapshot de input do frame (`viber.input.*`): teclado e rato nos três
/// estados. Publicado por [`publish_input`] a partir dos `ButtonInput`.
#[derive(Debug, Default, Clone)]
pub struct InputSnapshot {
    pub keys_pressed: HashSet<KeyCode>,
    pub keys_down: HashSet<KeyCode>,
    pub keys_released: HashSet<KeyCode>,
    pub mouse_pressed: HashSet<MouseButton>,
    pub mouse_down: HashSet<MouseButton>,
    pub mouse_released: HashSet<MouseButton>,
}

#[derive(Debug, Default)]
struct ScriptGlobals {
    time: f64,
    player: Option<Vec3>,
    scene: Option<String>,
    scene_entry: Option<String>,
    biome: Option<String>,
    menu_open: bool,
    toasts: VecDeque<String>,
    input: InputSnapshot,
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
            .add_systems(
                PreUpdate,
                (publish_globals, publish_input, game::apply_game_patches),
            )
            .add_systems(Update, drain_toasts);
    }
}

fn init_context(
    attachment: &ScriptAttachment,
    context: &mut LuaContext,
) -> Result<(), InteropError> {
    // Entidade do attach: `nearby` exclui-se a si e `play_clip`/`gesture`
    // sem `id` caem na própria. Scripts estáticos ficam sem entidade.
    let entity = match attachment {
        ScriptAttachment::EntityScript(entity, _) => Some(*entity),
        ScriptAttachment::StaticScript(_) => None,
    };
    context.set_app_data(ScriptEntity(entity));
    install_viber_api_with(context, globals(), game::game_store())
        .map_err(InteropError::external)
}

/// Publica o estado do frame: os campos do `ScriptHostState` e o
/// `MenusOpen` do jogo (falta do recurso = menus fechados, apps mínimas).
fn publish_globals(
    state: Res<ScriptHostState>,
    menus: Option<Res<bevy_ext_core::MenusOpen>>,
) {
    if let Ok(mut globals) = globals().write() {
        globals.time = state.time;
        globals.player = state.player;
        globals.scene.clone_from(&state.scene);
        globals.scene_entry.clone_from(&state.scene_entry);
        globals.biome.clone_from(&state.biome);
        globals.menu_open = menus.as_ref().is_some_and(|menus| menus.any());
    }
}

/// Publica o snapshot de input do frame para `viber.input.*`. Com `Option`
/// as apps sem `InputPlugin` (headless/REPL) publicam um snapshot vazio em
/// vez de ficarem com o frame anterior congelado.
fn publish_input(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
) {
    let mut input = InputSnapshot::default();
    if let Some(keys) = keys {
        input.keys_pressed = keys.get_just_pressed().copied().collect();
        input.keys_down = keys.get_pressed().copied().collect();
        input.keys_released = keys.get_just_released().copied().collect();
    }
    if let Some(mouse) = mouse {
        input.mouse_pressed = mouse.get_just_pressed().copied().collect();
        input.mouse_down = mouse.get_pressed().copied().collect();
        input.mouse_released = mouse.get_just_released().copied().collect();
    }
    if let Ok(mut globals) = globals().write() {
        globals.input = input;
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
    install_viber_api_with(lua, globals(), game::game_store())
}

fn install_viber_api_with(
    lua: &Lua,
    globals: Arc<RwLock<ScriptGlobals>>,
    store: Arc<RwLock<game::GameStore>>,
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

    // viber.scene_entry() — entrada por onde se chegou (nil = nenhuma).
    let entry_globals = globals.clone();
    viber.set(
        "scene_entry",
        lua.create_function(move |_, ()| {
            Ok(entry_globals
                .read()
                .ok()
                .and_then(|globals| globals.scene_entry.clone()))
        })?,
    )?;

    // viber.biome() — id do bioma atual (nil fora de bioma/interior).
    let biome_globals = globals.clone();
    viber.set(
        "biome",
        lua.create_function(move |_, ()| {
            Ok(biome_globals
                .read()
                .ok()
                .and_then(|globals| globals.biome.clone()))
        })?,
    )?;

    // viber.menu_open() — menu, mapa, viagem ou cinemática a roubar o input
    // (o `MenusOpen::any()` que o movimento nativo consulta).
    let menu_globals = globals.clone();
    viber.set(
        "menu_open",
        lua.create_function(move |_, ()| {
            Ok(menu_globals
                .read()
                .map(|globals| globals.menu_open)
                .unwrap_or(false))
        })?,
    )?;

    viber.set(
        "log",
        lua.create_function(|_, message: String| {
            log::info!("[lua] {message}");
            Ok(())
        })?,
    )?;

    keys::install(lua, &viber, globals.clone())?;

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

    game::install(lua, &viber, store)?;

    lua.globals().set("viber", viber)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::GameValue;

    /// VM isolada com stores próprios (os estáticos são partilhados pelo
    /// processo — cada teste injeta os seus para não se interferirem).
    fn vm() -> (Lua, Arc<RwLock<ScriptGlobals>>, Arc<RwLock<game::GameStore>>) {
        let lua = Lua::new();
        let globals = Arc::new(RwLock::new(ScriptGlobals::default()));
        let store = Arc::new(RwLock::new(game::GameStore::default()));
        install_viber_api_with(&lua, globals.clone(), store.clone()).expect("instala viber");
        (lua, globals, store)
    }

    #[test]
    fn viber_table_le_o_estado_do_frame() {
        let (lua, globals, _store) = vm();
        {
            let mut globals = globals.write().expect("globals");
            globals.time = 12.5;
            globals.player = Some(Vec3::new(1.0, 2.0, 3.0));
            globals.scene = Some("main".into());
        }

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
        let (lua, _globals, _store) = vm();
        let missing: bool = lua
            .load("return viber.player_position() == nil")
            .eval()
            .expect("position");
        assert!(missing);
    }

    #[test]
    fn game_escreve_e_le_primitivos_no_mapa() {
        let (lua, _globals, store) = vm();
        lua.load("viber.game().hp = 92")
            .exec()
            .expect("escreve hp");
        lua.load("viber.game().nome = 'Vael'")
            .exec()
            .expect("escreve nome");
        lua.load("viber.game().morto = true")
            .exec()
            .expect("escreve bool");

        let map = store.read().expect("store").map.clone();
        assert_eq!(map.get("hp"), Some(&GameValue::Num(92.0)));
        assert_eq!(map.get("nome"), Some(&GameValue::Str("Vael".into())));
        assert_eq!(map.get("morto"), Some(&GameValue::Bool(true)));

        // Leitura: do cache cru do proxy e via __index (outra chave).
        let hp: f64 = lua.load("return viber.game().hp").eval().expect("hp");
        assert_eq!(hp, 92.0);
        let nome: String = lua
            .load("local g = viber.game() return g['nome']")
            .eval()
            .expect("nome");
        assert_eq!(nome, "Vael");
    }

    #[test]
    fn nil_remove_a_chave_do_mapa() {
        let (lua, _globals, store) = vm();
        lua.load("viber.game().venom = 3.5").exec().expect("set");
        lua.load("viber.game().venom = nil").exec().expect("unset");
        assert!(store.read().expect("store").map.contains_key("venom") == false);
        let missing: bool = lua
            .load("return viber.game().venom == nil")
            .eval()
            .expect("leitura");
        assert!(missing);
    }

    #[test]
    fn tabelas_ficam_so_na_vm_de_sessao() {
        let (lua, _globals, store) = vm();
        lua.load("viber.game().enemies = {}")
            .exec()
            .expect("overlay");
        // O mapa plano não recebe tabelas…
        assert!(!store.read().expect("store").map.contains_key("enemies"));
        // …mas a sessão vê (e o `pairs` do proxy também).
        let ok: bool = lua
            .load("local g = viber.game() return type(g.enemies) == 'table'")
            .eval()
            .expect("sessão");
        assert!(ok);
        let seen: bool = lua
            .load("local g = viber.game() for k, v in pairs(g) do if k == 'enemies' then return type(v) == 'table' end end return false")
            .eval()
            .expect("pairs");
        assert!(seen);
    }

    #[test]
    fn game_e_o_mesmo_proxy_em_todas_as_chamadas() {
        let (lua, _globals, _store) = vm();
        let same: bool = lua
            .load("return viber.game() == viber.game()")
            .eval()
            .expect("identidade");
        assert!(same);
    }

    #[test]
    fn iteracao_generica_ve_o_mapa() {
        let (lua, _globals, _store) = vm();
        lua.load("viber.game().a = 1 viber.game().b = 2")
            .exec()
            .expect("semeia");
        let sum: f64 = lua
            .load("local s = 0 for k, v in viber.game() do s = s + v end return s")
            .eval()
            .expect("iteração");
        assert_eq!(sum, 3.0);
    }

    #[test]
    fn outra_vm_ve_as_escritas_e_as_remocoes() {
        let store = Arc::new(RwLock::new(game::GameStore::default()));
        let a = Lua::new();
        let b = Lua::new();
        let globals = Arc::new(RwLock::new(ScriptGlobals::default()));
        install_viber_api_with(&a, globals.clone(), store.clone()).expect("vm a");
        install_viber_api_with(&b, globals, store).expect("vm b");

        a.load("viber.game().ouro = 30").exec().expect("escreve");
        // O sync corre na chamada de `viber.game()` — sem ele o cache cru de
        // `b` ainda não tem a chave; com ele chega.
        let ouro: f64 = b.load("return viber.game().ouro").eval().expect("ouro");
        assert_eq!(ouro, 30.0);

        a.load("viber.game().ouro = nil").exec().expect("remove");
        let missing: bool = b
            .load("return viber.game().ouro == nil")
            .eval()
            .expect("remoção");
        assert!(missing);
    }

    #[test]
    fn patches_publicados_aplicam_e_null_remove() {
        let (lua, _globals, store) = vm();
        lua.load("viber.game().hp = 10").exec().expect("hp");
        // O Lua escreve no store injetado; o patch também.
        store.write().expect("store").publish_patch(vec![
            ("hp".into(), GameValue::Num(92.0)),
            ("venom".into(), GameValue::Null),
        ]);
        assert_eq!(
            store.read().expect("store").snapshot().get("hp"),
            Some(&GameValue::Num(10.0)),
            "patch ainda não aplicado"
        );
        store.write().expect("store").apply_patches();
        let snapshot = store.read().expect("store").snapshot();
        assert_eq!(snapshot.get("hp"), Some(&GameValue::Num(92.0)));
        assert!(!snapshot.contains_key("venom"));

        let hp: f64 = lua.load("return viber.game().hp").eval().expect("hp");
        assert_eq!(hp, 92.0);
        let missing: bool = lua
            .load("return viber.game().venom == nil")
            .eval()
            .expect("null");
        assert!(missing);
    }

    #[test]
    fn state_e_por_vm_e_partilhado_dentro_dela() {
        let (lua, _globals, _store) = vm();
        let ok: bool = lua
            .load(
                "local a = viber.state() a.aberta = true \
                 return a == viber.state() and viber.state().aberta == true",
            )
            .eval()
            .expect("state");
        assert!(ok);
        // Outra VM tem o SEU estado (sem `aberta`).
        let (outra, _g2, _s2) = vm();
        let missing: bool = outra
            .load("return viber.state().aberta == nil")
            .eval()
            .expect("isolamento");
        assert!(missing);
    }
    #[test]
    fn input_lee_os_tres_estados_do_snapshot() {
        let (lua, globals, _store) = vm();
        {
            let mut g = globals.write().expect("globals");
            g.input.keys_down.insert(KeyCode::KeyE);
            g.input.keys_down.insert(KeyCode::KeyW);
            g.input.keys_pressed.insert(KeyCode::KeyJ);
            g.input.keys_released.insert(KeyCode::KeyF);
            g.input.mouse_down.insert(MouseButton::Left);
            g.input.mouse_pressed.insert(MouseButton::Right);
        }
        let cases: Vec<bool> = lua
            .load(r#"
              return {
                viber.input.down('e'), viber.input.down('w'),
                viber.input.pressed('j'), viber.input.released('f'),
                viber.input.down('lmb'), viber.input.pressed('rmb'),
                viber.input.down('x') == false,
                viber.input.pressed('e') == false,
                viber.input.released('e') == false,
              }
            "#)
            .eval()
            .expect("input");
        assert!(cases.iter().all(|c| *c), "casos: {cases:?}");
    }

    #[test]
    fn input_normaliza_e_recusa_nomes() {
        let (lua, _globals, _store) = vm();
        let arrow: bool = lua
            .load("return viber.input.down('Arrow_Up')")
            .eval()
            .expect("arrow");
        assert!(!arrow); // sem snapshot: false, mas o nome foi aceite
        let erro: bool = lua
            .load("local ok = pcall(function() viber.input.down('não-existe') end) return not ok")
            .eval()
            .expect("erro");
        assert!(erro);
    }

    #[test]
    fn cena_entrada_e_bioma_vem_do_estado_publicado() {
        let (lua, globals, _store) = vm();
        {
            let mut g = globals.write().expect("globals");
            g.scene = Some("discordia".into());
            g.scene_entry = Some("from-town".into());
            g.biome = Some("floresta".into());
            g.menu_open = true;
        }
        let (scene, entry, biome, menu): (String, String, String, bool) = lua
            .load(
                "return viber.scene_name(), viber.scene_entry(), viber.biome(), viber.menu_open()",
            )
            .eval()
            .expect("estado");
        assert_eq!((scene.as_str(), entry.as_str(), biome.as_str(), menu),
                   ("discordia", "from-town", "floresta", true));

        // Sem publicação: nil/nil/nil e menus fechados.
        let (lua2, _g2, _s2) = vm();
        let (entry_nil, biome_nil, menu_closed): (bool, bool, bool) = lua2
            .load(
                "return viber.scene_entry() == nil, viber.biome() == nil, viber.menu_open() == false",
            )
            .eval()
            .expect("defaults");
        assert!(entry_nil && biome_nil && menu_closed);
    }

}