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

pub mod commands;
pub mod game;
pub mod keys;
pub mod sfx;

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
    /// Entidades nomeadas deste frame para `viber.nearby`/`player_nearby`.
    pub nearby: Vec<NearbyEnt>,
}

/// Uma entidade NOMEADA do snapshot de percepção (`viber.nearby` /
/// `viber.player_nearby`). A integração publica a lista no
/// `ScriptHostState::nearby` a cada frame; os `props` autorais ficam para
/// essa integração.
#[derive(Debug, Clone)]
pub struct NearbyEnt {
    /// Bits da entidade (o `id` das linhas Lua).
    pub id: i64,
    pub name: String,
    pub pos: Vec3,
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
    nearby: Vec<NearbyEnt>,
    toasts: VecDeque<String>,
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

impl Plugin for VaelScriptingPlugin {
    fn build(&self, app: &mut App) {
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
                (publish_globals, publish_input, game::apply_game_patches),
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
    install_viber_api_with(
        context,
        globals(),
        game::game_store(),
        entity.map(|entity| entity.to_bits() as i64),
    )
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
        globals.nearby.clone_from(&state.nearby);
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
fn push_command(
    globals: &Arc<RwLock<ScriptGlobals>>,
    command: commands::ScriptCommand,
) {
    if let Ok(mut globals) = globals.write() {
        globals.commands.push_back(command);
    }
}

/// Instala a tabela `viber` num contexto Lua. Público para o adaptador do
/// Vael reutilizar a mesma API quando precisar de um contexto de REPL.
pub fn install_viber_api(lua: &Lua) -> mlua::Result<()> {
    install_viber_api_with(lua, globals(), game::game_store(), None)
}

fn install_viber_api_with(
    lua: &Lua,
    globals: Arc<RwLock<ScriptGlobals>>,
    store: Arc<RwLock<game::GameStore>>,
    self_bits: Option<i64>,
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
        lua.create_function(move |_, (name, opts): (String, Option<mlua::Table>)| {
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
                None => None,
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
    // (speed 1.0, sem lock; o `play_clip` é a forma generalizada).
    let gesture_globals = globals.clone();
    viber.set(
        "gesture",
        lua.create_function(move |_, name: String| {
            push_command(
                &gesture_globals,
                commands::ScriptCommand::Gesture {
                    entity: None,
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
            move |_, (label, key, range): (String, String, Option<f32>)| {
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
                push_command(
                    &interaction_globals,
                    commands::ScriptCommand::SetInteraction {
                        entity: None,
                        label,
                        key,
                        range,
                    },
                );
                Ok(())
            },
        )?,
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
    // — entidades NOMEADAS do snapshot publicado, mais perto 1.º.
    // DESVIO: sem `ctx.origin` por script, `nearby` também centra no HERÓI
    // (no Viber centrava na entidade que corria o script); a própria
    // entidade continua de fora do `nearby` (o id vem do attach).
    for (name, exclude_self) in [("nearby", true), ("player_nearby", false)] {
        let nearby_globals = globals.clone();
        let self_bits = self_bits;
        viber.set(
            name,
            lua.create_function(
                move |lua, (radius, limit): (f32, Option<usize>)| {
                    if !radius.is_finite() || radius <= 0.0 {
                        return Err(mlua::Error::runtime(format!(
                            "viber.{name}: raio tem de ser finito e > 0"
                        )));
                    }
                    let Ok(globals) = nearby_globals.read() else {
                        return Ok(lua.create_table()?);
                    };
                    // Sem herói não há centro: tabela vazia (o Viber
                    // devolvia {} sem player também).
                    let Some(center) = globals.player else {
                        return Ok(lua.create_table()?);
                    };
                    // Filtra ANTES de ordenar: o snapshot pode ter milhares
                    // de nomeadas por frame.
                    let mut found: Vec<(f32, NearbyEnt)> = globals
                        .nearby
                        .iter()
                        .filter(|entry| !(exclude_self && Some(entry.id) == self_bits))
                        .filter_map(|entry| {
                            let distance = entry.pos.distance(center);
                            (distance <= radius).then_some((distance, entry.clone()))
                        })
                        .collect();
                    drop(globals);
                    found.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let cap = limit.unwrap_or(16).min(64);
                    let out = lua.create_table()?;
                    for (i, (distance, entry)) in found.into_iter().take(cap).enumerate() {
                        let row = lua.create_table()?;
                        row.raw_set("id", entry.id)?;
                        row.raw_set("name", entry.name.as_str())?;
                        row.raw_set("x", entry.pos.x)?;
                        row.raw_set("y", entry.pos.y)?;
                        row.raw_set("z", entry.pos.z)?;
                        row.raw_set("distance", distance)?;
                        out.raw_set(i + 1, row)?;
                    }
                    Ok(out)
                },
            )?,
        )?;
    }

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
        install_viber_api_with(&lua, globals.clone(), store.clone(), None)
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
        install_viber_api_with(&a, globals.clone(), store.clone(), None).expect("vm a");
        install_viber_api_with(&b, globals, store, None).expect("vm b");

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
        lua.load("viber.play_clip('mine', { speed = 1.4 })")
            .exec()
            .expect("clip");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Gesture { entity, name, speed, lock } => {
                assert_eq!(name, "mine");
                assert_eq!((speed, lock), (1.4, false));
                assert_eq!(entity, None);
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


    #[test]
    fn gesture_enfila_o_gesto_sem_velocidade() {
        let (lua, globals, _store) = vm();
        lua.load("viber.gesture('foldarms')").exec().expect("gesto");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::Gesture { entity, name, speed, lock } => {
                assert_eq!(name, "foldarms");
                assert_eq!((entity, speed, lock), (None, 1.0, false));
            }
            other => panic!("comando inesperado: {other:?}"),
        }
    }

    #[test]
    fn set_interaction_valida_tecla_e_alcance_a_fila() {
        let (lua, globals, _store) = vm();
        lua.load("viber.set_interaction('Minerar', 'j')")
            .exec()
            .expect("interação");
        match globals.write().expect("globals").commands.pop_front().expect("comando") {
            commands::ScriptCommand::SetInteraction { entity, label, key, range } => {
                assert_eq!((label.as_str(), key.as_str()), ("Minerar", "j"));
                assert_eq!(range, commands::BASE_RANGE_M);
                assert_eq!(entity, None);
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
    fn nearby_ordena_e_exclui_a_propria_entidade() {
        let (lua, globals, _store) = vm();
        {
            let mut g = globals.write().expect("globals");
            g.player = Some(Vec3::ZERO);
            let self_bits = 42i64;
            g.nearby = vec![
                NearbyEnt { id: self_bits, name: "eu".into(), pos: Vec3::ZERO },
                NearbyEnt { id: 7, name: "arbusto".into(), pos: Vec3::new(1.0, 0.0, 0.0) },
                NearbyEnt { id: 9, name: "porta".into(), pos: Vec3::new(2.0, 0.0, 0.0) },
                NearbyEnt { id: 11, name: "longe".into(), pos: Vec3::new(30.0, 0.0, 0.0) },
            ];
        }
        // Instala com self = 42 para exercitar a exclusão.
        let lua2 = Lua::new();
        let g2 = Arc::new(RwLock::new(ScriptGlobals::default()));
        let s2 = Arc::new(RwLock::new(game::GameStore::default()));
        {
            let mut shared = globals.read().expect("globals");
            let mut g = g2.write().expect("globals");
            g.player = shared.player;
            g.nearby.clone_from(&shared.nearby);
        }
        install_viber_api_with(&lua2, g2.clone(), s2, Some(42)).expect("instala");

        let (count, first_id, first_name, first_distance, second_id): (
            usize,
            i64,
            String,
            f32,
            i64,
        ) = lua2
            .load(r#"
              local a = viber.nearby(5.0)
              return #a, a[1].id, a[1].name, a[1].distance, a[2].id
            "#)
            .eval()
            .expect("nearby");
        assert_eq!(count, 2, "exclui a própria e o fora do raio");
        assert_eq!((first_id, first_name.as_str()), (7, "arbusto"));
        let (_, second_distance): (i64, f32) = lua2
            .load("local a = viber.nearby(5.0) return a[2].id, a[2].distance")
            .eval()
            .expect("segunda linha");
        assert_eq!(second_id, 9);
        assert!(first_distance < second_distance);

        // player_nearby não exclui nada (não há "própria").
        let count: usize = lua2
            .load("return #viber.player_nearby(5.0)")
            .eval()
            .expect("player_nearby");
        assert_eq!(count, 3);

        // Limite capado e raio inválido é erro.
        let capped: usize = lua2
            .load("return #viber.nearby(5.0, 1)")
            .eval()
            .expect("limite");
        assert_eq!(capped, 1);
        let erro: bool = lua2
            .load("return not pcall(function() viber.nearby(0) end)")
            .eval()
            .expect("raio");
        assert!(erro);

        // Sem herói: tabela vazia em ambos.
        globals.write().expect("globals").player = None;
        let empty: bool = lua
            .load("return #viber.nearby(5.0) == 0 and #viber.player_nearby(5.0) == 0")
            .eval()
            .expect("sem herói");
        assert!(empty);
    }

}