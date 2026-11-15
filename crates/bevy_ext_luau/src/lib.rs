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
//! enfileirados e loader de módulos.
//!
//! exit4/F2–F5 (2026-11-15): portados o grupo `events`, o NÚCLEO
//! (`log`/`position`/`set_position`/`distance_to_player`/`player_forward`/
//! `player_hp`/`player_id`/`entity_id`/`self_name`/`home`), a percepção e
//! props (F4: `nearby` com a semântica do monólito, `hostiles`, `find`,
//! `ground_below`/`road` via SEAM `SurfaceQueries`, `prop`/`props`,
//! `attach`, `set_pose`/`set_scale`/`set_visible`, `topple`/`corpse`,
//! `own_system`/`own_action`…) e os efeitos/áudio (F5: `sound`/`sound_loop`/
//! `sound_stop`, `music`/`audio`, `punch`/`kick`/`lantern`, `camera_free`/
//! `camera_pose`/`camera_state`, `bone_offset`, `clip_duration`). O grupo
//! `game` é UM SÓ (o gémeo do monólito virou shim `viber::luau::game` —
//! proxy sobre o store, interop JSON do save, `evict_module`, `load_json`).
//! `exports.rs`/`ownership.rs` são do BMS (shims no monólito).
//!
//! PENDENTES (documentados no relatório da migração): F6 IA/combate
//! (`sense`/`wander_target`/`next_state`/`entity_damage`/`radial_damage`/
//! `fire_projectile`/`push_player`), F7 save/câmara/resto (`save*`/`quit`/
//! `locale`/`say`/`prompt`/`markers`/`a11y`/`net_role`), F8
//! `viber.nlu`/`viber.decision` (features CelerAI) e `viber.vael` (do
//! próprio jogo). Timers/signals (`viber.after`/`viber.signals`) seguem na
//! onda do host.

pub mod commands;
pub mod entity;
pub mod events;
pub mod exports;
pub mod game;
pub mod keys;
// F8: os módulos IA são INCONDICIONAIS (o `mod imp` é que é cfg-gated —
// sem a feature, `available()`=false e o jogo segue no fluxo antigo). Assim
// o shim do monólito resolve em qualquer estado de features e o monólito
// reencaminha as dele (`nlu = ["dep:celer-nlu", "bevy_ext_luau/nlu"]`).
pub mod nlu;
pub mod ownership;
pub mod props;
pub mod sfx;
pub mod surfaces;
pub mod timers;
pub mod decision;

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

pub use commands::{ScriptCommand, ScriptCommandMessage};
pub use game::{GameValue, apply_game_patches, game_snapshot, publish_game_patch};
pub use surfaces::{RoadSample, SurfaceQueries, SurfaceQueriesHandle};

/// Criatura hostil viva neste frame (`viber.hostiles`) — portado do
/// `HostileSnap` do monólito.
#[derive(Debug, Clone)]
pub struct HostileSnap {
    pub id: i64,
    pub kind: String,
    pub pos: Vec3,
    pub hp: f32,
    pub max: f32,
    /// Forward do modelo no plano (XZ, unitário) — o jogo usa-o para backstab.
    pub forward: Vec2,
}

/// Entidade do attach corrente (scripts `EntityScript`); `None` em scripts
/// estáticos. As funções de percepção usam-na para se excluírem a si.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScriptEntity(pub Option<Entity>);

/// Estado do frame visível para os scripts. O jogo preenche-o; as funções
/// Lua leem-no sem acesso directo ao ECS.
#[derive(Resource, Default, Clone)]
pub struct ScriptHostState {
    pub time: f64,
    pub player: Option<Vec3>,
    pub scene: Option<String>,
    /// Entrada `<SceneEntry>` por onde se chegou à cena (`viber.scene_entry`).
    pub scene_entry: Option<String>,
    /// Id do `BiomeRegion` atual; `None` fora de biomas/interiores
    /// (`viber.biome`).
    pub biome: Option<String>,
    /// Pasta `scripts/` do projeto para `viber.load` (o loader de módulos
    /// lê ficheiros .lua daqui). O `publish_globals` espelha-a no static do
    /// `game` — a fonte do `viber.load`/`load_json` de todas as VMs.
    pub scripts_dir: Option<std::path::PathBuf>,
    /// Forward do herói no plano (x, z) — `viber.player_forward()`.
    pub player_forward: Vec2,
    /// Snapshot `(current, max)` do HP do herói — `viber.player_hp()`.
    pub player_hp: Option<(f32, f32)>,
    /// Bits da entidade do herói — `viber.player_id()`.
    pub player_id: Option<i64>,
    /// Posição de início-de-frame por entidade (bits → posição) para
    /// `viber.position()`/`distance_to_player` da entidade que corre o
    /// script. O monólito semeava `ScriptCtx::origin` por chamada; aqui é o
    /// snapshot publicado.
    pub positions: std::collections::HashMap<i64, Vec3>,
    /// Nome de mundo por entidade (bits → nome) para `viber.self_name()`.
    pub names: std::collections::HashMap<i64, String>,
    /// Snapshot `nome → bits` de TODAS as entidades nomeadas do mundo
    /// (`viber.find`/`find_all`; vetores ordenados por bits).
    pub named_entities: std::collections::HashMap<String, Vec<i64>>,
    /// Snapshot `(current, max)` do HP por entidade COM vitais (sem o
    /// player) para `viber.entity_hp(id)`.
    pub hp_snapshot: std::collections::HashMap<i64, (f32, f32)>,
    /// Props autorais por entidade (`viber.nearby`/`viber.prop`).
    pub props: std::collections::HashMap<i64, std::sync::Arc<Vec<(String, String)>>>,
    /// Esquema `--@export` por script path (`viber.prop`/`viber.props`) —
    /// o parser vive no `exports` deste crate.
    pub exports: std::collections::HashMap<String, std::sync::Arc<exports::ScriptExports>>,
    /// Hostis vivos deste frame (`viber.hostiles`).
    pub hostiles: Vec<HostileSnap>,
    /// Nomes dos prototypes do mundo (validação de `viber.spawn_prototype`).
    pub prototype_names: Vec<String>,
    /// Presets de partícula válidos (`viber.burst`; `sparks` entra à parte).
    pub particle_presets: Vec<String>,
    /// Consultas de superfície (`viber.ground_below`/`road`/`road_at`/
    /// `on_road`/`in_water`) — seam [`surfaces::SurfaceQueries`] implementado
    /// pela integração.
    pub surfaces: Option<surfaces::SurfaceQueriesHandle>,
    /// Pose VIVA da câmara de jogo (`viber.camera_state`).
    pub camera: Option<(Vec3, f32, f32)>,
    /// Duração de cada clip do herói (`viber.clip_duration`), nomes
    /// normalizados → segundos.
    pub clip_durations: std::collections::HashMap<String, f32>,
    /// Grupos por entidade (`viber.group`/`in_group`): bits → nomes.
    /// exit5/ponte (2026-11-15).
    pub groups: std::collections::HashMap<i64, Vec<String>>,
    /// Id da cinemática em curso (`viber.cinematic_playing()`); `None` fora.
    pub cinematic: Option<String>,
    /// Acessibilidade publicada (`viber.a11y`): escala de legendas, shake,
    /// hit-stop e hold de interação (o jogo desenha; a engine aplica).
    pub a11y: (f32, bool, bool, bool),
    /// Papel de rede (`viber.net_role`): "off" | "host" | "join".
    pub net_role: Option<String>,
    /// Idioma ativo (`viber.locale.get()`).
    pub locale_lang: Option<String>,
    /// Tabela do locale ativo para `viber.localize` (chaves `@nome`).
    pub locale_table: Option<std::collections::HashMap<String, String>>,
    /// Metadados dos 4 slots (`viber.save_slots`), pares (chave, JSON) por
    /// slot — o mesmo conjunto de chaves que o monólito escreve na tabela.
    pub save_slots: Option<Vec<Vec<(String, serde_json::Value)>>>,
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

#[derive(Default)]
pub struct ScriptGlobals {
    time: f64,
    player: Option<Vec3>,
    scene: Option<String>,
    scene_entry: Option<String>,
    biome: Option<String>,
    menu_open: bool,
    /// Forward do herói no plano (x, z) — `viber.player_forward()`.
    player_forward: Vec2,
    /// Snapshot `(current, max)` do HP do herói — `viber.player_hp()`.
    player_hp: Option<(f32, f32)>,
    /// Bits da entidade do herói — `viber.player_id()`.
    player_id: Option<i64>,
    /// Posição de início-de-frame das entidades com script
    /// (`viber.position`/`distance_to_player` da própria entidade).
    positions: std::collections::HashMap<i64, Vec3>,
    /// Nome de mundo por entidade (`viber.self_name`).
    names: std::collections::HashMap<i64, String>,
    /// Snapshot `nome → bits` de TODAS as entidades nomeadas do mundo para
    /// `viber.find`/`find_all` (vetores ordenados por bits: ordem estável).
    named_entities: std::collections::HashMap<String, Vec<i64>>,
    /// Snapshot `(current, max)` do HP de cada entidade COM vitais (sem o
    /// player) para `viber.entity_hp(id)`.
    hp_snapshot: std::collections::HashMap<i64, (f32, f32)>,
    /// Props autorais (`prop-*`) por entidade — DADOS que o jogo interpreta.
    props: std::collections::HashMap<i64, std::sync::Arc<Vec<(String, String)>>>,
    /// Esquema `--@export` por script path (`viber.prop`/`viber.props`).
    exports: std::collections::HashMap<String, std::sync::Arc<exports::ScriptExports>>,
    /// Hostis vivos (`enemies/`/`bosses/`) para `viber.hostiles`.
    hostiles: Vec<HostileSnap>,
    /// Nomes dos prototypes do mundo (validação de `viber.spawn_prototype`).
    prototype_names: Vec<String>,
    /// Presets de partícula válidos para `viber.burst` (`sparks` à parte).
    particle_presets: Vec<String>,
    /// Consultas de superfície publicadas (`viber.ground_below`/`road`/…).
    surfaces: Option<surfaces::SurfaceQueriesHandle>,
    /// Pose VIVA da câmara de jogo — `viber.camera_state()`: posição do olho
    /// e direção de vista (yaw/pitch em graus, convenção do `camera_pose`).
    camera: Option<(Vec3, f32, f32)>,
    /// Duração de cada clip do herói (`viber.clip_duration`), nomes
    /// normalizados → segundos.
    clip_durations: std::collections::HashMap<String, f32>,
    /// Ring de `viber.log` (cap 256; linhas até 8 KiB como o monólito).
    logs: VecDeque<String>,
    toasts: VecDeque<String>,
    /// Id da cinemática em curso (`viber.cinematic_playing()`); `None` fora.
    cinematic: Option<String>,
    /// Acessibilidade publicada (`viber.a11y`): escala de legendas, shake,
    /// hit-stop e hold de interação (o jogo desenha; a engine aplica).
    a11y: (f32, bool, bool, bool),
    /// Papel de rede (`viber.net_role`): "off" | "host" | "join".
    net_role: Option<String>,
    /// Tabela do locale ativo para `viber.localize` (chaves `@nome`).
    locale_table: Option<std::collections::HashMap<String, String>>,
    /// Idioma ativo (`viber.locale.get()`).
    locale_lang: Option<String>,
    /// Metadados dos 4 slots (`viber.save_slots`), JSON do sidecar.
    save_slots: Option<Vec<Vec<(String, serde_json::Value)>>>,
    /// Grupos por entidade (`viber.group`/`in_group`): bits → nomes.
    groups: std::collections::HashMap<i64, Vec<String>>,
    input: InputSnapshot,
    /// Comandos `viber.*` enfileirados nesta frame; [`drain_commands`]
    /// move-os para as mensagens no Update.
    commands: VecDeque<commands::ScriptCommand>,
}

static SCRIPT_GLOBALS: OnceLock<Arc<RwLock<ScriptGlobals>>> = OnceLock::new();

fn globals() -> Arc<RwLock<ScriptGlobals>> {
    SCRIPT_GLOBALS
        .get_or_init(|| Arc::new(RwLock::new(ScriptGlobals::default())))
        .clone()
}

/// Host do Vael. Requer `AssetPlugin` (o BMS carrega scripts como assets).
pub struct VaelScriptingPlugin;

/// F2 (exit4): o estado da UI declarativa capturado no build do plugin —
/// o `ContextInitializer` do BMS é um FN POINTER (não captura), portanto o
/// `install_ui_api` do `bevy_ext_ui` lê-o daqui. Os Arcs internos partilham
/// as MESMAS filas com o `UiPlugin`; setado uma vez por processo (o plugin
/// tem de ser adicionado DEPOIS do plugin de UI — sem o recurso, os
/// contextos não levam `viber.ui` e os HUD scripts não correm).
#[cfg(feature = "ui")]
static UI_SCRIPT_STATE: OnceLock<bevy_ext_ui::ui::script::UiScriptState> = OnceLock::new();

impl Plugin for VaelScriptingPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "ui")]
        if let Some(state) = app
            .world()
            .get_resource::<bevy_ext_ui::ui::script::UiScriptState>()
        {
            let _ = UI_SCRIPT_STATE.set(state.clone());
        } else {
            log::warn!("[lua] feature `ui` ligada sem UiScriptState no mundo —                 adicione o VaelScriptingPlugin DEPOIS do plugin de UI; HUD scripts não correm");
        }
        app.init_resource::<ScriptHostState>()
            // O BMS precisa da infraestrutura de contextos; o plugin Lua é
            // configurado com o inicializador da tabela `viber`.
            .add_plugins(BMSScriptingInfrastructurePlugin::default())
            .add_plugins(
                LuaScriptingPlugin::default().add_context_initializer(init_context),
            )
            // Mensagem dos comandos sem consumidor no core (feedback de
            // câmara/animação); o adaptador do Vael regista o leitor.
            .add_message::<commands::ScriptCommandMessage>()
            .add_systems(
                PreUpdate,
                (
                    timers::tick_timers,
                    publish_globals,
                    publish_input,
                    game::apply_game_patches,
                    events::fan_out_events,
                ),
            )
            .add_systems(Update, (drain_toasts, commands::drain_commands));
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
    // Path do script do contexto (`viber.events` subscreve por path; a chave
    // é o asset path do handle — o que a integração registar).
    let path = match attachment {
        ScriptAttachment::EntityScript(_, script) | ScriptAttachment::StaticScript(script) => {
            script.path().map(|path| path.to_string()).unwrap_or_default()
        }
    };
    context.set_app_data(events::ScriptPath(path));
    let _ = entity;
    install_viber_api_with(context, globals(), game::game_store())
    .map_err(InteropError::external)?;
    // F2: `viber.ui` (a superfície da UI declarativa) — depois do `viber` nos
    // globals; as filas são Arc partilhado com o UiPlugin.
    #[cfg(feature = "ui")]
    if let Some(state) = UI_SCRIPT_STATE.get() {
        bevy_ext_ui::ui::script::install_ui_api(&**context, state)
            .map_err(InteropError::external)?;
    }
    Ok(())
}

/// Publica o estado do frame: os campos do `ScriptHostState` e o
/// `MenusOpen` do jogo (falta do recurso = menus fechados, apps mínimas).
/// `pub`: a ponte do monólito (`bevy_ext_viber_engine::luau::bms_bridge`)
/// ordena o publisher do estado ANTES deste sistema (`.before`).
pub fn publish_globals(
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
        globals.player_forward = state.player_forward;
        globals.player_hp = state.player_hp;
        globals.player_id = state.player_id;
        globals.positions.clone_from(&state.positions);
        globals.names.clone_from(&state.names);
        globals.named_entities.clone_from(&state.named_entities);
        globals.hp_snapshot.clone_from(&state.hp_snapshot);
        globals.props.clone_from(&state.props);
        globals.exports.clone_from(&state.exports);
        globals.hostiles.clone_from(&state.hostiles);
        globals.prototype_names.clone_from(&state.prototype_names);
        globals.particle_presets.clone_from(&state.particle_presets);
        globals.surfaces = state.surfaces.clone();
        globals.camera = state.camera;
        globals.clip_durations.clone_from(&state.clip_durations);
        // exit5/ponte: o resto do estado do frame (grupos, cinemática,
        // acessibilidade, rede, locale, slots de save) publicados pela
        // integração no mesmo recurso.
        globals.groups.clone_from(&state.groups);
        globals.cinematic.clone_from(&state.cinematic);
        globals.a11y = state.a11y;
        globals.net_role.clone_from(&state.net_role);
        globals.locale_lang.clone_from(&state.locale_lang);
        globals.locale_table.clone_from(&state.locale_table);
        globals.save_slots.clone_from(&state.save_slots);
    }
    // A pasta de scripts do frame publica-se no static do `game` (a fonte
    // do `viber.load`/`load_json` de todas as VMs). Só `Some` escreve — o
    // host do monólito semeia o static no `new` e o `None` de apps mínimas
    // não o limpa.
    if let Some(dir) = state.scripts_dir.as_ref() {
        game::set_scripts_dir(Some(dir.clone()));
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

/// Enfila um comando `viber.*` (falha silenciosa só quando o lock está
/// envenenado — um pânico noutro teste/thread).
pub(crate) fn push_command(
    globals: &Arc<RwLock<ScriptGlobals>>,
    command: commands::ScriptCommand,
) {
    if let Ok(mut globals) = globals.write() {
        globals.commands.push_back(command);
    }
}

/// Centro do `viber.nearby`: a entidade que corre o script (excluindo-se a
/// si) ou o HERÓI (`player_nearby` — controllers always-active não vivem no
/// herói).
#[derive(Debug, Clone, Copy, PartialEq)]
enum NearbyCenter {
    SelfEntity,
    Player,
}

/// Constrói a tabela `viber.nearby`/`player_nearby`: filtra ANTES de ordenar
/// (o snapshot pode ter milhares de nomeadas por frame — clonar cada nome
/// para depois o deitar fora custava 3–4 ms por chamada no Vael), cap 64.
#[allow(clippy::type_complexity)]
fn build_nearby(
    lua: &Lua,
    shared: &Arc<RwLock<ScriptGlobals>>,
    radius: f32,
    limit: Option<usize>,
    center: NearbyCenter,
) -> mlua::Result<mlua::Table> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(mlua::Error::runtime(format!(
            "viber.{}: raio tem de ser finito e > 0",
            if center == NearbyCenter::Player { "player_nearby" } else { "nearby" }
        )));
    }
    let out = lua.create_table()?;
    let Ok(g) = shared.read() else {
        return Ok(out);
    };
    let own = lua
        .app_data_ref::<ScriptEntity>()
        .and_then(|e| e.0)
        .map(|entity| entity.to_bits() as i64);
    let origin = match center {
        NearbyCenter::Player => match g.player {
            Some(p) => p,
            // Sem herói não há centro: tabela vazia (o Viber devolvia {} sem
            // player também).
            None => return Ok(out),
        },
        NearbyCenter::SelfEntity => match own.and_then(|bits| g.positions.get(&bits).copied()) {
            Some(p) => p,
            None => return Ok(out),
        },
    };
    let mut found: Vec<(f32, i64, &str, Vec3)> = g
        .names
        .iter()
        .filter_map(|(bits, name)| {
            if center == NearbyCenter::SelfEntity && Some(*bits) == own {
                return None;
            }
            let pos = *g.positions.get(bits)?;
            let d = pos.distance(origin);
            (d <= radius).then_some((d, *bits, name.as_str(), pos))
        })
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let cap = limit.unwrap_or(16).min(64);
    for (i, (distance, bits, name, pos)) in found.into_iter().take(cap).enumerate() {
        let t = lua.create_table()?;
        t.raw_set("id", bits)?;
        t.raw_set("name", name)?;
        t.raw_set("x", pos.x)?;
        t.raw_set("y", pos.y)?;
        t.raw_set("z", pos.z)?;
        t.raw_set("distance", distance)?;
        if let Some(props) = g.props.get(&bits) {
            let table = lua.create_table()?;
            for (key, value) in props.iter() {
                table.raw_set(key.as_str(), value.as_str())?;
            }
            t.raw_set("props", table)?;
        }
        out.raw_set(i + 1, t)?;
    }
    Ok(out)
}

/// A tabela `viber.road`/`road_at`: `y` é o tabuleiro quando o ponto está
/// NUM vão suspenso e dentro da fita; senão é o terreno (fallback: deck_y).
fn road_table(lua: &Lua, sample: &RoadSample, ground: Option<f32>) -> mlua::Result<mlua::Table> {
    let on = sample.center_dist <= sample.half_width + 1e-3;
    let y = if sample.deck && on {
        sample.deck_y
    } else {
        ground.unwrap_or(sample.deck_y)
    };
    let table = lua.create_table()?;
    table.raw_set("y", y)?;
    table.raw_set("s", sample.s)?;
    table.raw_set("length", sample.length)?;
    table.raw_set("half_width", sample.half_width)?;
    table.raw_set("lateral", sample.lateral)?;
    table.raw_set("deck", sample.deck && on)?;
    table.raw_set("heading", sample.heading)?;
    table.raw_set("on", on)?;
    table.raw_set("cx", sample.cx)?;
    table.raw_set("cz", sample.cz)?;
    Ok(table)
}

/// A entidade do contexto (app data do attach). Erro Lua quando o script
/// corre fora de `on_update` (attach estático/REPL) — a mesma semântica do
/// "fora de on_update" do monólito para comandos que precisam de entidade.
fn script_entity(lua: &Lua) -> mlua::Result<Entity> {
    lua.app_data_ref::<ScriptEntity>()
        .and_then(|e| e.0)
        .ok_or_else(|| mlua::Error::runtime("fora de on_update (sem entidade no attach)"))
}

/// A mesma entidade, em `Option` (comandos que aceitam `None` = sem dono).
fn script_entity_opt(lua: &Lua) -> Option<Entity> {
    lua.app_data_ref::<ScriptEntity>().and_then(|e| e.0)
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

    // ── Núcleo portado do monólito (exit4/F3) ───────────────────────────

    // viber.log(msg) — log da engine + ring buffer (cap 256, linha até
    // 8 KiB — o mesmo push_log do ScriptCtx).
    let log_globals = globals.clone();
    viber.set(
        "log",
        lua.create_function(move |lua, msg: String| {
            let entity = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .map(|e| format!("{e:?}"))
                .unwrap_or_else(|| "pre-activate".into());
            log::info!(target: "viber::luau", "[{entity}] {msg}");
            if let Ok(mut globals) = log_globals.write() {
                const MAX_LOG_MESSAGE: usize = 8192;
                let mut line = msg;
                if line.len() > MAX_LOG_MESSAGE {
                    let mut end = MAX_LOG_MESSAGE;
                    while !line.is_char_boundary(end) {
                        end -= 1;
                    }
                    line.truncate(end);
                    line.push('\u{2026}');
                }
                if globals.logs.len() >= 256 {
                    globals.logs.pop_front();
                }
                globals.logs.push_back(line);
            }
            Ok(())
        })?,
    )?;

    // viber.position() -> x, y, z — snapshot de início-de-frame da entidade
    // que corre o script (do mapa publicado); zeros fora de on_update.
    let position_globals = globals.clone();
    viber.set(
        "position",
        lua.create_function(move |lua, ()| {
            let origin = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .and_then(|entity| {
                    position_globals
                        .read()
                        .ok()
                        .and_then(|g| g.positions.get(&(entity.to_bits() as i64)).copied())
                })
                .unwrap_or_default();
            Ok((origin.x, origin.y, origin.z))
        })?,
    )?;

    // viber.set_position(x, y, z) — enfileira o comando (aplicado pós-frame
    // com snap no terreno). Erro fora de on_update, como no monólito.
    let setpos_globals = globals.clone();
    viber.set(
        "set_position",
        lua.create_function(move |lua, (x, y, z): (f32, f32, f32)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.set_position: coordenadas não finitas (NaN/inf)",
                ));
            }
            let entity = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .ok_or_else(|| {
                    mlua::Error::runtime(
                        "viber.set_position outside on_update (no owning entity)",
                    )
                })?;
            push_command(
                &setpos_globals,
                commands::ScriptCommand::SetPosition {
                    entity,
                    pos: Vec3::new(x, y, z),
                },
            );
            Ok(())
        })?,
    )?;

    // viber.distance_to_player() -> number | nil (nil = sem herói ou fora
    // de on_update).
    let distance_globals = globals.clone();
    viber.set(
        "distance_to_player",
        lua.create_function(move |lua, ()| {
            let origin = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .and_then(|entity| {
                    distance_globals
                        .read()
                        .ok()
                        .and_then(|g| g.positions.get(&(entity.to_bits() as i64)).copied())
                });
            let player = distance_globals.read().ok().and_then(|g| g.player);
            Ok(player.zip(origin).map(|(p, o)| p.distance(o)))
        })?,
    )?;

    // viber.player_forward() -> dx, dz — forward do herói no plano.
    let forward_globals = globals.clone();
    viber.set(
        "player_forward",
        lua.create_function(move |_, ()| {
            let (x, z) = forward_globals
                .read()
                .map(|g| (g.player_forward.x, g.player_forward.y))
                .unwrap_or_default();
            Ok((x, z))
        })?,
    )?;

    // viber.player_hp() -> ok, cur, max — snapshot do HP do herói.
    let hp_globals = globals.clone();
    viber.set(
        "player_hp",
        lua.create_function(move |_, ()| match hp_globals.read().ok().and_then(|g| g.player_hp) {
            Some((cur, max)) => Ok((true, cur, max)),
            None => Ok((false, 0.0, 0.0)),
        })?,
    )?;

    // viber.player_id() -> bits | nil — a entidade do herói.
    let pid_globals = globals.clone();
    viber.set(
        "player_id",
        lua.create_function(move |_, ()| Ok(pid_globals.read().ok().and_then(|g| g.player_id)))?,
    )?;

    // viber.entity_id() -> bits | nil — a entidade que corre o script agora.
    viber.set(
        "entity_id",
        lua.create_function(|lua, ()| {
            Ok(lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .map(|entity| entity.to_bits() as i64))
        })?,
    )?;

    // viber.self_name() -> string — nome de MUNDO da entidade corrente
    // (do mapa publicado); vazio fora de on_update/sem Name.
    let name_globals = globals.clone();
    viber.set(
        "self_name",
        lua.create_function(move |lua, ()| {
            Ok(lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .and_then(|entity| {
                    name_globals
                        .read()
                        .ok()
                        .and_then(|g| g.names.get(&(entity.to_bits() as i64)).cloned())
                })
                .unwrap_or_default())
        })?,
    )?;

    // viber.home() -> x, z — posição de SPAWN (centro do wander), lida da
    // tabela de estado semeada em `activate_at`; `position()` é a ACTUAL e
    // fazia scripts de leash orbitarem o sítio corrente.
    let home_globals = globals.clone();
    viber.set(
        "home",
        lua.create_function(move |lua, ()| {
            let fallback = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .and_then(|entity| {
                    home_globals
                        .read()
                        .ok()
                        .and_then(|g| g.positions.get(&(entity.to_bits() as i64)).copied())
                })
                .unwrap_or_default();
            let state: mlua::Table = lua.named_registry_value("viber_state")?;
            if let Ok(home) = state.raw_get::<mlua::Table>("home")
                && let (Ok(x), Ok(z)) = (home.raw_get::<f32>("x"), home.raw_get::<f32>("z"))
            {
                return Ok((x, z));
            }
            Ok((fallback.x, fallback.z))
        })?,
    )?;

    // ── Comandos enfileirados (drenados no Update) ──────────────────────

    // viber.sound_at_player(name) — SFX curto na posição do herói; sem
    // herói toca como som de interface (volume cheio). Nome inválido =
    // erro de script (o silêncio era o pior diagnóstico).
    let sfx_globals = globals.clone();
    viber.set(
        "sound_at_player",
        lua.create_function(move |_, name: String| {
            let clip = sfx::sfx_clip_from_str(&name).ok_or_else(|| {
                let names: Vec<&str> =
                    sfx::SFX_NAME_REGISTRY.iter().map(|(n, _)| *n).collect();
                mlua::Error::runtime(format!(
                    "clip de som desconhecido '{name}' ({})",
                    names.join(", ")
                ))
            })?;
            let position = sfx_globals.read().ok().and_then(|g| g.player);
            push_command(&sfx_globals, commands::ScriptCommand::PlaySfx { clip, position });
            Ok(())
        })?,
    )?;

    // viber.play_clip(nome [, opts]) — clip de ação no rig (opts:
    // {speed = 1.0, id = bits, lock = false}). Sem `id` a integração
    // resolve para a entidade do script.
    let clip_globals = globals.clone();
    viber.set(
        "play_clip",
        lua.create_function(move |lua, (name, opts): (String, Option<mlua::Table>)| {
            let (speed, id, lock) = match opts {
                Some(t) => (
                    t.raw_get::<f32>("speed").ok().unwrap_or(1.0),
                    t.raw_get::<i64>("id").ok(),
                    t.raw_get::<bool>("lock").ok().unwrap_or(false),
                ),
                None => (1.0, None, false),
            };
            if !speed.is_finite() || speed <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.play_clip: speed tem de ser finito e > 0",
                ));
            }
            let entity = match id {
                Some(bits) => Some(commands::entity_from_id(bits)?),
                None => Some(script_entity(lua)?),
            };
            push_command(
                &clip_globals,
                commands::ScriptCommand::Gesture { entity, name, speed, lock },
            );
            Ok(())
        })?,
    )?;

    // viber.shake(amount) / viber.fov_kick(deg) / viber.hit_stop(secs)
    let shake_globals = globals.clone();
    viber.set(
        "shake",
        lua.create_function(move |_, amount: f32| {
            if !amount.is_finite() || amount < 0.0 {
                return Err(mlua::Error::runtime("viber.shake: valor finito e >= 0"));
            }
            push_command(&shake_globals, commands::ScriptCommand::Shake(amount));
            Ok(())
        })?,
    )?;
    let fov_globals = globals.clone();
    viber.set(
        "fov_kick",
        lua.create_function(move |_, deg: f32| {
            if !deg.is_finite() {
                return Err(mlua::Error::runtime("viber.fov_kick: valor não finito"));
            }
            push_command(&fov_globals, commands::ScriptCommand::FovKick(deg));
            Ok(())
        })?,
    )?;
    let hitstop_globals = globals.clone();
    viber.set(
        "hit_stop",
        lua.create_function(move |_, secs: f32| {
            if !secs.is_finite() || secs < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.hit_stop: segundos finitos e >= 0",
                ));
            }
            push_command(&hitstop_globals, commands::ScriptCommand::HitStop(secs));
            Ok(())
        })?,
    )?;

    // viber.damage_number(texto [, opts]) — default: 1,8 m acima do herói;
    // cada eixo dado SUBSTITUI o default (um `{y = 3}` não exige x e z).
    let dmg_globals = globals.clone();
    viber.set(
        "damage_number",
        lua.create_function(move |_, (text, opts): (String, Option<mlua::Table>)| {
            let base = dmg_globals
                .read()
                .ok()
                .and_then(|g| g.player)
                .unwrap_or(Vec3::ZERO)
                + Vec3::Y * 1.8;
            let mut pos = base;
            let mut color = None;
            if let Some(t) = opts {
                let axis = |name: &str, fallback: f32| {
                    t.raw_get::<f32>(name)
                        .ok()
                        .filter(|value| value.is_finite())
                        .unwrap_or(fallback)
                };
                pos = Vec3::new(axis("x", base.x), axis("y", base.y), axis("z", base.z));
                if let Ok(Some(hex)) = t.raw_get::<Option<String>>("color") {
                    color = Some(commands::parse_hex_color(&hex).ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "viber.damage_number: cor inválida '{hex}'"
                        ))
                    })?);
                }
            }
            push_command(
                &dmg_globals,
                commands::ScriptCommand::DamageNumber { text, pos, color },
            );
            Ok(())
        })?,
    )?;

    // viber.ring(x, z, raio [, cor "#hex"]) — anel de choque no chão.
    let ring_globals = globals.clone();
    viber.set(
        "ring",
        lua.create_function(
            move |_, (x, z, radius, color): (f32, f32, f32, Option<String>)| {
                if !(x.is_finite() && z.is_finite() && radius.is_finite()) || radius <= 0.0 {
                    return Err(mlua::Error::runtime(
                        "viber.ring: coordenadas finitas e raio > 0",
                    ));
                }
                let color = match color {
                    Some(hex) => Some(commands::parse_hex_color(&hex).ok_or_else(|| {
                        mlua::Error::runtime(format!("viber.ring: cor inválida '{hex}'"))
                    })?),
                    None => None,
                };
                push_command(
                    &ring_globals,
                    commands::ScriptCommand::Ring { x, z, radius, color },
                );
                Ok(())
            },
        )?,
    )?;

    // ── Interação e percepção ───────────────────────────────────────────

    // viber.gesture(name) — gesto one-shot no rig da própria entidade
    // (speed 1.0, sem lock; o `play_clip` é a forma generalizada). Erro fora
    // de on_update (a entidade é conhecida no attach do contexto), como no
    // monólito.
    let gesture_globals = globals.clone();
    viber.set(
        "gesture",
        lua.create_function(move |lua, name: String| {
            let entity = script_entity(lua)?;
            push_command(
                &gesture_globals,
                commands::ScriptCommand::Gesture {
                    entity: Some(entity),
                    name,
                    speed: 1.0,
                    lock: false,
                },
            );
            Ok(())
        })?,
    )?;

    // viber.set_interaction(label, key [, range]) — valida tecla e alcance
    // À FILA (erro de script, não silêncio). Default: o alcance BASE
    // autorado; a escala global entra na integração.
    let interaction_globals = globals.clone();
    viber.set(
        "set_interaction",
        lua.create_function(
            move |lua, (label, key, range): (String, String, Option<f32>)| {
                keys::key_code_from_str(&key).ok_or_else(|| {
                    mlua::Error::runtime(format!(
                        "viber.set_interaction: tecla desconhecida '{key}'"
                    ))
                })?;
                let range = range.unwrap_or(commands::BASE_RANGE_M);
                if !range.is_finite() || range < 0.0 {
                    return Err(mlua::Error::runtime(
                        "viber.set_interaction: range tem de ser finito e >= 0",
                    ));
                }
                let entity = script_entity(lua)?;
                push_command(
                    &interaction_globals,
                    commands::ScriptCommand::SetInteraction {
                        entity: Some(entity),
                        label,
                        key,
                        range,
                    },
                );
                Ok(())
            },
        )?,
    )?;

    // ── F5: efeitos/áudio/câmara ────────────────────────────────────────

    // viber.sound(clip) — SFX curto na posição da entidade (volume cai com
    // a distância à câmara). Nome inválido = erro de script (warn 1×).
    // `{x=, y=, z=}` posiciona o SFX no mundo (golpe num prop); sem
    // overrides toca na origem da entidade que corre o script.
    let sound_globals = globals.clone();
    viber.set(
        "sound",
        lua.create_function(move |lua, (name, opts): (String, Option<mlua::Table>)| {
            let clip = sfx::sfx_clip_from_str(&name).ok_or_else(|| {
                let names: Vec<&str> = sfx::SFX_NAME_REGISTRY.iter().map(|(n, _)| *n).collect();
                mlua::Error::runtime(format!(
                    "clip de som desconhecido '{name}' ({})",
                    names.join(", ")
                ))
            })?;
            // Origem da entidade corrente (do snapshot publicado); 0 fora de
            // on_update — o mesmo default do monólito (ctx.origin).
            let origin = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .and_then(|entity| {
                    sound_globals
                        .read()
                        .ok()
                        .and_then(|g| g.positions.get(&(entity.to_bits() as i64)).copied())
                })
                .unwrap_or_default();
            let position = match opts {
                Some(opts) => {
                    let x = opts.raw_get::<Option<f32>>("x")?;
                    let y = opts.raw_get::<Option<f32>>("y")?;
                    let z = opts.raw_get::<Option<f32>>("z")?;
                    // Um NaN aqui seguia para o evento de SFX: a atenuação
                    // por distância dava NaN e o volume chegava NaN ao mixer
                    // (o resto das primitivas com posição rejeita não-finitos).
                    for (axis, value) in [("x", x), ("y", y), ("z", z)] {
                        if value.is_some_and(|v| !v.is_finite()) {
                            return Err(mlua::Error::runtime(format!(
                                "viber.sound('{name}'): {axis} tem de ser finito (NaN/inf)"
                            )));
                        }
                    }
                    if x.is_none() && y.is_none() && z.is_none() {
                        Some(origin)
                    } else {
                        Some(Vec3::new(
                            x.unwrap_or(origin.x),
                            y.unwrap_or(origin.y),
                            z.unwrap_or(origin.z),
                        ))
                    }
                }
                None => Some(origin),
            };
            push_command(&sound_globals, commands::ScriptCommand::PlaySfx { clip, position });
            Ok(())
        })?,
    )?;

    // viber.sound_loop(id, clip [, {pitch, volume, x, y, z, bus}]) /
    // viber.sound_stop(id). `clip` = nome do registry ou caminho sob
    // sfx_dir; `bus` = "sfx" (default) | "ambience" (vento, chuva, insetos).
    let loop_globals = globals.clone();
    viber.set(
        "sound_loop",
        lua.create_function(move |lua, (id, clip, opts): (String, String, Option<mlua::Table>)| {
            if id.is_empty() || clip.is_empty() {
                return Err(mlua::Error::runtime(
                    "viber.sound_loop: id e clip não podem ser vazios",
                ));
            }
            let mut pitch = 1.0_f64;
            let mut volume = 1.0_f32;
            let mut position = None;
            let mut ambience = false;
            let origin_y = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .and_then(|entity| {
                    loop_globals
                        .read()
                        .ok()
                        .and_then(|g| g.positions.get(&(entity.to_bits() as i64)).copied())
                })
                .map(|o| o.y)
                .unwrap_or(0.0);
            if let Some(opts) = opts {
                if let Ok(value) = opts.get::<f64>("pitch") {
                    if !value.is_finite() || value <= 0.0 {
                        return Err(mlua::Error::runtime(
                            "viber.sound_loop: pitch tem de ser finito e > 0",
                        ));
                    }
                    pitch = value;
                }
                if let Ok(value) = opts.get::<f32>("volume") {
                    if !value.is_finite() || value < 0.0 {
                        return Err(mlua::Error::runtime(
                            "viber.sound_loop: volume tem de ser finito e >= 0",
                        ));
                    }
                    volume = value;
                }
                if let Ok(bus) = opts.get::<String>("bus") {
                    ambience = match bus.as_str() {
                        "ambience" | "ambiente" | "ambient" => true,
                        "sfx" => false,
                        other => {
                            return Err(mlua::Error::runtime(format!(
                                "viber.sound_loop: bus '{other}' inválido (sfx|ambience)"
                            )));
                        }
                    };
                }
                let x = opts.get::<f32>("x").ok();
                let y = opts.get::<f32>("y").ok();
                let z = opts.get::<f32>("z").ok();
                // A mesma regra do `viber.sound`: NaN/inf são ERRO (o resto
                // das primitivas com posição rejeita não-finitos) — aqui
                // caíam em silêncio e o loop tocava sem posição.
                for (axis, value) in [("x", x), ("y", y), ("z", z)] {
                    if value.is_some_and(|v| !v.is_finite()) {
                        return Err(mlua::Error::runtime(format!(
                            "viber.sound_loop: {axis} tem de ser finito (NaN/inf)"
                        )));
                    }
                }
                if let (Some(x), Some(z)) = (x, z) {
                    // Sem `y` = a cota da entidade (0 enterrava o loop nas
                    // colinas — o `viber.sound` já usava a origem).
                    let y = y.unwrap_or(origin_y);
                    position = Some(Vec3::new(x, y, z));
                }
            }
            let owner = lua.app_data_ref::<ScriptEntity>().and_then(|e| e.0);
            push_command(
                &loop_globals,
                commands::ScriptCommand::SoundLoop {
                    id,
                    clip,
                    pitch,
                    volume,
                    position,
                    ambience,
                    owner,
                },
            );
            Ok(())
        })?,
    )?;
    let stop_globals = globals.clone();
    viber.set(
        "sound_stop",
        lua.create_function(move |_, id: String| {
            push_command(&stop_globals, commands::ScriptCommand::SoundStop(id));
            Ok(())
        })?,
    )?;

    // viber.music.layer(nome | nil) — força a layer de BGM. nil devolve a
    // zona/combate automáticos.
    // viber.music.combat(active [, boss]) — a REGRA de quando entra a música
    // de combate é do jogo (a engine só tem o estado e o crossfade).
    let music_globals = globals.clone();
    let music_table = lua.create_table()?;
    music_table.set(
        "layer",
        lua.create_function(move |_, name: Option<String>| {
            push_command(&music_globals, commands::ScriptCommand::MusicLayer(name));
            Ok(())
        })?,
    )?;
    let combat_globals = globals.clone();
    music_table.set(
        "combat",
        lua.create_function(move |_, (active, boss): (bool, Option<bool>)| {
            push_command(
                &combat_globals,
                commands::ScriptCommand::MusicCombat {
                    active,
                    boss: boss.unwrap_or(false),
                },
            );
            Ok(())
        })?,
    )?;
    viber.set("music", music_table)?;

    // viber.audio.* — primitivas de mistura/ambiente (o jogo decide quando).
    let duck_globals = globals.clone();
    let audio_table = lua.create_table()?;
    audio_table.set(
        "duck",
        lua.create_function(move |_, (bus, gain, secs): (String, f32, f32)| {
            if !gain.is_finite() || !secs.is_finite() || secs <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.audio.duck: gain finito e secs > 0",
                ));
            }
            if !commands::is_valid_audio_bus(&bus) {
                return Err(mlua::Error::runtime(format!(
                    "viber.audio.duck: bus '{bus}' inválido (master|music|sfx|voice|ambience)"
                )));
            }
            push_command(
                &duck_globals,
                commands::ScriptCommand::AudioDuck { bus, gain, secs },
            );
            Ok(())
        })?,
    )?;
    let reverb_globals = globals.clone();
    audio_table.set(
        "reverb",
        lua.create_function(move |_, (preset, wet): (String, Option<f32>)| {
            if !commands::is_valid_reverb_preset(&preset) {
                return Err(mlua::Error::runtime(format!(
                    "viber.audio.reverb: preset '{preset}' inválido (off|room|hall|cave)"
                )));
            }
            if let Some(wet) = wet
                && (!wet.is_finite() || !(0.0..=1.0).contains(&wet))
            {
                return Err(mlua::Error::runtime(
                    "viber.audio.reverb: wet tem de estar em 0..=1",
                ));
            }
            push_command(
                &reverb_globals,
                commands::ScriptCommand::AudioReverb { preset, wet },
            );
            Ok(())
        })?,
    )?;
    viber.set("audio", audio_table)?;

    // viber.punch(stops, bloom) — pulso de pós-processo.
    let punch_globals = globals.clone();
    viber.set(
        "punch",
        lua.create_function(move |_, (stops, bloom): (f32, f32)| {
            if !(stops.is_finite() && bloom.is_finite()) {
                return Err(mlua::Error::runtime("viber.punch: valores não finitos"));
            }
            push_command(&punch_globals, commands::ScriptCommand::Punch { stops, bloom });
            Ok(())
        })?,
    )?;

    // viber.kick(x, y, z) — solavanco direcional da câmara.
    let kick_globals = globals.clone();
    viber.set(
        "kick",
        lua.create_function(move |_, (x, y, z): (f32, f32, f32)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime("viber.kick: valores não finitos"));
            }
            push_command(
                &kick_globals,
                commands::ScriptCommand::CameraKick(Vec3::new(x, y, z)),
            );
            Ok(())
        })?,
    )?;

    // viber.lantern(intensity) — acende/reforça a lanterna do viajante (a
    // tocha equipada ilumina mesmo de DIA; 0 = só a lanterna automática
    // nocturna). `intensity` em lúmenes, o mesmo espaço da `LANTERN`.
    let lantern_globals = globals.clone();
    viber.set(
        "lantern",
        lua.create_function(move |_, intensity: f32| {
            if !intensity.is_finite() {
                return Err(mlua::Error::runtime(
                    "viber.lantern: intensidade não finita (NaN/inf)",
                ));
            }
            push_command(
                &lantern_globals,
                commands::ScriptCommand::SetLantern(intensity.max(0.0)),
            );
            Ok(())
        })?,
    )?;

    // viber.bone_offset(bone, rx, ry, rz) — rotação ADITIVA num osso do
    // herói, somada por cima da animação (idle/walk/run) todos os frames.
    // Zeros (identidade) limpam a entrada.
    let bone_globals = globals.clone();
    viber.set(
        "bone_offset",
        lua.create_function(move |_, (bone, rx, ry, rz): (String, f32, f32, f32)| {
            if bone.trim().is_empty() {
                return Err(mlua::Error::runtime("viber.bone_offset: osso vazio"));
            }
            if !rx.is_finite() || !ry.is_finite() || !rz.is_finite() {
                return Err(mlua::Error::runtime(
                    "viber.bone_offset: rotação não finita (NaN/inf)",
                ));
            }
            let identity = rx.abs() < 1e-6 && ry.abs() < 1e-6 && rz.abs() < 1e-6;
            let rotation = if identity {
                Quat::IDENTITY
            } else {
                Quat::from_euler(EulerRot::XYZ, rx, ry, rz)
            };
            push_command(
                &bone_globals,
                commands::ScriptCommand::SetBoneOffset {
                    bone: bone.trim().to_lowercase(),
                    rotation,
                },
            );
            Ok(())
        })?,
    )?;

    // viber.camera_free(ativo) — a câmara de JOGO passa a ser escrita pelo
    // script (`camera_pose`). `false` devolve a pose ao rig de terceira
    // pessoa no frame seguinte.
    let camerafree_globals = globals.clone();
    viber.set(
        "camera_free",
        lua.create_function(move |_, active: bool| {
            push_command(&camerafree_globals, commands::ScriptCommand::CameraFree(active));
            Ok(())
        })?,
    )?;

    // viber.camera_pose(olho_x, olho_y, olho_z, alvo_x, alvo_y, alvo_z) —
    // pose do modo livre. Sem `camera_free(true)` é inerte.
    let camerapose_globals = globals.clone();
    viber.set(
        "camera_pose",
        lua.create_function(
            move |_, (x, y, z, tx, ty, tz): (f32, f32, f32, f32, f32, f32)| {
                if !(x.is_finite()
                    && y.is_finite()
                    && z.is_finite()
                    && tx.is_finite()
                    && ty.is_finite()
                    && tz.is_finite())
                {
                    return Err(mlua::Error::runtime(
                        "viber.camera_pose: coordenadas não finitas (NaN/inf)",
                    ));
                }
                push_command(
                    &camerapose_globals,
                    commands::ScriptCommand::CameraPose {
                        pos: Vec3::new(x, y, z),
                        look: Vec3::new(tx, ty, tz),
                    },
                );
                Ok(())
            },
        )?,
    )?;

    // viber.camera_state() -> {x,y,z,yaw,pitch} — pose VIVA da câmara de
    // jogo (publicada pela integração). `nil` sem câmara.
    let camerastate_globals = globals.clone();
    viber.set(
        "camera_state",
        lua.create_function(move |lua, ()| {
            let Some((pos, yaw, pitch)) = camerastate_globals.read().ok().and_then(|g| g.camera)
            else {
                return Ok(mlua::Value::Nil);
            };
            let table = lua.create_table()?;
            table.raw_set("x", pos.x)?;
            table.raw_set("y", pos.y)?;
            table.raw_set("z", pos.z)?;
            table.raw_set("yaw", yaw)?;
            table.raw_set("pitch", pitch)?;
            Ok(mlua::Value::Table(table))
        })?,
    )?;

    // viber.clip_duration(nome) — duração do clip do herói (s), fuzzy pelo
    // nome normalizado (o `normalize_clip_name` é o contrato do
    // `bevy_ext_core::animation`). O jogo agenda swings/one-shots sem
    // hardcodar o GLB.
    let clip_globals = globals.clone();
    viber.set(
        "clip_duration",
        lua.create_function(move |_, name: String| {
            let key = bevy_ext_core::animation::normalize_clip_name(&name);
            Ok(clip_globals
                .read()
                .ok()
                .and_then(|g| g.clip_durations.get(&key).copied()))
        })?,
    )?;

    // ── Módulos e cenas ─────────────────────────────────────────────────

    // viber.change_scene(id [, entrada]) -> true | false, erro — troca de
    // cena (fade, descarrega a atual, carrega a outra; `entrada` =
    // `<SceneEntry name>`, "@back" = o portal por onde se entrou). Sem
    // registry de cenas no host, a validação de ids conhecidos é do
    // consumidor do comando (a integração responde com o 2.º valor).
    let scene_globals = globals.clone();
    viber.set(
        "change_scene",
        lua.create_function(move |_, (scene, entry): (String, Option<String>)| {
            let id = scene.trim().to_string();
            if id.is_empty() {
                return Ok((false, Some("id de cena vazio".to_string())));
            }
            push_command(
                &scene_globals,
                commands::ScriptCommand::ChangeScene { scene: id, entry },
            );
            Ok((true, None))
        })?,
    )?;

    // viber.interacted(key) -> bool — a tecla foi pressionada NESTE frame.
    // PENDENTE na integração: o teste de foco (`InteractionFocus`, a
    // entidade alvo mais próxima vence) e o gate de `ui_typing` do Viber.
    let interacted_globals = globals.clone();
    viber.set(
        "interacted",
        lua.create_function(move |_, key: String| {
            let code = keys::key_code_from_str(&key)
                .ok_or_else(|| mlua::Error::runtime(format!("tecla desconhecida '{key}'")))?;
            Ok(interacted_globals
                .read()
                .map(|globals| globals.input.keys_pressed.contains(&code))
                .unwrap_or(false))
        })?,
    )?;

    // viber.nearby(raio [, limite]) / viber.player_nearby(raio [, limite])
    // — entidades NOMEADAS à volta da entidade corrente / do HERÓI, mais
    // perto 1.º; `props` são os dados autorais (`prop-*`, resource-*). A
    // PRÓPRIA entidade fica de fora do `nearby` (a distância é 0, era sempre
    // a 1.ª da lista, e um script que escolhe "o mais próximo" interagia
    // consigo). Portado do monólito com os snapshots publicados (F4).
    let nearby_globals = globals.clone();
    viber.set(
        "nearby",
        lua.create_function(
            move |lua, (radius, limit): (f32, Option<usize>)| -> mlua::Result<mlua::Table> {
                build_nearby(lua, &nearby_globals, radius, limit, NearbyCenter::SelfEntity)
            },
        )?,
    )?;
    let playernear_globals = globals.clone();
    viber.set(
        "player_nearby",
        lua.create_function(
            move |lua, (radius, limit): (f32, Option<usize>)| -> mlua::Result<mlua::Table> {
                build_nearby(lua, &playernear_globals, radius, limit, NearbyCenter::Player)
            },
        )?,
    )?;

    // viber.hostiles([raio]) -> { {id, kind, x, y, z, hp, max, distance, …}, … }
    // Criaturas vivas de `enemies/` e `bosses/`, à volta do HERÓI.
    let hostiles_globals = globals.clone();
    viber.set(
        "hostiles",
        lua.create_function(move |lua, radius: Option<f32>| {
            let radius = radius.unwrap_or(2.4);
            if !radius.is_finite() || radius <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.hostiles: raio tem de ser finito e > 0",
                ));
            }
            let out = lua.create_table()?;
            let shared = hostiles_globals.read().ok();
            let Some(player) = shared.as_ref().and_then(|g| g.player) else {
                return Ok(out);
            };
            let mut rows: Vec<&HostileSnap> = shared
                .as_ref()
                .map(|g| g.hostiles.iter().filter(|h| h.pos.distance(player) <= radius).collect())
                .unwrap_or_default();
            rows.sort_by(|a, b| {
                a.pos
                    .distance_squared(player)
                    .total_cmp(&b.pos.distance_squared(player))
            });
            for (i, h) in rows.iter().take(128).enumerate() {
                let row = lua.create_table()?;
                row.raw_set("id", h.id)?;
                row.raw_set("kind", h.kind.clone())?;
                row.raw_set("x", h.pos.x)?;
                row.raw_set("y", h.pos.y)?;
                row.raw_set("z", h.pos.z)?;
                row.raw_set("hp", h.hp)?;
                row.raw_set("max", h.max)?;
                row.raw_set("forward_x", h.forward.x)?;
                row.raw_set("forward_z", h.forward.y)?;
                row.raw_set("distance", h.pos.distance(player))?;
                out.raw_set(i + 1, row)?;
            }
            Ok(out)
        })?,
    )?;

    // viber.ground_below(x, y, z) -> y | nil — a superfície sólida mais alta
    // em ou abaixo de `y` nesta coluna (topo do mundo, piso de gruta, chão
    // de vão). `nil` quando não há terreno sólido abaixo. Seam
    // `SurfaceQueries` publicado pela integração.
    let ground_globals = globals.clone();
    viber.set(
        "ground_below",
        lua.create_function(move |_, (x, y, z): (f32, f32, f32)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.ground_below: argumentos não finitos (NaN/inf)",
                ));
            }
            let surfaces = ground_globals.read().ok().and_then(|g| g.surfaces.clone());
            Ok(surfaces.and_then(|s| s.ground_below(x, y, z)))
        })?,
    )?;

    // viber.on_road(x, z) / viber.in_water(x, z) — consultas ao mundo carvado.
    let road_globals = globals.clone();
    viber.set(
        "on_road",
        lua.create_function(move |_, (x, z): (f32, f32)| {
            Ok(road_globals
                .read()
                .ok()
                .and_then(|g| g.surfaces.clone())
                .is_some_and(|s| s.on_road(x, z)))
        })?,
    )?;
    let water_globals = globals.clone();
    viber.set(
        "in_water",
        lua.create_function(move |_, (x, z): (f32, f32)| {
            Ok(water_globals
                .read()
                .ok()
                .and_then(|g| g.surfaces.clone())
                .is_some_and(|s| s.in_water(x, z)))
        })?,
    )?;

    // viber.road(x, z) -> {y, s, length, half_width, lateral, deck, heading,
    // on, cx, cz} | nil — projeção na estrada mais próxima, mesmo fora da
    // fita. `y` é o tabuleiro quando o ponto está NUM vão suspenso; senão é
    // o terreno. Sem estradas = nil.
    let roadq_globals = globals.clone();
    viber.set(
        "road",
        lua.create_function(move |lua, (x, z): (f32, f32)| {
            if !(x.is_finite() && z.is_finite()) {
                return Ok(mlua::Value::Nil);
            }
            let surfaces = roadq_globals.read().ok().and_then(|g| g.surfaces.clone());
            let Some(surfaces) = surfaces else {
                return Ok(mlua::Value::Nil);
            };
            let Some(sample) = surfaces.nearest_road(x, z) else {
                return Ok(mlua::Value::Nil);
            };
            let ground = surfaces.ground_top(x, z);
            Ok(mlua::Value::Table(road_table(lua, &sample, ground)?))
        })?,
    )?;

    // viber.road_at(s) -> o mesmo formato, no arco `s` (m) da estrada mais
    // longa. Serve o look-ahead da IA e a grelha. Sem estradas = nil.
    let roadat_globals = globals.clone();
    viber.set(
        "road_at",
        lua.create_function(move |lua, s: f32| {
            if !s.is_finite() {
                return Ok(mlua::Value::Nil);
            }
            let surfaces = roadat_globals.read().ok().and_then(|g| g.surfaces.clone());
            let Some(surfaces) = surfaces else {
                return Ok(mlua::Value::Nil);
            };
            let Some(sample) = surfaces.road_at(s) else {
                return Ok(mlua::Value::Nil);
            };
            let ground = surfaces.ground_top(sample.cx, sample.cz);
            let table = road_table(lua, &sample, ground)?;
            table.raw_set("on", true)?;
            table.raw_set("x", sample.cx)?;
            table.raw_set("z", sample.cz)?;
            Ok(mlua::Value::Table(table))
        })?,
    )?;

    // ── F4: pose/props/efeitos de mundo ─────────────────────────────────

    // viber.face_towards(x, z) / viber.face_player() — virar a entidade.
    let face_globals = globals.clone();
    let faceto_globals = globals.clone();
    viber.set(
        "face_towards",
        lua.create_function(move |lua, (x, z): (f32, f32)| {
            if !(x.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.face_towards: coordenadas não finitas (NaN/inf)",
                ));
            }
            let entity = script_entity(lua)?;
            let origin_y = lua
                .app_data_ref::<ScriptEntity>()
                .and_then(|e| e.0)
                .and_then(|entity| {
                    face_globals
                        .read()
                        .ok()
                        .and_then(|g| g.positions.get(&(entity.to_bits() as i64)).copied())
                })
                .map(|o| o.y)
                .unwrap_or_default();
            push_command(
                &faceto_globals,
                commands::ScriptCommand::FaceTowards {
                    entity,
                    target: Vec3::new(x, origin_y, z),
                },
            );
            Ok(())
        })?,
    )?;
    let faceplayer_globals = globals.clone();
    let attach_globals = globals.clone();
    let detach_globals = globals.clone();
    viber.set(
        "face_player",
        lua.create_function(move |lua, ()| {
            let entity = script_entity(lua)?;
            let shared = faceplayer_globals.read().ok();
            let Some(player) = shared.as_ref().and_then(|g| g.player) else {
                return Ok(());
            };
            push_command(
                &faceplayer_globals,
                commands::ScriptCommand::FaceTowards { entity, target: player },
            );
            Ok(())
        })?,
    )?;

    // viber.attach(slot, url [, opts]) / viber.detach(slot [, id]) — prende
    // um GLB a um osso do rig. `url` vazio/nil solta o slot. opts:
    // {bone="hand_r", id, x, y, z, rx, ry, rz (rad), scale}.
    viber.set(
        "attach",
        lua.create_function(
            move |_, (slot, url, opts): (String, Option<String>, Option<mlua::Table>)| {
                if slot.trim().is_empty() {
                    return Err(mlua::Error::runtime("viber.attach: slot vazio"));
                }
                let mut owner = None;
                let mut bone = "hand_r".to_string();
                let mut pos = Vec3::ZERO;
                let mut rot = Vec3::ZERO;
                let mut scale = 1.0f32;
                if let Some(opts) = opts {
                    if let Some(bits) = opts.raw_get::<Option<i64>>("id")? {
                        owner = Some(commands::entity_from_id(bits)?);
                    }
                    if let Some(name) = opts.raw_get::<Option<String>>("bone")? {
                        bone = name;
                    }
                    pos.x = opts.raw_get::<Option<f32>>("x")?.unwrap_or(0.0);
                    pos.y = opts.raw_get::<Option<f32>>("y")?.unwrap_or(0.0);
                    pos.z = opts.raw_get::<Option<f32>>("z")?.unwrap_or(0.0);
                    rot.x = opts.raw_get::<Option<f32>>("rx")?.unwrap_or(0.0);
                    rot.y = opts.raw_get::<Option<f32>>("ry")?.unwrap_or(0.0);
                    rot.z = opts.raw_get::<Option<f32>>("rz")?.unwrap_or(0.0);
                    scale = opts.raw_get::<Option<f32>>("scale")?.unwrap_or(1.0);
                }
                if !scale.is_finite() || scale <= 0.0 {
                    return Err(mlua::Error::runtime(
                        "viber.attach: scale tem de ser finito e > 0",
                    ));
                }
                // Um NaN em pos/rot envenena o `Transform` do prop para
                // sempre (o `f32::clamp` não o recupera). As restantes APIs
                // numéricas já rejeitam não-finitos.
                if !pos.is_finite() || !rot.is_finite() {
                    return Err(mlua::Error::runtime(
                        "viber.attach: x/y/z/rx/ry/rz têm de ser finitos",
                    ));
                }
                let url = url.filter(|u| !u.trim().is_empty());
                push_command(
                    &attach_globals,
                    commands::ScriptCommand::Attach {
                        owner,
                        slot,
                        url,
                        bone,
                        pos,
                        rot,
                        scale,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    viber.set(
        "detach",
        lua.create_function(move |_, (slot, id): (String, Option<i64>)| {
            let owner = match id {
                Some(bits) => Some(commands::entity_from_id(bits)?),
                None => None,
            };
            push_command(
                &detach_globals,
                commands::ScriptCommand::Attach {
                    owner,
                    slot,
                    url: None,
                    bone: String::new(),
                    pos: Vec3::ZERO,
                    rot: Vec3::ZERO,
                    scale: 1.0,
                },
            );
            Ok(())
        })?,
    )?;

    // viber.set_visible(visível [, id]) — esconde/mostra a entidade e a
    // subárvore (a `Visibility` do Bevy propaga aos filhos com Inherited).
        let set_visible_globals = globals.clone();
    viber.set(
        "set_visible",
        lua.create_function(move |lua, (visible, id): (bool, Option<i64>)| {
            let entity = entity::resolve_entity(lua, id)?;
            push_command(
                &set_visible_globals,
                commands::ScriptCommand::SetVisible { entity, visible },
            );
            Ok(())
        })?,
    )?;

    // viber.set_pose(x, y, z, yaw [, id]) — pose LOCAL. Sem id = a própria
    // entidade. A escala mantém-se. Com own_system("locomotion") o herói
    // deixa de andar a pé.
        let set_pose_globals = globals.clone();
    viber.set(
        "set_pose",
        lua.create_function(move |lua, (x, y, z, yaw, id): (f32, f32, f32, f32, Option<i64>)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite() && yaw.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.set_pose: x/y/z/yaw têm de ser finitos",
                ));
            }
            let entity = entity::resolve_entity(lua, id)?;
            push_command(
                &set_pose_globals,
                commands::ScriptCommand::SetPose {
                    entity,
                    pos: Vec3::new(x, y, z),
                    yaw,
                },
            );
            Ok(())
        })?,
    )?;

    // viber.burst(preset, x, y, z [, count]) — preset validado contra a
    // biblioteca publicada (`sparks` é o preset de impacto do melee, fora
    // dela — o mesmo do monólito).
    let burst_globals = globals.clone();
    viber.set(
        "burst",
        lua.create_function(move |_, (preset, x, y, z, count): (String, f32, f32, f32, Option<usize>)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.burst: coordenadas não finitas (NaN/inf)",
                ));
            }
            let known = burst_globals
                .read()
                .ok()
                .map(|g| preset == "sparks" || g.particle_presets.iter().any(|p| p == &preset))
                .unwrap_or(false);
            if !known {
                return Err(mlua::Error::runtime(format!(
                    "viber.burst: preset desconhecido '{preset}'"
                )));
            }
            push_command(
                &burst_globals,
                commands::ScriptCommand::Burst {
                    preset,
                    pos: Vec3::new(x, y, z),
                    count: count.unwrap_or(12),
                },
            );
            Ok(())
        })?,
    )?;

    // viber.decal(url, x, y, z [, {nx, ny, nz, size, life, color}]) — marca
    // de impacto projectada numa superfície (chão/parede/rocha). A textura é
    // do jogo; a engine só desenha e desvanece.
        let decal_globals = globals.clone();
    viber.set(
        "decal",
        lua.create_function(move |_, (url, x, y, z, opts): (String, f32, f32, f32, Option<mlua::Table>)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.decal: coordenadas têm de ser finitas",
                ));
            }
            if url.trim().is_empty() {
                return Err(mlua::Error::runtime("viber.decal: url vazia"));
            }
            let (mut nx, mut ny, mut nz) = (0.0f32, 1.0f32, 0.0f32);
            let mut size = 0.7f32;
            let mut life = 1.5f32;
            let mut color = [1.0f32, 1.0, 1.0];
            if let Some(t) = opts {
                nx = t.raw_get("nx").ok().unwrap_or(nx);
                ny = t.raw_get("ny").ok().unwrap_or(ny);
                nz = t.raw_get("nz").ok().unwrap_or(nz);
                size = t.raw_get("size").ok().unwrap_or(size);
                life = t.raw_get("life").ok().unwrap_or(life);
                if let Ok(Some(hex)) = t.raw_get::<Option<String>>("color") {
                    color = commands::parse_hex_color(&hex).ok_or_else(|| {
                        mlua::Error::runtime(format!("viber.decal: cor inválida '{hex}'"))
                    })?;
                }
            }
            if !(nx.is_finite() && ny.is_finite() && nz.is_finite())
                || Vec3::new(nx, ny, nz).length_squared() < 1e-6
            {
                nx = 0.0;
                ny = 1.0;
                nz = 0.0;
            }
            if !size.is_finite() || size <= 0.0 {
                size = 0.7;
            }
            if !life.is_finite() || life <= 0.0 {
                life = 1.5;
            }
            push_command(
                &decal_globals,
                commands::ScriptCommand::Decal {
                    url,
                    position: Vec3::new(x, y, z),
                    normal: Vec3::new(nx, ny, nz),
                    size,
                    life,
                    color,
                },
            );
            Ok(())
        })?,
    )?;

    // viber.spawn_prototype(nome, x, z [, opts]) — instancia um `<Prototype>`
    // do mundo. opts: {y = cota explícita (default: assenta no terreno),
    // on_spawned = fn(bits)}. O spawn real é pós-frame na integração.
    let spawn_globals = globals.clone();
    viber.set(
        "spawn_prototype",
        lua.create_function(move |lua, (name, x, z, opts): (String, f32, f32, Option<mlua::Table>)| {
            if !(x.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.spawn_prototype: coordenadas não finitas (NaN/inf)",
                ));
            }
            let (y, on_spawned) = match opts {
                Some(t) => (
                    t.raw_get::<f32>("y").ok(),
                    t.raw_get::<mlua::Function>("on_spawned").ok(),
                ),
                None => (None, None),
            };
            if let Some(y) = y
                && !y.is_finite()
            {
                return Err(mlua::Error::runtime(
                    "viber.spawn_prototype: y não finito (NaN/inf)",
                ));
            }
            // Validado à fila: prototype desconhecido é ERRO de script (o
            // transbordo silencioso era o pior caso).
            let known = spawn_globals
                .read()
                .ok()
                .map(|g| g.prototype_names.iter().any(|n| n == &name))
                .unwrap_or(false);
            if !known {
                return Err(mlua::Error::runtime(format!(
                    "viber.spawn_prototype: prototype desconhecido '{name}'"
                )));
            }
            let caller_path = lua
                .app_data_ref::<events::ScriptPath>()
                .map(|p| p.0.clone())
                .filter(|p| !p.is_empty());
            push_command(
                &spawn_globals,
                commands::ScriptCommand::SpawnPrototype {
                    name,
                    pos: Vec3::new(x, y.unwrap_or(0.0), z),
                    seat: y.is_none(),
                    on_spawned,
                    caller_path,
                },
            );
            Ok(())
        })?,
    )?;

    // viber.own_system(nome) / viber.release_system(nome) — reclama/devolve
    // um sistema NATIVO (validado contra a lista congelada).
        let own_system_globals = globals.clone();
    viber.set(
        "own_system",
        lua.create_function(move |_, name: String| {
            if !ownership::KNOWN_SYSTEMS.contains(&name.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "viber.own_system: sistema desconhecido '{name}' (válidos: {})",
                    ownership::KNOWN_SYSTEMS.join(", ")
                )));
            }
            push_command(&own_system_globals, commands::ScriptCommand::OwnSystem(name));
            Ok(())
        })?,
    )?;
        let release_system_globals = globals.clone();
    viber.set(
        "release_system",
        lua.create_function(move |_, name: String| {
            if !ownership::KNOWN_SYSTEMS.contains(&name.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "viber.release_system: sistema desconhecido '{name}' (válidos: {})",
                    ownership::KNOWN_SYSTEMS.join(", ")
                )));
            }
            push_command(&release_system_globals, commands::ScriptCommand::ReleaseSystem(name));
            Ok(())
        })?,
    )?;

    // viber.own_action(nome) — reclama uma ação da UI para Lua: o handler
    // nativo cala e a ação chega via `viber.events()` como ui_action.
        let own_action_globals = globals.clone();
    viber.set(
        "own_action",
        lua.create_function(move |_, name: String| {
            push_command(&own_action_globals, commands::ScriptCommand::OwnUiAction(name));
            Ok(())
        })?,
    )?;

    // viber.hit_flash([id][, segundos]) — acende o flash branco de impacto.
    // Feedback visual genérico; não toca HP nem regras. `id` omitido = a
    // própria entidade (como o resto das vitais).
        let hit_flash_globals = globals.clone();
    viber.set(
        "hit_flash",
        lua.create_function(move |lua, (id, secs): (Option<i64>, Option<f32>)| {
            // HIT_FLASH_SECS do feedback do monólito (0.16 s).
            let secs = secs.unwrap_or(0.16);
            if !secs.is_finite() || secs <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.hit_flash: segundos têm de ser finitos e > 0",
                ));
            }
            let entity = entity::resolve_entity(lua, id)?;
            push_command(
                &hit_flash_globals,
                commands::ScriptCommand::HitFlash {
                    entity,
                    secs: secs.min(2.0),
                },
            );
            Ok(())
        })?,
    )?;

    // viber.topple([id]) — destrutível com queda (`break-style: fall`).
        let topple_globals = globals.clone();
    viber.set(
        "topple",
        lua.create_function(move |lua, id: Option<i64>| {
            let entity = entity::resolve_entity(lua, id)?;
            push_command(&topple_globals, commands::ScriptCommand::Topple { entity });
            Ok(())
        })?,
    )?;

    // viber.corpse([id]) — marca a morte. A engine não agenda despawn nem XP:
    // o jogo chama isto, deixa o ragdoll fazer o resto e decide quando remover
    // o corpo. Sem `id` = a própria entidade.
        let corpse_globals = globals.clone();
    viber.set(
        "corpse",
        lua.create_function(move |lua, id: Option<i64>| {
            let entity = entity::resolve_entity(lua, id)?;
            push_command(&corpse_globals, commands::ScriptCommand::Corpse { entity });
            Ok(())
        })?,
    )?;

    // viber.entity_despawn(id) / viber.despawn_self() — remoção de entidade.
        let entity_despawn_globals = globals.clone();
    viber.set(
        "entity_despawn",
        lua.create_function(move |_, id: i64| {
            let target = commands::entity_from_id(id)?;
            push_command(&entity_despawn_globals, commands::ScriptCommand::Despawn(target));
            Ok(())
        })?,
    )?;
        let despawn_self_globals = globals.clone();
    viber.set(
        "despawn_self",
        lua.create_function(move |lua, ()| {
            let entity = script_entity(lua)?;
            push_command(&despawn_self_globals, commands::ScriptCommand::Despawn(entity));
            Ok(())
        })?,
    )?;

    keys::install(lua, &viber, globals.clone())?;

    let toast_globals = globals.clone();
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
    // Filas de eventos por-path (`viber.events`): path → array de tabelas.
    // A presença da chave É a subscrição (criada na 1.ª chamada); o
    // `fan_out_events` copia a fila da engine para cada VM subscrita.
    events::install(lua, &viber)?;
    lua.set_named_registry_value("viber_events", lua.create_table()?)?;
    // F4: procura por nome (`find`/`find_all`/`entity_name`/`self_id`/
    // `entity_hp`/`set_scale`) e props exportados (`prop`/`props`).
    entity::install(lua, &viber, globals.clone())?;
    props::install(lua, &viber, globals.clone())?;
    timers::install(lua, &viber)?;
    // F8: runtimes IA (sem a feature, `available()`=false e o jogo segue no
    // fluxo antigo — o mesmo contrato do stub do monólito).
    nlu::install(lua, &viber)?;
    decision::install(lua, &viber)?;

    // ── F6 (IA/combate/cinemática) ───────────────────────────────────────

    // viber.entity_damage(amount[, id]) — dano direto (sem i-frames). Negativo
    // era dano ao contrário (a fórmula `dano - armadura` curava em silêncio).
    let damage_globals = globals.clone();
    viber.set(
        "entity_damage",
        lua.create_function(move |lua, (amount, id): (f32, Option<i64>)| {
            if !amount.is_finite() || amount < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.entity_damage: amount tem de ser finito e >= 0 (para curar use entity_heal)",
                ));
            }
            let entity = resolve_target(lua, id, &damage_globals)?;
            push_command(&damage_globals, commands::ScriptCommand::EntityDamage { entity, amount });
            Ok(())
        })?,
    )?;

    // viber.entity_heal(amount[, id]) — o resultado nunca sai de [0, max].
    let heal_globals = globals.clone();
    viber.set(
        "entity_heal",
        lua.create_function(move |lua, (amount, id): (f32, Option<i64>)| {
            if !amount.is_finite() {
                return Err(mlua::Error::runtime(
                    "viber.entity_heal: amount não finito (NaN/inf)",
                ));
            }
            let entity = resolve_target(lua, id, &heal_globals)?;
            push_command(&heal_globals, commands::ScriptCommand::EntityHeal { entity, amount });
            Ok(())
        })?,
    )?;

    // viber.entity_set_max_hp(max[, id]) — cria o `Health` se faltar.
    let setmax_globals = globals.clone();
    viber.set(
        "entity_set_max_hp",
        lua.create_function(move |lua, (max, id): (f32, Option<i64>)| {
            if !max.is_finite() || max <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.entity_set_max_hp: max tem de ser finito e > 0",
                ));
            }
            let entity = resolve_target(lua, id, &setmax_globals)?;
            push_command(&setmax_globals, commands::ScriptCommand::EntitySetMaxHp { entity, max });
            Ok(())
        })?,
    )?;

    // viber.radial_damage(pos, radius, max_damage[, skip_id]) — dano por
    // distância com falloff; a seleção de alvos é da integração (World).
    let radial_globals = globals.clone();
    viber.set(
        "radial_damage",
        lua.create_function(move |lua, (x, y, z, radius, max_damage, skip): (f32, f32, f32, f32, f32, Option<i64>)| {
            let pos = Vec3::new(x, y, z);
            if !pos.is_finite() {
                return Err(mlua::Error::runtime("viber.radial_damage: pos finita"));
            }
            if !radius.is_finite() || radius <= 0.0 {
                return Err(mlua::Error::runtime("viber.radial_damage: radius tem de ser finito e > 0"));
            }
            if !max_damage.is_finite() || max_damage < 0.0 {
                return Err(mlua::Error::runtime("viber.radial_damage: max_damage tem de ser finito e >= 0"));
            }
            let skip = match skip {
                Some(bits) => Some(commands::entity_from_id(bits)?),
                None => script_entity_opt(lua),
            };
            push_command(&radial_globals, commands::ScriptCommand::RadialDamage { pos, radius, max_damage, skip });
            Ok(())
        })?,
    )?;

    // viber.fire_projectile{…} — projétil da engine (linha reta ou arco).
    let proj_globals = globals.clone();
    viber.set(
        "fire_projectile",
        lua.create_function(move |lua, opts: mlua::Table| {
            let from = Vec3::new(
                opts.get::<f32>("x")?,
                opts.get::<f32>("y")?,
                opts.get::<f32>("z")?,
            );
            let dir = Vec3::new(
                opts.get::<f32>("dir_x")?,
                opts.get::<f32>("dir_y")?,
                opts.get::<f32>("dir_z")?,
            );
            if !dir.is_finite() || dir.length_squared() < f32::EPSILON {
                return Err(mlua::Error::runtime("viber.fire_projectile: dir tem de ser finito e não nulo"));
            }
            let speed = opts.get::<f32>("speed")?;
            let damage = opts.get::<f32>("damage")?;
            if !speed.is_finite() || speed <= 0.0 || !damage.is_finite() || damage < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.fire_projectile: speed > 0 e damage >= 0",
                ));
            }
            let faction = opts.get::<String>("faction")?;
            let arc = opts.get::<Option<bool>>("arc")?.unwrap_or(false);
            let shooter = script_entity_opt(lua);
            push_command(&proj_globals, commands::ScriptCommand::FireProjectile {
                from,
                dir: dir.normalize(),
                speed,
                damage,
                faction,
                arc,
                shooter,
            });
            Ok(())
        })?,
    )?;

    // viber.push_player(dir_x, dir_y, dir_z, force) — empurrão no herói.
    let push_globals = globals.clone();
    viber.set(
        "push_player",
        lua.create_function(move |_, (x, y, z, force): (f32, f32, f32, f32)| {
            let dir = Vec3::new(x, y, z);
            if !dir.is_finite() || !force.is_finite() {
                return Err(mlua::Error::runtime("viber.push_player: valores têm de ser finitos"));
            }
            push_command(&push_globals, commands::ScriptCommand::PushPlayer { dir, force });
            Ok(())
        })?,
    )?;

    // viber.entity_knockback(id, dir_x, dir_y, dir_z, force).
    let knock_globals = globals.clone();
    viber.set(
        "entity_knockback",
        lua.create_function(move |_, (id, x, y, z, force): (i64, f32, f32, f32, f32)| {
            let entity = commands::entity_from_id(id)?;
            let dir = Vec3::new(x, y, z);
            if !dir.is_finite() || !force.is_finite() {
                return Err(mlua::Error::runtime("viber.entity_knockback: valores têm de ser finitos"));
            }
            push_command(&knock_globals, commands::ScriptCommand::EntityKnockback { entity, dir, force });
            Ok(())
        })?,
    )?;

    // viber.move_towards(x, y, z[, speed]) / viber.move_by(dx, dy, dz[, speed])
    // — o movimento de script vai para o `AiLocomotion` (snap no terreno).
    let mt_globals = globals.clone();
    viber.set(
        "move_towards",
        lua.create_function(move |lua, (x, y, z, speed): (f32, f32, f32, Option<f32>)| {
            let entity = script_entity(lua)?;
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime("viber.move_towards: alvo não finito"));
            }
            push_command(&mt_globals, commands::ScriptCommand::MoveTowards {
                entity,
                target: Vec3::new(x, y, z),
                speed: speed.unwrap_or(0.0),
            });
            Ok(())
        })?,
    )?;
    let mb_globals = globals.clone();
    viber.set(
        "move_by",
        lua.create_function(move |lua, (dx, dy, dz, speed): (f32, f32, f32, Option<f32>)| {
            let entity = script_entity(lua)?;
            if !(dx.is_finite() && dy.is_finite() && dz.is_finite()) {
                return Err(mlua::Error::runtime("viber.move_by: delta não finito"));
            }
            // Alvo relativo: a posição de início-de-frame da própria
            // entidade (o snapshot que o publisher publica). Sem snapshot
            // (REPL/attach estático) o delta vira alvo absoluto — o erro
            // de fora-de-on_update já apanhou o caso normal.
            let origin = script_entity_opt(lua)
                .and_then(|e| mb_globals.read().ok()?.positions.get(&(e.to_bits() as i64)).copied())
                .unwrap_or_default();
            push_command(&mb_globals, commands::ScriptCommand::MoveTowards {
                entity,
                target: origin + Vec3::new(dx, dy, dz),
                speed: speed.unwrap_or(0.0),
            });
            Ok(())
        })?,
    )?;

    // viber.set_player_speed(m/s) — velocidade base do herói.
    let spd_globals = globals.clone();
    viber.set(
        "set_player_speed",
        lua.create_function(move |_, mps: f32| {
            if !mps.is_finite() || mps < 0.0 {
                return Err(mlua::Error::runtime("viber.set_player_speed: m/s finito e >= 0"));
            }
            push_command(&spd_globals, commands::ScriptCommand::SetPlayerSpeed(mps));
            Ok(())
        })?,
    )?;

    // viber.cinematic(id[, anchor_id]) — toca uma `<Cinematic>` do mundo.
    let cine_globals = globals.clone();
    viber.set(
        "cinematic",
        lua.create_function(move |lua, (id, anchor): (String, Option<i64>)| {
            let anchor = match anchor {
                Some(bits) => Some(commands::entity_from_id(bits)?),
                None => script_entity_opt(lua),
            };
            push_command(&cine_globals, commands::ScriptCommand::PlayCinematic { id, anchor });
            Ok(())
        })?,
    )?;

    // viber.cinematic_playing() -> id | nil
    let cine_playing_globals = globals.clone();
    viber.set(
        "cinematic_playing",
        lua.create_function(move |_, ()| {
            Ok(cine_playing_globals
                .read()
                .ok()
                .and_then(|globals| globals.cinematic.clone()))
        })?,
    )?;

    // viber.sense(x, y, z, loudness) — ruído audível no `SenseBus` (IA).
    let noise_globals = globals.clone();
    viber.set(
        "sense",
        lua.create_function(move |_, (x, y, z, loudness): (f32, f32, f32, f32)| {
            let pos = Vec3::new(x, y, z);
            if !pos.is_finite() || !loudness.is_finite() || loudness < 0.0 {
                return Err(mlua::Error::runtime("viber.sense: valores finitos, loudness >= 0"));
            }
            push_command(&noise_globals, commands::ScriptCommand::Noise { pos, loudness });
            Ok(())
        })?,
    )?;

    // ── F7 (save/mundo) ──────────────────────────────────────────────────

    // viber.save() / viber.save_slot(n) — o jogo decide (autosave/slot).
    let save_globals = globals.clone();
    viber.set(
        "save",
        lua.create_function(move |_, ()| {
            push_command(&save_globals, commands::ScriptCommand::SaveGame(None));
            Ok(())
        })?,
    )?;
    let save_slot_globals = globals.clone();
    viber.set(
        "save_slot",
        lua.create_function(move |_, slot: u32| {
            push_command(&save_slot_globals, commands::ScriptCommand::SaveGame(Some(slot)));
            Ok(())
        })?,
    )?;
    let load_globals = globals.clone();
    viber.set(
        "load_save",
        lua.create_function(move |_, ()| {
            push_command(&load_globals, commands::ScriptCommand::LoadGame(None));
            Ok(())
        })?,
    )?;
    let load_slot_globals = globals.clone();
    viber.set(
        "load_slot",
        lua.create_function(move |_, slot: u32| {
            push_command(&load_slot_globals, commands::ScriptCommand::LoadGame(Some(slot)));
            Ok(())
        })?,
    )?;

    // viber.terrain(kind, x, y, z, radius[, strength]) — edição voxel
    // (raise/lower/flatten/paint), aplicada na `TerrainEditQueue`.
    let terrain_globals = globals.clone();
    viber.set(
        "terrain",
        lua.create_function(move |_, (kind, x, y, z, radius, strength): (String, f32, f32, f32, f32, Option<f32>)| {
            if !matches!(kind.as_str(), "raise" | "lower" | "flatten" | "smooth") {
                return Err(mlua::Error::runtime(format!(
                    "viber.terrain: kind inválido '{kind}' (raise/lower/flatten/smooth)"
                )));
            }
            let pos = Vec3::new(x, y, z);
            if !pos.is_finite() || !radius.is_finite() || radius <= 0.0 {
                return Err(mlua::Error::runtime("viber.terrain: pos finita e radius > 0"));
            }
            push_command(&terrain_globals, commands::ScriptCommand::TerrainEdit {
                kind,
                pos,
                radius,
                strength: strength.unwrap_or(1.0),
            });
            Ok(())
        })?,
    )?;

    // ── F7-b (fala/UI de texto/acessibilidade) ──────────────────────────

    // viber.say(texto[, segs][, orador]) — o jogo apresenta a fala; o
    // orador explícito (bits) existe para callbacks noutro script.
    let say_globals = globals.clone();
    viber.set(
        "say",
        lua.create_function(move |lua, (text, secs, speaker): (String, Option<f32>, Option<i64>)| {
            let secs = secs.unwrap_or(0.0);
            if !secs.is_finite() || secs < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.say: segundos têm de ser finitos e >= 0",
                ));
            }
            let speaker = match speaker {
                Some(bits) => Some(commands::entity_from_id(bits)?),
                None => script_entity_opt(lua),
            };
            push_command(&say_globals, commands::ScriptCommand::Say { text, secs, speaker });
            Ok(())
        })?,
    )?;

    // viber.quit() — AppExit limpo; o save é responsabilidade do jogo.
    let quit_globals = globals.clone();
    viber.set(
        "quit",
        lua.create_function(move |_, ()| {
            push_command(&quit_globals, commands::ScriptCommand::Quit);
            Ok(())
        })?,
    )?;

    // viber.a11y() -> {subtitle_scale, shake, hitstop, interact_hold} — o
    // publisher publica o snapshot; a11y_set enfileira as mudanças.
    let a11y_globals = globals.clone();
    viber.set(
        "a11y",
        lua.create_function(move |lua, ()| {
            let out = lua.create_table()?;
            let (subtitle_scale, shake, hitstop, interact_hold) = a11y_globals
                .read()
                .map(|globals| globals.a11y)
                .unwrap_or_default();
            out.raw_set("subtitle_scale", subtitle_scale)?;
            out.raw_set("shake", shake)?;
            out.raw_set("hitstop", hitstop)?;
            out.raw_set("interact_hold", interact_hold)?;
            Ok(out)
        })?,
    )?;
    let a11y_set_globals = globals.clone();
    viber.set(
        "a11y_set",
        lua.create_function(move |_, opts: mlua::Table| {
            let subtitle_scale = opts.raw_get::<f32>("subtitle_scale").ok();
            if subtitle_scale.is_some_and(|value| !value.is_finite() || value <= 0.0) {
                return Err(mlua::Error::runtime(
                    "viber.a11y_set: subtitle_scale tem de ser finito e > 0",
                ));
            }
            push_command(&a11y_set_globals, commands::ScriptCommand::A11ySet {
                subtitle_scale,
                shake: opts.raw_get::<bool>("shake").ok(),
                hitstop: opts.raw_get::<bool>("hitstop").ok(),
                interact_hold: opts.raw_get::<bool>("interact_hold").ok(),
            });
            Ok(())
        })?,
    )?;

    // viber.net_role() -> "off" | "host" | "join" (VIBER_NET no arranque).
    let net_globals = globals.clone();
    viber.set(
        "net_role",
        lua.create_function(move |_, ()| {
            Ok(net_globals
                .read()
                .ok()
                .and_then(|globals| globals.net_role.clone())
                .unwrap_or_else(|| "off".to_string()))
        })?,
    )?;

    // viber.markers{blips, beacons, waypoint} — o HUD do mundo lê o snapshot
    // publicado; aqui só se valida e enfileira.
    let markers_globals = globals.clone();
    viber.set(
        "markers",
        lua.create_function(move |_, spec: mlua::Table| {
            fn point(row: &mlua::Table) -> Option<Vec3> {
                let x: f32 = row.raw_get("x").ok()?;
                let y: f32 = row.raw_get("y").ok()?;
                let z: f32 = row.raw_get("z").ok()?;
                (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Vec3::new(x, y, z))
            }
            fn points(table: mlua::Table, cap: usize) -> mlua::Result<Vec<Vec3>> {
                let mut out = Vec::new();
                for row in table.sequence_values::<mlua::Table>() {
                    if out.len() >= cap {
                        break;
                    }
                    if let Some(pos) = point(&row?) {
                        out.push(pos);
                    }
                }
                Ok(out)
            }
            let mut blips = Vec::new();
            if let Ok(rows) = spec.raw_get::<mlua::Table>("blips") {
                for row in rows.sequence_values::<mlua::Table>() {
                    if blips.len() >= 32 {
                        break;
                    }
                    let row = row?;
                    let Some(pos) = point(&row) else { continue };
                    let tower: bool = row.raw_get("tower").unwrap_or(false);
                    blips.push((pos, tower));
                }
            }
            let beacons = spec
                .raw_get::<mlua::Table>("beacons")
                .ok()
                .map(|rows| points(rows, 16))
                .transpose()?
                .unwrap_or_default();
            let waypoint = spec
                .raw_get::<mlua::Table>("waypoint")
                .ok()
                .and_then(|row| {
                    let pos = point(&row)?;
                    let label: String = row.raw_get("label").unwrap_or_default();
                    Some((pos, label))
                });
            push_command(&markers_globals, commands::ScriptCommand::SetMarkers { blips, beacons, waypoint });
            Ok(())
        })?,
    )?;

    // viber.prompt(key, label) — prompt de interação de MUNDO; vazio limpa.
    let prompt_globals = globals.clone();
    viber.set(
        "prompt",
        lua.create_function(move |_, (key, label): (Option<String>, Option<String>)| {
            let (key, label) = match (key, label) {
                (Some(key), Some(label)) if !label.trim().is_empty() => (key, label),
                _ => (String::new(), String::new()),
            };
            push_command(&prompt_globals, commands::ScriptCommand::Prompt { key, label });
            Ok(())
        })?,
    )?;

    // viber.localize(texto) — resolve as chaves `@nome` do locale ativo (a
    // tabela é publicada pelo publisher; sem ela devolve o texto cru).
    let localize_globals = globals.clone();
    viber.set(
        "localize",
        lua.create_function(move |_, text: String| {
            let table = localize_globals
                .read()
                .ok()
                .and_then(|globals| globals.locale_table.clone());
            Ok(bevy_ext_core::dialogue::localized(&text, table.as_ref()))
        })?,
    )?;

    // viber.locale.get()/set(lang) — idioma ativo em runtime.
    let locale_get_globals = globals.clone();
    let locale_get = lua.create_function(move |_, ()| {
        Ok(locale_get_globals
            .read()
            .ok()
            .and_then(|globals| globals.locale_lang.clone()))
    })?;
    let locale_set_globals = globals.clone();
    let locale_set = lua.create_function(move |_, lang: String| {
        let lang = lang.trim().to_ascii_lowercase();
        if lang.is_empty()
            || !lang
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(mlua::Error::runtime(
                "viber.locale.set: idioma inválido (use 'pt', 'en', …)",
            ));
        }
        push_command(&locale_set_globals, commands::ScriptCommand::SetLocale(lang));
        Ok(())
    })?;
    let locale_table = lua.create_table()?;
    locale_table.set("get", locale_get)?;
    locale_table.set("set", locale_set)?;
    viber.set("locale", locale_table)?;

    // viber.save_slots() — metadados dos 4 slots para o menu de continuar
    // (o publisher publica `save_slots` como JSON; aqui vira tabela).
    let slots_globals = globals.clone();
    viber.set(
        "save_slots",
        lua.create_function(move |lua, ()| {
            let table = lua.create_table()?;
            let rows = slots_globals
                .read()
                .ok()
                .and_then(|globals| globals.save_slots.clone())
                .unwrap_or_default();
            for (index, row) in rows.iter().enumerate() {
                let item = lua.create_table()?;
                for (key, value) in row {
                    match value {
                        serde_json::Value::String(text) => item.set(key.as_str(), text.as_str())?,
                        serde_json::Value::Number(n) => item.set(key.as_str(), n.as_f64())?,
                        serde_json::Value::Bool(b) => item.set(key.as_str(), *b)?,
                        _ => item.set(key.as_str(), value.to_string())?,
                    }
                }
                table.set(index + 1, item)?;
            }
            Ok(table)
        })?,
    )?;

    // ── sinais e grupos (o par signal/groups do Godot, com ligações como
    // DADO: `connect-<sinal>` do nó, grupos em `groups="…"` publicados) ──

    // viber.group(nome) -> {bits…} — membros publicados do grupo (ordem
    // estável por bits).
    let group_globals = globals.clone();
    viber.set(
        "group",
        lua.create_function(move |lua, name: String| {
            let out = lua.create_table()?;
            let mut bits: Vec<i64> = group_globals
                .read()
                .map(|globals| {
                    globals
                        .groups
                        .iter()
                        .filter(|(_, groups)| groups.iter().any(|g| g == &name))
                        .map(|(bits, _)| *bits)
                        .collect()
                })
                .unwrap_or_default();
            bits.sort_unstable();
            for (i, bits) in bits.iter().enumerate() {
                out.raw_set(i as u32 + 1, *bits)?;
            }
            Ok(out)
        })?,
    )?;

    // viber.in_group(nome[, id]) — o próprio script (ou o id) pertence?
    let in_group_globals = globals.clone();
    viber.set(
        "in_group",
        lua.create_function(move |lua, (name, id): (String, Option<i64>)| {
            let bits = match id {
                Some(bits) => bits,
                None => match script_entity_opt(lua) {
                    Some(entity) => entity.to_bits() as i64,
                    None => return Ok(false),
                },
            };
            Ok(in_group_globals
                .read()
                .map(|globals| {
                    globals
                        .groups
                        .get(&bits)
                        .is_some_and(|groups| groups.iter().any(|g| g == &name))
                })
                .unwrap_or(false))
        })?,
    )?;

    // viber.emit(sinal, …) — entrega aos `connect-<sinal>` do emissor.
    let emit_globals = globals.clone();
    viber.set(
        "emit",
        lua.create_function(move |lua, (signal, args): (String, mlua::MultiValue)| {
            let args: Vec<serde_json::Value> = args
                .into_iter()
                .map(|value| mlua_serde_to_json(value))
                .collect();
            let emitter = script_entity_opt(lua);
            push_command(&emit_globals, commands::ScriptCommand::EmitSignal { emitter, signal, args });
            Ok(())
        })?,
    )?;

    // viber.call_group(grupo, método, …) — chama o método em todos os membros.
    let call_globals = globals.clone();
    viber.set(
        "call_group",
        lua.create_function(move |_, (group, method, args): (String, String, mlua::MultiValue)| {
            let args: Vec<serde_json::Value> = args
                .into_iter()
                .map(|value| mlua_serde_to_json(value))
                .collect();
            push_command(&call_globals, commands::ScriptCommand::CallGroup {
                group,
                method,
                args,
                sender: None,
            });
            Ok(())
        })?,
    )?;

    // viber.sender() — dentro de um handler, quem emitiu (bits); nil fora.
    let sender_globals = globals.clone();
    viber.set(
        "sender",
        lua.create_function(move |lua, ()| {
            match lua
                .named_registry_value::<Option<i64>>("viber_signal_sender")?
            {
                Some(bits) => bits.into_lua(lua),
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )?;

    lua.globals().set("viber", viber)?;
    Ok(())
}

/// Um valor Lua → JSON (o args de sinais/grupos viaja como JSON; a entrega
/// converte de volta no contexto do alvo). Tabelas viram objectos/arrays.
fn mlua_serde_to_json(value: mlua::Value) -> serde_json::Value {
    use mlua::Value as V;
    match value {
        V::Nil => serde_json::Value::Null,
        V::Boolean(b) => serde_json::Value::Bool(b),
        V::Integer(i) => serde_json::Value::from(i),
        V::Number(n) => serde_json::Value::from(n),
        V::String(text) => serde_json::Value::String(text.to_string_lossy().to_string()),
        V::Table(table) => {
            // Sequência 1..n → array; caso contrário objecto.
            let len = table.raw_len();
            if len > 0 {
                let mut list = Vec::with_capacity(len);
                for i in 1..=len {
                    list.push(mlua_serde_to_json(table.raw_get::<mlua::Value>(i).unwrap_or(V::Nil)));
                }
                serde_json::Value::Array(list)
            } else {
                let mut map = serde_json::Map::new();
                for pair in table.pairs::<mlua::Value, mlua::Value>() {
                    let Ok((key, value)) = pair else { continue };
                    let key = match key {
                        V::String(text) => text.to_string_lossy().to_string(),
                        other => format!("{other:?}"),
                    };
                    map.insert(key, mlua_serde_to_json(value));
                }
                serde_json::Value::Object(map)
            }
        }
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

/// Entidade alvo de `entity_*`: `id` explícito OU a entidade do script
/// (erro fora de `on_update` — a semântica do monólito).
fn resolve_target(
    lua: &Lua,
    id: Option<i64>,
    _globals: &Arc<RwLock<ScriptGlobals>>,
) -> mlua::Result<Entity> {
    match id {
        Some(bits) => commands::entity_from_id(bits),
        None => script_entity(lua),
    }
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
        install_viber_api_with(&lua, globals.clone(), store.clone())
            .expect("instala viber");
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

    #[test]
    fn sound_at_player_fila_o_clip_na_posicao_do_heroi() {
        let (lua, globals, _store) = vm();
        globals.write().expect("globals").player = Some(Vec3::new(10.0, 4.0, 20.0));

        lua.load("viber.sound_at_player('notification')")
            .exec()
            .expect("som");
        let queued = {
            let mut g = globals.write().expect("globals");
            g.commands.pop_front().expect("comando")
        };
        match queued {
            commands::ScriptCommand::PlaySfx { clip, position } => {
                assert_eq!(clip, bevy_ext_core::SfxClip::Notification);
                assert_eq!(position, Some(Vec3::new(10.0, 4.0, 20.0)));
            }
            other => panic!("comando inesperado: {other:?}"),
        }

        // Sem herói: som de interface (position None).
        globals.write().expect("globals").player = None;
        lua.load("viber.sound_at_player('growl')").exec().expect("growl");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::PlaySfx { clip, position } => {
                assert_eq!(clip, bevy_ext_core::SfxClip::WolfGrowl);
                assert_eq!(position, None);
            }
            other => panic!("comando inesperado: {other:?}"),
        }

        // Clip desconhecido é ERRO de script (não silêncio).
        let erro: bool = lua
            .load("return not pcall(function() viber.sound_at_player('piano') end)")
            .eval()
            .expect("erro");
        assert!(erro);
    }

    #[test]
    fn play_clip_valida_e_enfila_o_gesto() {
        let (lua, globals, _store) = vm();
        // Sem `id`, a entidade vem do attach do contexto (exit4/F3: o
        // comando drena SEM contexto — a entidade tem de viajar nele).
        let own = Entity::try_from_bits(42u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        lua.load("viber.play_clip('mine', { speed = 1.4 })")
            .exec()
            .expect("clip");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Gesture { entity, name, speed, lock } => {
                assert_eq!(name, "mine");
                assert_eq!((speed, lock), (1.4, false));
                assert_eq!(entity, Some(own));
            }
            other => panic!("comando inesperado: {other:?}"),
        }

        // Com id: os bits viram Entity (e lixo vira erro).
        let e = Entity::try_from_bits(9u64).expect("entidade");
        lua.load(format!("viber.play_clip('chop', {{ id = {}, lock = true }})", e.to_bits() as i64))
            .exec()
            .expect("clip com id");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Gesture { entity, lock, .. } => {
                assert_eq!(entity, Some(e));
                assert!(lock);
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        let erro: bool = lua
            .load("return not pcall(function() viber.play_clip('x', { speed = 0 }) end)")
            .eval()
            .expect("speed inválido");
        assert!(erro);
        let erro: bool = lua
            .load("return not pcall(function() viber.play_clip('x', { id = 0 }) end)")
            .eval()
            .expect("id inválido");
        assert!(erro);
    }

    #[test]
    fn feedback_de_camara_valida_e_enfila() {
        let (lua, globals, _store) = vm();
        lua.load("viber.shake(0.4) viber.fov_kick(-6.5) viber.hit_stop(0.08)")
            .exec()
            .expect("feedback");
        let mut queued = Vec::new();
        {
            let mut g = globals.write().expect("globals");
            while let Some(c) = g.commands.pop_front() {
                queued.push(c);
            }
        }
        assert_eq!(queued.len(), 3);
        assert!(matches!(queued[0], commands::ScriptCommand::Shake(0.4)));
        assert!(matches!(queued[1], commands::ScriptCommand::FovKick(-6.5)));
        assert!(matches!(queued[2], commands::ScriptCommand::HitStop(0.08)));

        // NaN/negativos são erro de script.
        let erro: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.shake(-0.1) end),
                not pcall(function() viber.shake(0.0 / 0.0) end),
                not pcall(function() viber.fov_kick(0.0 / 0.0) end),
                not pcall(function() viber.hit_stop(-1) end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erro.iter().all(|e| *e));
    }

    #[test]
    fn damage_number_cai_acima_do_heroi_por_omissao() {
        let (lua, globals, _store) = vm();
        globals.write().expect("globals").player = Some(Vec3::new(1.0, 2.0, 3.0));
        lua.load("viber.damage_number('-12')")
            .exec()
            .expect("número");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::DamageNumber { text, pos, color } => {
                assert_eq!(text, "-12");
                assert_eq!(pos, Vec3::new(1.0, 3.8, 3.0));
                assert_eq!(color, None);
            }
            other => panic!("comando inesperado: {other:?}"),
        }

        // Cada eixo dado substitui o default; cor valida.
        lua.load(
            "viber.damage_number('+5', { y = 9, color = '#f80' })",
        )
        .exec()
        .expect("com overrides");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::DamageNumber { pos, color, .. } => {
                assert_eq!(pos, Vec3::new(1.0, 9.0, 3.0));
                assert_eq!(color, Some([1.0, 136.0 / 255.0, 0.0]));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        let erro: bool = lua
            .load("return not pcall(function() viber.damage_number('x', { color = 'vermelho' }) end)")
            .eval()
            .expect("cor inválida");
        assert!(erro);
    }

    #[test]
    fn ring_valida_coordenadas_e_cor() {
        let (lua, globals, _store) = vm();
        lua.load("viber.ring(3, 4, 2.5, '#ff0000')")
            .exec()
            .expect("anel");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Ring { x, z, radius, color } => {
                assert_eq!((x, z, radius), (3.0, 4.0, 2.5));
                assert_eq!(color, Some([1.0, 0.0, 0.0]));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        let erro: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.ring(0, 0, 0) end),
                not pcall(function() viber.ring(0.0 / 0.0, 0, 1) end),
                not pcall(function() viber.ring(0, 0, 1, '#12345') end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erro.iter().all(|e| *e));
    }

    #[derive(Resource, Default)]
    struct Seen {
        sfx: usize,
        feedback: usize,
    }

    fn count_seen(
        mut sfx: MessageReader<bevy_ext_core::SfxEvent>,
        mut feedback: MessageReader<ScriptCommandMessage>,
        mut seen: ResMut<Seen>,
    ) {
        seen.sfx += sfx.read().count();
        seen.feedback += feedback.read().count();
    }

    #[test]
    fn drain_roteia_sfx_e_feedback_para_mensagens() {
        // O dreno real corre num App mínimo e lê o store DO PROCESSO —
        // nenhum outro teste enfileira nele (todos usam stores injetados).
        let mut app = App::new();
        app.add_message::<bevy_ext_core::SfxEvent>()
            .add_message::<ScriptCommandMessage>()
            .init_resource::<Seen>()
            .add_systems(Update, (commands::drain_commands, count_seen));

        let lua = Lua::new();
        install_viber_api(&lua).expect("viber do processo");
        lua.load("viber.sound_at_player('coin') viber.shake(1.0)")
            .exec()
            .expect("comandos");
        // 1.º tick: o dreno escreve as mensagens (o leitor inicializa a
        // marca no fim da fila — não vê nada ainda); 2.º tick: o leitor
        // conta.
        app.update();
        app.update();

        let seen = app.world().resource::<Seen>();
        assert_eq!((seen.sfx, seen.feedback), (1, 1));
    }


    struct FakeSurfaces {
        ground: Option<f32>,
        road: Option<crate::surfaces::RoadSample>,
    }
    impl crate::surfaces::SurfaceQueries for FakeSurfaces {
        fn ground_below(&self, _x: f32, y: f32, _z: f32) -> Option<f32> {
            self.ground.filter(|g| g <= &y)
        }
        fn ground_top(&self, _x: f32, _z: f32) -> Option<f32> {
            self.ground
        }
        fn on_road(&self, _x: f32, _z: f32) -> bool {
            self.road.is_some()
        }
        fn in_water(&self, _x: f32, _z: f32) -> bool {
            false
        }
        fn nearest_road(&self, _x: f32, _z: f32) -> Option<crate::surfaces::RoadSample> {
            self.road
        }
        fn road_at(&self, _s: f32) -> Option<crate::surfaces::RoadSample> {
            self.road
        }
    }

    fn sample(s: f32) -> crate::surfaces::RoadSample {
        crate::surfaces::RoadSample {
            s,
            length: 120.0,
            half_width: 3.0,
            lateral: 0.5,
            deck: false,
            deck_y: 8.0,
            heading: 0.3,
            center_dist: 0.5,
            cx: 4.0,
            cz: -2.0,
        }
    }

    #[test]
    fn ground_below_road_e_on_road_lêem_o_trait() {
        let (lua, globals, _store) = vm();
        globals.write().expect("globals").surfaces =
            Some(std::sync::Arc::new(FakeSurfaces {
                ground: Some(4.5),
                road: Some(sample(10.0)),
            }));
        let ground: f32 = lua
            .load("return viber.ground_below(0, 10, 0)")
            .eval()
            .expect("ground abaixo");
        assert_eq!(ground, 4.5);
        let missing: bool = lua
            .load("return viber.ground_below(0, 2, 0) == nil")
            .eval()
            .expect("acima do y dado: nil");
        assert!(missing);
        let road: bool = lua.load("return viber.on_road(0, 0)").eval().expect("on_road");
        assert!(road);
        let water: bool = lua.load("return viber.in_water(0, 0)").eval().expect("in_water");
        assert!(!water);
        let (y, s, length, on): (f32, f32, f32, bool) = lua
            .load("local r = viber.road(1, 1) return r.y, r.s, r.length, r.on")
            .eval()
            .expect("road");
        assert_eq!((y, s, length, on), (4.5, 10.0, 120.0, true));
        let road_at: f32 = lua.load("return viber.road_at(5).s").eval().expect("road_at");
        assert_eq!(road_at, 10.0);

        // Sem superfícies publicadas: nil/false (nunca panica).
        let (lua2, _g2, _s2) = vm();
        let empty: bool = lua2
            .load("return viber.road(0, 0) == nil and not viber.on_road(0, 0)")
            .eval()
            .expect("sem surfaces");
        assert!(empty);
    }

    #[test]
    fn hostiles_ordenam_por_distância_ao_heroi() {
        let (lua, globals, _store) = vm();
        globals.write().expect("globals").player = Some(Vec3::ZERO);
        globals.write().expect("globals").hostiles = vec![
            HostileSnap {
                id: 3,
                kind: "wolf".into(),
                pos: Vec3::new(4.0, 0.0, 0.0),
                hp: 30.0,
                max: 30.0,
                forward: Vec2::new(0.0, 1.0),
            },
            HostileSnap {
                id: 8,
                kind: "wolf".into(),
                pos: Vec3::new(1.0, 0.0, 0.0),
                hp: 10.0,
                max: 30.0,
                forward: Vec2::new(1.0, 0.0),
            },
        ];
        let (count, first_id, first_distance): (usize, i64, f32) = lua
            .load("local h = viber.hostiles(10.0) return #h, h[1].id, h[1].distance")
            .eval()
            .expect("hostiles");
        assert_eq!((count, first_id), (2, 8), "o mais perto 1.º");
        assert!((first_distance - 1.0).abs() < 1e-4);
        // Raio invalida as longe.
        let one: usize = lua.load("return #viber.hostiles(2.0)").eval().expect("raio");
        assert_eq!(one, 1);
    }

    #[test]
    fn find_e_find_all_ordem_estável_e_substring() {
        let (lua, globals, _store) = vm();
        globals.write().expect("globals").named_entities = [
            ("porta_norte".to_string(), vec![9, 5]),
            ("porta_sul".to_string(), vec![7]),
            ("moinho".to_string(), vec![3]),
        ]
        .into_iter()
        .collect();
        let north: i64 = lua.load("return viber.find('porta_norte')").eval().expect("exato");
        assert_eq!(north, 9);
        let substring: i64 = lua.load("return viber.find('porta')").eval().expect("substring 1.ª chave lexicográfica");
        assert_eq!(substring, 9);
        let (a, b, c): (i64, i64, i64) = lua
            .load("local t = viber.find_all('porta') return t[1], t[2], t[3]")
            .eval()
            .expect("ordem estável: nome, depois bits");
        assert_eq!((a, b, c), (9, 5, 7));
        let empty: bool = lua
            .load("return viber.find('') == nil and #viber.find_all('') == 0 and viber.find('nada') == nil")
            .eval()
            .expect("vazio");
        assert!(empty);
    }

    #[test]
    fn attach_valida_e_detach_solta() {
        let (lua, globals, _store) = vm();
        lua.load(
            "viber.attach('lanterna', 'props/lanterna.glb', { bone = 'spine_02', y = 0.2, scale = 1.5 })",
        )
        .exec()
        .expect("attach");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Attach { owner, slot, url, bone, pos, scale, .. } => {
                assert_eq!(owner, None);
                assert_eq!(slot, "lanterna");
                assert_eq!(url.as_deref(), Some("props/lanterna.glb"));
                assert_eq!(bone, "spine_02");
                assert_eq!(pos, Vec3::new(0.0, 0.2, 0.0));
                assert_eq!(scale, 1.5);
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        // scale inválido e slot vazio são erro.
        let erros: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.attach('x', 'y', { scale = 0 }) end),
                not pcall(function() viber.attach('  ', 'y') end),
                not pcall(function() viber.attach('x', 'y', { y = 0/0 }) end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erros.iter().all(|e| *e));
        lua.load("viber.detach('lanterna')").exec().expect("detach");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Attach { slot, url, .. } => {
                assert_eq!(slot, "lanterna");
                assert_eq!(url, None, "url None solta o slot");
            }
            other => panic!("comando inesperado: {other:?}"),
        }
    }

    #[test]
    fn spawn_prototype_valida_contra_a_biblioteca_publicada() {
        let (lua, globals, _store) = vm();
        globals.write().expect("globals").prototype_names = vec!["barril".into()];
        lua.load("viber.spawn_prototype('barril', 5, -3, { y = 1.5 })")
            .exec()
            .expect("spawn");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SpawnPrototype { name, pos, seat, .. } => {
                assert_eq!(name, "barril");
                assert_eq!(pos, Vec3::new(5.0, 1.5, -3.0));
                assert!(!seat, "y explícito não assenta");
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        let erro: bool = lua
            .load("return not pcall(function() viber.spawn_prototype('fantasma', 0, 0) end)")
            .eval()
            .expect("desconhecido");
        assert!(erro);
        // Sem y: seat (assenta no terreno) e NaN rejeitado.
        lua.load("viber.spawn_prototype('barril', 1, 2)").exec().expect("seat");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SpawnPrototype { seat, .. } => assert!(seat),
            other => panic!("comando inesperado: {other:?}"),
        }
        let erro: bool = lua
            .load("return not pcall(function() viber.spawn_prototype('barril', 0/0, 0) end)")
            .eval()
            .expect("NaN");
        assert!(erro);
    }

    #[test]
    fn own_system_valida_a_lista_congelada() {
        let (lua, globals, _store) = vm();
        lua.load("viber.own_system('dialogue') viber.release_system('locomotion')")
            .exec()
            .expect("sistemas");
        let mut queued = Vec::new();
        {
            let mut g = globals.write().expect("globals");
            while let Some(c) = g.commands.pop_front() {
                queued.push(c);
            }
        }
        assert!(matches!(&queued[0], commands::ScriptCommand::OwnSystem(s) if s == "dialogue"));
        assert!(matches!(&queued[1], commands::ScriptCommand::ReleaseSystem(s) if s == "locomotion"));
        let erro: bool = lua
            .load("return not pcall(function() viber.own_system('não-existe') end)")
            .eval()
            .expect("desconhecido");
        assert!(erro);
    }

    #[test]
    fn set_pose_set_scale_set_visible_e_topple_enfileiram() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(12u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        lua.load(
            r#"
              viber.set_pose(1, 2, 3, 0.5)
              viber.set_visible(false)
              viber.set_scale(2.5)
              viber.topple()
              viber.corpse()
              viber.hit_flash()
              viber.entity_despawn(12)
              viber.own_action('buy')
            "#,
        )
        .exec()
        .expect("fila");
        let mut queued = Vec::new();
        {
            let mut g = globals.write().expect("globals");
            while let Some(c) = g.commands.pop_front() {
                queued.push(c);
            }
        }
        assert_eq!(queued.len(), 8);
        assert!(matches!(&queued[0], commands::ScriptCommand::SetPose { entity, pos, yaw } if *entity == own && *pos == Vec3::new(1.0, 2.0, 3.0) && *yaw == 0.5));
        assert!(matches!(&queued[1], commands::ScriptCommand::SetVisible { entity, visible: false } if *entity == own));
        assert!(matches!(&queued[2], commands::ScriptCommand::SetScale { entity, scale: 2.5 } if *entity == own));
        assert!(matches!(&queued[3], commands::ScriptCommand::Topple { entity } if *entity == own));
        assert!(matches!(&queued[4], commands::ScriptCommand::Corpse { entity } if *entity == own));
        assert!(matches!(&queued[5], commands::ScriptCommand::HitFlash { secs, .. } if *secs > 0.0));
        assert!(matches!(&queued[6], commands::ScriptCommand::Despawn(e) if *e == own));
        assert!(matches!(&queued[7], commands::ScriptCommand::OwnUiAction(name) if name == "buy"));
        let _ = &queued;
    }

    #[test]
    fn nucleo_lê_o_snapshot_publicado() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(5u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        {
            let mut g = globals.write().expect("globals");
            g.player = Some(Vec3::new(10.0, 2.0, 6.0));
            g.player_forward = Vec2::new(0.0, 1.0);
            g.player_hp = Some((70.0, 100.0));
            g.player_id = Some(1);
            g.positions.insert(5, Vec3::new(4.0, 2.0, 6.0));
            g.names.insert(5, "moinho".into());
        }
        let (x, y, z): (f32, f32, f32) = lua.load("return viber.position()").eval().expect("position");
        assert_eq!((x, y, z), (4.0, 2.0, 6.0));
        let distance: f32 = lua.load("return viber.distance_to_player()").eval().expect("distance");
        assert!((distance - 6.0).abs() < 1e-4);
        let (fx, fz): (f32, f32) = lua.load("return viber.player_forward()").eval().expect("forward");
        assert_eq!((fx, fz), (0.0, 1.0));
        let (ok, cur, max): (bool, f32, f32) = lua.load("return viber.player_hp()").eval().expect("hp");
        assert_eq!((ok, cur, max), (true, 70.0, 100.0));
        let pid: i64 = lua.load("return viber.player_id()").eval().expect("pid");
        assert_eq!(pid, 1);
        let eid: i64 = lua.load("return viber.entity_id()").eval().expect("eid");
        assert_eq!(eid, 5);
        let name: String = lua.load("return viber.self_name()").eval().expect("nome");
        assert_eq!(name, "moinho");
        // home: sem `home` no estado, cai na posição corrente.
        let (hx, hz): (f32, f32) = lua.load("return viber.home()").eval().expect("home");
        assert_eq!((hx, hz), (4.0, 6.0));
        // Sem entidade no attach: position zeros, set_position erro.
        let (lua2, _g2, _s2) = vm();
        let (x, _y, _z): (f32, f32, f32) = lua2.load("return viber.position()").eval().expect("position zeros");
        assert_eq!(x, 0.0);
        let erro: bool = lua2
            .load("return not pcall(function() viber.set_position(1, 2, 3) end)")
            .eval()
            .expect("erro");
        assert!(erro);
    }

    #[test]
    fn set_position_enfila_com_a_entidade_do_attach() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(8u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        lua.load("viber.set_position(1.5, -2.0, 3.5)").exec().expect("set");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SetPosition { entity, pos } => {
                assert_eq!(entity, own);
                assert_eq!(pos, Vec3::new(1.5, -2.0, 3.5));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        // NaN é erro de script.
        let erro: bool = lua
            .load("return not pcall(function() viber.set_position(0/0, 1, 2) end)")
            .eval()
            .expect("NaN");
        assert!(erro);
    }

    #[test]
    fn log_ring_e_cap() {
        let (lua, globals, _store) = vm();
        for i in 0..260 {
            lua.load(format!("viber.log('linha {i}')")).exec().expect("log");
        }
        let logs = globals.read().expect("globals").logs.clone();
        assert_eq!(logs.len(), 256);
        assert_eq!(logs.front().map(String::as_str), Some("linha 4"), "cap 256 descarta as antigas");
    }

    #[test]
    fn home_lê_o_estado_semeado_pela_integracao() {
        let (lua, _globals, _store) = vm();
        let own = Entity::try_from_bits(3u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        let state: mlua::Table = lua.named_registry_value("viber_state").expect("estado");
        let home = lua.create_table().expect("home");
        home.raw_set("x", 10.5).expect("x");
        home.raw_set("z", -4.0).expect("z");
        state.raw_set("home", home).expect("semear");
        let (x, z): (f32, f32) = lua.load("return viber.home()").eval().expect("home");
        assert_eq!((x, z), (10.5, -4.0));
    }

    #[test]
    fn gesture_enfila_o_gesto_sem_velocidade() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(7u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        lua.load("viber.gesture('foldarms')").exec().expect("gesto");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Gesture { entity, name, speed, lock } => {
                assert_eq!(name, "foldarms");
                assert_eq!((entity, speed, lock), (Some(own), 1.0, false));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        // Fora de on_update (attach estático): erro, como no monólito.
        let (lua2, _g2, _s2) = vm();
        let erro: bool = lua2
            .load("return not pcall(function() viber.gesture('x') end)")
            .eval()
            .expect("erro sem entidade");
        assert!(erro);
    }

    #[test]
    fn set_interaction_valida_tecla_e_alcance_a_fila() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(11u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        lua.load("viber.set_interaction('Minerar', 'j')")
            .exec()
            .expect("interação");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SetInteraction { entity, label, key, range } => {
                assert_eq!((label.as_str(), key.as_str()), ("Minerar", "j"));
                assert_eq!(range, commands::BASE_RANGE_M);
                assert_eq!(entity, Some(own));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        // Range explícito entra; negativo/NaN e tecla lixa são erro.
        lua.load("viber.set_interaction('Abrir', 'e', 6.0)")
            .exec()
            .expect("com range");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SetInteraction { range, .. } => assert_eq!(range, 6.0),
            other => panic!("comando inesperado: {other:?}"),
        }
        let erro: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.set_interaction('X', 'tecla-falsa') end),
                not pcall(function() viber.set_interaction('X', 'e', -1) end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erro.iter().all(|e| *e));
    }

    #[test]
    fn interacted_le_a_tecla_pressionada_do_frame() {
        let (lua, globals, _store) = vm();
        globals
            .write()
            .expect("globals")
            .input
            .keys_pressed
            .insert(KeyCode::KeyE);
        let ok: bool = lua
            .load("return viber.interacted('e') and not viber.interacted('j')")
            .eval()
            .expect("interacted");
        assert!(ok);
        let erro: bool = lua
            .load("return not pcall(function() viber.interacted('meta-1') end)")
            .eval()
            .expect("erro");
        assert!(erro);
    }

    #[test]
    fn sound_valida_clip_e_posiciona_na_origem() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(21u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        globals.write().expect("globals").positions.insert(21, Vec3::new(3.0, 4.0, 5.0));
        lua.load("viber.sound('hit')").exec().expect("som");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::PlaySfx { clip, position } => {
                assert_eq!(clip, bevy_ext_core::SfxClip::Hit);
                assert_eq!(position, Some(Vec3::new(3.0, 4.0, 5.0)), "sem overrides = origem da entidade");
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        // Override parcial: y dado substitui só o y.
        lua.load("viber.sound('hit', { y = 9 })").exec().expect("com y");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::PlaySfx { position, .. } => {
                assert_eq!(position, Some(Vec3::new(3.0, 9.0, 5.0)));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        // NaN e clip desconhecido são ERRO.
        let erros: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.sound('hit', { x = 0/0 }) end),
                not pcall(function() viber.sound('piano') end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erros.iter().all(|e| *e));
    }

    #[test]
    fn sound_loop_valida_opts_e_enfila() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(30u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        globals.write().expect("globals").positions.insert(30, Vec3::new(0.0, 7.0, 0.0));
        lua.load(
            "viber.sound_loop('motor', 'vehicles/engine_rev.ogg', { pitch = 1.5, volume = 0.4, x = 2, z = 8, bus = 'ambience' })",
        )
        .exec()
        .expect("loop");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SoundLoop { id, clip, pitch, volume, position, ambience, owner } => {
                assert_eq!((id.as_str(), clip.as_str(), pitch, volume), ("motor", "vehicles/engine_rev.ogg", 1.5, 0.4));
                assert_eq!(position, Some(Vec3::new(2.0, 7.0, 8.0)), "sem y = cota da entidade");
                assert!(ambience);
                assert_eq!(owner, Some(own));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        let erros: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.sound_loop('', 'x') end),
                not pcall(function() viber.sound_loop('id', 'x', { pitch = 0 }) end),
                not pcall(function() viber.sound_loop('id', 'x', { volume = -1 }) end),
                not pcall(function() viber.sound_loop('id', 'x', { bus = 'master' }) end),
                not pcall(function() viber.sound_loop('id', 'x', { y = 0/0 }) end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erros.iter().all(|e| *e));
        lua.load("viber.sound_stop('motor')").exec().expect("stop");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SoundStop(id) => assert_eq!(id, "motor"),
            other => panic!("comando inesperado: {other:?}"),
        }
    }

    #[test]
    fn music_e_audio_validam_e_enfileiram() {
        let (lua, globals, _store) = vm();
        lua.load(
            "viber.music.layer('boss') viber.music.combat(false) \
             viber.audio.duck('music', 0.3, 2.0) viber.audio.reverb('cave', 0.5) \
             viber.music.layer(nil)",
        )
        .exec()
        .expect("fila de áudio");
        let mut queued = Vec::new();
        {
            let mut g = globals.write().expect("globals");
            while let Some(c) = g.commands.pop_front() {
                queued.push(c);
            }
        }
        assert_eq!(queued.len(), 5);
        assert!(matches!(&queued[0], commands::ScriptCommand::MusicLayer(name) if name.as_deref() == Some("boss")));
        assert!(matches!(&queued[1], commands::ScriptCommand::MusicCombat { active: false, boss: false }));
        assert!(matches!(&queued[2], commands::ScriptCommand::AudioDuck { bus, gain, secs } if bus == "music" && *gain == 0.3 && *secs == 2.0));
        assert!(matches!(&queued[3], commands::ScriptCommand::AudioReverb { preset, wet } if preset == "cave" && *wet == Some(0.5)));
        assert!(matches!(&queued[4], commands::ScriptCommand::MusicLayer(None)));
        // Validações: bus/preset inválidos, wet fora de 0..=1, secs <= 0.
        let erros: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.audio.duck('não-existe', 0.3, 2.0) end),
                not pcall(function() viber.audio.duck('music', 0.3, 0.0) end),
                not pcall(function() viber.audio.reverb('banheiro') end),
                not pcall(function() viber.audio.reverb('cave', 1.5) end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erros.iter().all(|e| *e));
    }

    #[test]
    fn feedback_de_camara_e_lanterna_enfileiram() {
        let (lua, globals, _store) = vm();
        lua.load(
            "viber.punch(0.5, 0.2) viber.kick(1, -2, 3) viber.lantern(12.5) \
             viber.camera_free(true) viber.camera_pose(0, 1, 2, 4, 5, 6) \
             viber.bone_offset('hand_r', 0.1, 0, 0) viber.bone_offset('hand_r', 0, 0, 0)",
        )
        .exec()
        .expect("fila");
        let mut queued = Vec::new();
        {
            let mut g = globals.write().expect("globals");
            while let Some(c) = g.commands.pop_front() {
                queued.push(c);
            }
        }
        assert_eq!(queued.len(), 7);
        assert!(matches!(&queued[0], commands::ScriptCommand::Punch { stops, bloom } if *stops == 0.5 && *bloom == 0.2));
        assert!(matches!(&queued[1], commands::ScriptCommand::CameraKick(v) if *v == Vec3::new(1.0, -2.0, 3.0)));
        assert!(matches!(&queued[2], commands::ScriptCommand::SetLantern(v) if *v == 12.5));
        assert!(matches!(&queued[3], commands::ScriptCommand::CameraFree(true)));
        assert!(matches!(&queued[4], commands::ScriptCommand::CameraPose { pos, look } if *pos == Vec3::new(0.0, 1.0, 2.0) && *look == Vec3::new(4.0, 5.0, 6.0)));
        assert!(matches!(&queued[5], commands::ScriptCommand::SetBoneOffset { bone, rotation } if bone == "hand_r" && *rotation != Quat::IDENTITY));
        assert!(matches!(&queued[6], commands::ScriptCommand::SetBoneOffset { rotation, .. } if *rotation == Quat::IDENTITY), "zeros limpam a entrada");
        // Validações: NaN em tudo.
        let erros: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.punch(0/0, 1) end),
                not pcall(function() viber.kick(0/0, 1, 2) end),
                not pcall(function() viber.lantern(0/0) end),
                not pcall(function() viber.camera_pose(0/0, 1, 2, 3, 4, 5) end),
                not pcall(function() viber.bone_offset('', 0, 0, 0) end),
                not pcall(function() viber.bone_offset('hand', 0/0, 0, 0) end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erros.iter().all(|e| *e));
    }

    #[test]
    fn camera_state_e_clip_duration_lêem_o_snapshot() {
        let (lua, globals, _store) = vm();
        globals.write().expect("globals").camera = Some((Vec3::new(1.0, 5.0, 9.0), 45.0, -10.0));
        globals.write().expect("globals").clip_durations.insert("mine".into(), 0.8);
        let (x, yaw, pitch): (f32, f32, f32) = lua
            .load("local c = viber.camera_state() return c.x, c.yaw, c.pitch")
            .eval()
            .expect("camera");
        assert_eq!((x, yaw, pitch), (1.0, 45.0, -10.0));
        let dur: f32 = lua.load("return viber.clip_duration('Animator3D_Mine')").eval().expect("clip");
        assert!((dur - 0.8).abs() < 1e-5, "normaliza o nome do clip");
        let missing: bool = lua
            .load("return viber.clip_duration('não-existe') == nil")
            .eval()
            .expect("sem clip");
        assert!(missing);
        // Sem câmara publicada: nil.
        let (lua2, _g2, _s2) = vm();
        let none: bool = lua2.load("return viber.camera_state() == nil").eval().expect("sem câmara");
        assert!(none);
    }

    #[test]
    fn nearby_ordena_e_exclui_a_propria_entidade() {
        let (lua, globals, _store) = vm();
        let own = Entity::try_from_bits(42u64).expect("entidade");
        lua.set_app_data(ScriptEntity(Some(own)));
        {
            let mut g = globals.write().expect("globals");
            g.player = Some(Vec3::new(30.0, 0.0, 0.0));
            g.positions.insert(42, Vec3::ZERO);
            g.positions.insert(7, Vec3::new(1.0, 0.0, 0.0));
            g.positions.insert(9, Vec3::new(2.0, 0.0, 0.0));
            g.positions.insert(11, Vec3::new(30.0, 0.0, 0.0));
            g.names.insert(42, "eu".into());
            g.names.insert(7, "arbusto".into());
            g.names.insert(9, "porta".into());
            g.names.insert(11, "longe".into());
            g.props.insert(
                7,
                std::sync::Arc::new(vec![("resource-wood".into(), "5".into())]),
            );
        }
        // nearby centra na PRÓPRIA (posição publicada) e exclui-a.
        let (count, first_id, first_name, props_ok): (usize, i64, String, bool) = lua
            .load(r#"
              local a = viber.nearby(5.0)
              local props = a[1].props
              return #a, a[1].id, a[1].name, props and props["resource-wood"] == "5"
            "#)
            .eval()
            .expect("nearby");
        assert_eq!(count, 2, "exclui a própria e o fora do raio");
        assert_eq!((first_id, first_name.as_str()), (7, "arbusto"));
        assert!(props_ok, "props autorais viajam na linha");

        // player_nearby centra no HERÓI (30 m): só a "longe" (a 30 m do
        // centro, zero de distância) entra; as outras ficam fora.
        let (pcount, pid): (usize, i64) = lua
            .load("local a = viber.player_nearby(5.0) return #a, a[1].id")
            .eval()
            .expect("player_nearby");
        assert_eq!((pcount, pid), (1, 11));

        // Limite capado e raio inválido é erro.
        let capped: usize = lua
            .load("return #viber.nearby(5.0, 1)")
            .eval()
            .expect("limite");
        assert_eq!(capped, 1);
        let erro: bool = lua
            .load("return not pcall(function() viber.nearby(0) end)")
            .eval()
            .expect("raio");
        assert!(erro);

        // Sem posição publicada da própria: nearby vazio (não panica).
        let (lua2, g2, _s2) = vm();
        lua2.set_app_data(ScriptEntity(Some(Entity::try_from_bits(99u64).expect("ent"))));
        let empty: bool = lua2
            .load("return #viber.nearby(5.0) == 0")
            .eval()
            .expect("sem origem");
        assert!(empty);
    }

    #[test]
    fn load_cacheia_modulos_num_env_proprio() {
        let (lua, globals, _store) = vm();
        let dir = std::env::temp_dir().join(format!(
            "vael-luau-load-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("relógio")
                .subsec_nanos()
        ));
        std::fs::create_dir_all(dir.join("lib")).expect("mkdir");
        std::fs::write(
            dir.join("lib/fsm.lua"),
            "local M = {}\nfunction M.estado() return 'wander' end\nreturn M\n",
        )
        .expect("modulo com return");
        std::fs::write(
            dir.join("efeitos.lua"),
            "viber.game().efeitos = true\n",
        )
        .expect("modulo sem return");
        lua.set_app_data(game::ScriptsDir(Some(dir.clone())));

        let nome: String = lua
            .load("return viber.load('lib/fsm.lua').estado()")
            .eval()
            .expect("módulo");
        assert_eq!(nome, "wander");

        // Cache: a MESMA tabela em chamadas seguintes (identidade Lua).
        let same: bool = lua
            .load("return viber.load('lib/fsm.lua') == viber.load('scripts/lib/fsm.lua')")
            .eval()
            .expect("cache com e sem prefixo");
        assert!(same, "prefixo scripts/ resolve para o mesmo módulo");

        // Sem `return`: cacheia `true` e não re-corre (o flag de game()
        // ficou true — ter corrido de novo não mudaria nada, mas o cache
        // devolve bool, não nil).
        let flag: bool = lua
            .load("viber.load('efeitos.lua') return viber.game().efeitos == true")
            .eval()
            .expect("sem return");
        assert!(flag);
        let cached: bool = lua
            .load("return viber.load('efeitos.lua') == true")
            .eval()
            .expect("cacheia true");
        assert!(cached);

        // Fuga de caminho rejeitada; ficheiro inexistente é erro claro.
        let erro: Vec<bool> = lua
            .load(r#"
              return {
                not pcall(function() viber.load('../segredo.lua') end),
                not pcall(function() viber.load('/etc/passwd') end),
                not pcall(function() viber.load('lib/nada.lua') end),
                not pcall(function() viber.load('') end),
              }
            "#)
            .eval()
            .expect("validações");
        assert!(erro.iter().all(|e| *e));

        // Sem scripts_dir publicado: erro (a integração publica o caminho).
        let (lua3, _g3, _s3) = vm();
        lua3.set_app_data(game::ScriptsDir(None));
        let (lua2, _g2, _s2) = vm();
        let erro: bool = lua3
            .load("return not pcall(function() viber.load('lib/fsm.lua') end)")
            .eval()
            .expect("sem dir");
        assert!(erro);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_modulo_nao_clobbera_globals_do_carregador() {
        let (lua, globals, _store) = vm();
        let dir = std::env::temp_dir().join(format!(
            "vael-luau-load-sandbox-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("relógio")
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("grosso.lua"), "achei = 'do modulo'\n").expect("módulo");
        lua.set_app_data(game::ScriptsDir(Some(dir)));

        let isolado: bool = lua
            .load(
                "viber.load('grosso.lua') return achei == nil and viber.game().achei == nil",
            )
            .eval()
            .expect("sandbox");
        assert!(isolado, "o módulo escreveu no env do chamador");
    }

    #[test]
    fn change_scene_enfila_o_pedido() {
        let (lua, globals, _store) = vm();
        let (ok, erro): (bool, bool) = lua
            .load("local ok, err = viber.change_scene('  interiors/taverna ', '@back') return ok, err == nil")
            .eval()
            .expect("troca");
        assert!(ok && erro);
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::ChangeScene { scene, entry } => {
                assert_eq!(scene, "interiors/taverna", "id vem sem espaços");
                assert_eq!(entry.as_deref(), Some("@back"));
            }
            other => panic!("comando inesperado: {other:?}"),
        }

        // Sem entrada e id vazio.
        lua.load("viber.change_scene('main')").exec().expect("sem entrada");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::ChangeScene { scene, entry } => {
                assert_eq!(scene, "main");
                assert_eq!(entry, None);
            }
            other => panic!("comando inesperado: {other:?}"),
        }
        let (ok, msg): (bool, String) = lua
            .load("local ok, err = viber.change_scene('   ') return ok, err")
            .eval()
            .expect("vazio");
        assert!(!ok);
        assert!(!msg.is_empty());
    }

}