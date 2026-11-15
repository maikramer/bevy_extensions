//! Runtime de debug do Vael: um plugin Bevy que publica `vael-tools/*` no
//! mesmo `bevy_remote` do jogo.
//!
//! O crate não depende do monólito (`bevy_ext_viber_engine`): mede
//! frames/hitches/RSS, lista entidades, injeta teclas/texto/rato e publica a
//! prontidão do carregamento (`BridgeReadiness` — o dono do véu insere o
//! recurso; sem ele o `ping` mantém a forma antiga). As features de jogo
//! (Lua/scene/events) entram depois como adapters, sem arrastar a engine.

use std::collections::VecDeque;

use bevy::input::ButtonInput;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput, NativeKey, NativeKeyCode};
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::remote::http::RemoteHttpPlugin;
use bevy::remote::{BrpError, BrpResult, RemoteMethodSystemId, RemoteMethods, RemotePlugin};
use bevy::time::Virtual;
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_ext_bridge_protocol::{
    METHOD_CLICK, METHOD_FRAME_STATS, METHOD_HITCHES, METHOD_KEY, METHOD_MEMORY, METHOD_MOVE,
    METHOD_PAUSE, METHOD_PING, METHOD_PLAY, METHOD_STEP, METHOD_TEXT, METHOD_TREE,
};
use serde::{Serialize};
use serde_json::{Value, json};

/// Sistemas de input sintético. Ordenado ANTES do `gather_actions` do jogo
/// (`viber::input_map`) para que um `just_pressed` sintético chegue ao
/// `ActionState` no mesmo frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeToolsInput;

/// Janela de frames guardada (~4 s a 60 fps).
pub const FRAME_WINDOW: usize = 240;
/// Hitches recentes guardados.
pub const HITCH_WINDOW: usize = 32;
/// Limiar por omissão de um hitch (ms).
pub const DEFAULT_HITCH_MS: f32 = 50.0;

/// Espelho da prontidão do carregamento. O dono do véu (hoje o monólito via
/// um sistema espelho no Vael; um dia o core) insere e actualiza este
/// recurso; o `ping` publica `ready` + `loading` só quando ele existe — sem
/// ele, o pong mantém a forma antiga (sem prontidão) e o cliente decide
/// (`wait-ready` trata pong sem `ready` como compat, com aviso).
///
/// O JSON segue a forma do `viber.ping` (`loading.status/progress/
/// last_load_secs/timed_out`) para os consumidores não saberem a diferença.
#[derive(Resource, Debug, Clone, Default, Serialize)]
pub struct BridgeReadiness {
    /// O mundo está inteiro (ou a época esgotou o teto).
    pub ready: bool,
    /// `ready` por esgotar o teto de carregamento, não por estar completo.
    pub timed_out: bool,
    /// Linha curta do véu ("a carregar modelos 3/7 (43%)").
    pub status: String,
    /// Progresso mostrado no véu (0..1).
    pub progress: f32,
    /// Duração da última época até ficar pronta (s).
    pub last_load_secs: Option<f32>,
}

/// Escreve `ready` + `loading` no pong quando há espelho. Função pura para
/// testes — o formato tem de coincidir com o `viber.ping` legado.
pub fn merge_readiness(pong: &mut Value, readiness: Option<&BridgeReadiness>) {
    let Some(r) = readiness else {
        return;
    };
    pong["ready"] = json!(r.ready);
    pong["loading"] = json!({
        "status": r.status,
        "progress": r.progress,
        "last_load_secs": r.last_load_secs,
        "timed_out": r.timed_out,
    });
}

#[derive(Resource, Debug, Clone)]
pub struct FrameStats {
    ring: VecDeque<f32>,
    hitches: VecDeque<Hitch>,
    hitch_ms: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Hitch {
    pub frame_ms: f32,
    pub at_ms: u64,
}

impl Default for FrameStats {
    fn default() -> Self {
        let hitch_ms = std::env::var("VAEL_HITCH_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<f32>().ok())
            .filter(|ms| ms.is_finite() && *ms > 0.0)
            .unwrap_or(DEFAULT_HITCH_MS);
        Self {
            ring: VecDeque::with_capacity(FRAME_WINDOW),
            hitches: VecDeque::with_capacity(HITCH_WINDOW),
            hitch_ms,
        }
    }
}

impl FrameStats {
    /// Alimenta a janela. Público para testes headless e para o adapter de
    /// sistemas que queira medir um frame artificial.
    pub fn push(&mut self, frame_secs: f32, at_secs: f64) {
        let frame_ms = (frame_secs * 1000.0).max(0.0);
        if self.ring.len() == FRAME_WINDOW {
            self.ring.pop_front();
        }
        self.ring.push_back(frame_ms);
        if frame_ms >= self.hitch_ms {
            if self.hitches.len() == HITCH_WINDOW {
                self.hitches.pop_front();
            }
            self.hitches.push_back(Hitch {
                frame_ms,
                at_ms: (at_secs * 1000.0).max(0.0) as u64,
            });
        }
    }

    pub fn snapshot(&self) -> Value {
        let mut frames: Vec<f32> = self.ring.iter().copied().collect();
        let len = frames.len();
        if frames.is_empty() {
            return json!({
                "frames": 0,
                "avg_ms": 0.0,
                "min_ms": 0.0,
                "max_ms": 0.0,
                "p95_ms": 0.0,
                "p99_ms": 0.0,
                "fps": 0.0,
                "hitches": self.hitches.iter().cloned().collect::<Vec<_>>(),
                "history": [],
            });
        }
        frames.sort_by(f32::total_cmp);
        let avg = frames.iter().sum::<f32>() / len as f32;
        let p95 = percentile(&frames, 0.95);
        let p99 = percentile(&frames, 0.99);
        json!({
            "frames": len,
            "avg_ms": avg,
            "min_ms": frames[0],
            "max_ms": frames[len - 1],
            "p95_ms": p95,
            "p99_ms": p99,
            "fps": if avg > 0.0 { 1000.0 / avg } else { 0.0 },
            "hitches": self.hitches.iter().cloned().collect::<Vec<_>>(),
            "history": self.ring.iter().copied().collect::<Vec<_>>(),
        })
    }

    pub fn hitches(&self) -> Vec<Hitch> {
        self.hitches.iter().cloned().collect()
    }
}

fn percentile(sorted: &[f32], q: f32) -> f32 {
    let index = ((sorted.len() as f32 - 1.0) * q).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// Plugin do runtime. Pode ser adicionado depois do `build_app`; reusa o
/// `RemotePlugin`/`RemoteHttpPlugin` já instalados pela engine.
pub struct BridgeToolsPlugin {
    pub port: u16,
}

impl Default for BridgeToolsPlugin {
    fn default() -> Self {
        Self {
            port: bevy_ext_bridge_protocol::DEFAULT_TOOLS_PORT,
        }
    }
}

impl Plugin for BridgeToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameStats>()
            .init_resource::<PendingKeys>()
            .init_resource::<PendingReleases>()
            .init_resource::<PendingMouse>()
            .init_resource::<FrameStep>()
            // Depois do input core: se a tecla fosse escrita antes, o
            // `just_pressed` era limpo no mesmo frame e o salto perdia-se.
            .add_systems(
                PreUpdate,
                (flush_pending_keys, flush_pending_mouse)
                    .in_set(BridgeToolsInput)
                    .after(bevy::input::InputSystems),
            )
            .add_systems(Last, (record_frame, finish_step));

        if app.world().get_resource::<RemoteMethods>().is_none() {
            app.add_plugins(RemotePlugin::default());
            app.add_plugins(RemoteHttpPlugin::default().with_port(self.port));
        }
        register_methods(app);
    }
}

fn record_frame(time: Res<Time>, mut stats: ResMut<FrameStats>) {
    stats.push(time.delta_secs(), time.elapsed_secs_f64());
}

fn register_methods(app: &mut App) {
    register(app, METHOD_PING, ping);
    register(app, METHOD_FRAME_STATS, frame_stats);
    register(app, METHOD_HITCHES, hitches);
    register(app, METHOD_MEMORY, memory);
    register(app, METHOD_TREE, tree);
    // Sem `vael-tools/logs`: o ring próprio de logs ainda não existe e um
    // stub `[]` MASCARAVA o `viber.logs` legado (que tem layer real) — o
    // fallback do cliente só dispara em erro, não em lista vazia. Quando o
    // monólito sair, o ring entra aqui via `LogPlugin::custom_layer`
    // (suportado no bevy_log 0.20).
    register(app, METHOD_KEY, input_key);
    register(app, METHOD_TEXT, input_text);
    register(app, METHOD_CLICK, input_click);
    register(app, METHOD_MOVE, input_move);
    register(app, METHOD_STEP, step);
    register(app, METHOD_PLAY, play);
    register(app, METHOD_PAUSE, pause);
}

fn register<M>(
    app: &mut App,
    name: &'static str,
    handler: impl IntoSystem<In<Option<Value>>, BrpResult, M> + 'static,
) where
    M: 'static,
{
    let id = app.world_mut().register_system(handler);
    app.world_mut()
        .resource_mut::<RemoteMethods>()
        .insert(name, RemoteMethodSystemId::Instant(id));
}

fn brp_err(message: impl Into<String>) -> BrpError {
    BrpError {
        code: -32000,
        message: message.into(),
        data: None,
    }
}

fn ping(
    _params: In<Option<Value>>,
    readiness: Option<Res<BridgeReadiness>>,
) -> BrpResult {
    let mut pong = json!({
        "pong": true,
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "runtime": "vael-tools",
    });
    merge_readiness(&mut pong, readiness.as_deref());
    Ok(pong)
}

fn frame_stats(_params: In<Option<Value>>, stats: Res<FrameStats>) -> BrpResult {
    Ok(stats.snapshot())
}

fn hitches(_params: In<Option<Value>>, stats: Res<FrameStats>) -> BrpResult {
    Ok(json!(stats.hitches()))
}

fn memory(_params: In<Option<Value>>) -> BrpResult {
    Ok(memory_snapshot())
}

fn memory_snapshot() -> Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let value = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key)?.split_whitespace().next())
            .and_then(|raw| raw.parse::<u64>().ok())
    };
    json!({
        "rss_kb": value("VmRSS:"),
        "hwm_kb": value("VmHWM:"),
        "threads": value("Threads:"),
    })
}

fn tree(params: In<Option<Value>>, query: Query<(Entity, Option<&Name>)>) -> BrpResult {
    let limit = params
        .0
        .as_ref()
        .and_then(|params| params.get("limit"))
        .and_then(Value::as_u64)
        .unwrap_or(2000) as usize;
    let filter = params
        .0
        .as_ref()
        .and_then(|params| params.get("filter"))
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let mut out = Vec::new();
    let mut truncated = false;
    // O mundo tem >80k entidades (meshes de glTF incluídas); a árvore útil é
    // a das entidades com `Name`. O limite evita um JSON de vários MB.
    for (entity, name) in &query {
        let Some(name) = name else {
            continue;
        };
        if !name_matches(name.as_str(), filter.as_deref()) {
            continue;
        }
        if out.len() >= limit {
            truncated = true;
            break;
        }
        out.push(json!({
            "entity": entity.to_bits(),
            "name": name.as_str(),
        }));
    }
    Ok(json!({ "entities": out, "truncated": truncated }))
}

/// Filtro do `tree`: substring sem maiúsculas (None = tudo). Função pura
/// para testes.
fn name_matches(name: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(filter) => name.to_lowercase().contains(&filter.to_lowercase()),
    }
}

#[derive(Resource, Default)]
struct PendingKeys(Vec<(KeyCode, ButtonState, Option<String>)>);

/// Releases agendados por `hold_ms` (tempo REAL desde o arranque).
#[derive(Resource, Default)]
struct PendingReleases(Vec<(f64, KeyCode, Option<String>)>);

/// Clique sintético em curso: o PRESS à espera de injecção e o botão a
/// libertar no frame seguinte (mesma semântica do engine legado — press +
/// release no MESMO frame eram invisíveis para a UI/picking; o release
/// passa um Update depois). `hold` = arrasto: sem release automático.
#[derive(Resource, Default)]
struct PendingMouse {
    press: Option<(f32, f32, MouseButton)>,
    release: Option<MouseButton>,
    hold: bool,
}

#[derive(Resource, Default)]
struct FrameStep {
    remaining: u32,
}

fn step(
    params: In<Option<Value>>,
    mut step: ResMut<FrameStep>,
    mut time: ResMut<Time<Virtual>>,
) -> BrpResult {
    let frames = params
        .0
        .as_ref()
        .and_then(|params| params.get("frames"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 600) as u32;
    step.remaining = frames;
    time.unpause();
    Ok(json!({ "frames": frames }))
}

fn play(
    _params: In<Option<Value>>,
    mut step: ResMut<FrameStep>,
    mut time: ResMut<Time<Virtual>>,
) -> BrpResult {
    step.remaining = 0;
    time.unpause();
    Ok(json!({ "paused": false }))
}

fn pause(
    _params: In<Option<Value>>,
    mut step: ResMut<FrameStep>,
    mut time: ResMut<Time<Virtual>>,
) -> BrpResult {
    step.remaining = 0;
    time.pause();
    Ok(json!({ "paused": true }))
}

fn finish_step(mut step: ResMut<FrameStep>, mut time: ResMut<Time<Virtual>>) {
    if step.remaining == 0 {
        return;
    }
    step.remaining -= 1;
    if step.remaining == 0 {
        time.pause();
    }
}

fn input_key(
    params: In<Option<Value>>,
    mut pending: ResMut<PendingKeys>,
    mut releases: ResMut<PendingReleases>,
    real: Res<Time<Real>>,
) -> BrpResult {
    let params = params.0.unwrap_or(Value::Null);
    let raw = params
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| brp_err("input.key sem `key`"))?;
    let key: KeyCode = serde_json::from_value(Value::String(raw.to_string()))
        .map_err(|error| brp_err(format!("tecla inválida `{raw}`: {error}")))?;
    let state = match params
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("click")
    {
        "press" => ButtonState::Pressed,
        "release" => ButtonState::Released,
        "click" => ButtonState::Pressed,
        other => return Err(brp_err(format!("estado de tecla inválido: {other}"))),
    };
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string);
    let hold_ms = params
        .get("hold_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(60_000);
    let requested = params
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("click");
    pending.0.push((key, state, text.clone()));
    if hold_ms > 0 && requested != "release" {
        // Segura a tecla: o release fica agendado em tempo REAL, para um
        // `input.key` com `hold_ms` durar o pedido mesmo com o jogo pausado.
        releases
            .0
            .push((real.elapsed_secs_f64() + hold_ms as f64 / 1000.0, key, text));
    } else if requested == "click" {
        pending.0.push((key, ButtonState::Released, text));
    }
    Ok(json!({ "queued": true, "hold_ms": hold_ms }))
}

/// Um keypress sintético do `input.text` (press + release implicados).
#[derive(Debug, Clone, PartialEq, Eq)]
struct TextKey {
    key: KeyCode,
    /// Envolve o press/release num Shift sintético (maiúsculas/símbolos).
    shift: bool,
    /// Texto do evento (preenche `logical_key`/`text`).
    text: Option<String>,
}

/// Converte texto numa sequência de keypresses. Controlo com significado
/// próprio: `\n`/`\r` têm de chegar como `Key::Enter` (a barra de fala
/// submete no Enter — com `text: Some("\n")` o `logical_key` era
/// `Character("\n")` e a barra INSERTAVA o newline) e `\t` como `Key::Tab`.
/// Função pura para testes; o cap de 4096 é do handler.
fn text_key_events(text: &str) -> Vec<TextKey> {
    let mut out = Vec::new();
    for char in text.chars() {
        match char {
            '\n' | '\r' => out.push(TextKey {
                key: KeyCode::Enter,
                shift: false,
                text: None,
            }),
            '\t' => out.push(TextKey {
                key: KeyCode::Tab,
                shift: false,
                text: None,
            }),
            _ => {
                let (key, shift) = keycode_for_char(char);
                out.push(TextKey {
                    key,
                    shift,
                    text: Some(char.to_string()),
                });
            }
        }
    }
    out
}

fn input_text(params: In<Option<Value>>, world: &mut World) -> BrpResult {
    let text = params
        .0
        .as_ref()
        .and_then(|params| params.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| brp_err("input.text sem `text`"))?;
    // Cada char vira 2-4 key events no mesmo frame — texto gigante
    // congelava a engine; rejeitar cedo.
    if text.chars().count() > 4096 {
        return Err(brp_err("text demasiado longo (cap 4096 chars)"));
    }
    for event in text_key_events(text) {
        if event.shift {
            apply_key(world, KeyCode::ShiftLeft, ButtonState::Pressed, None);
        }
        apply_key(world, event.key, ButtonState::Pressed, event.text.clone());
        apply_key(world, event.key, ButtonState::Released, event.text);
        if event.shift {
            apply_key(world, KeyCode::ShiftLeft, ButtonState::Released, None);
        }
    }
    Ok(json!({ "chars": text.chars().count() }))
}

fn input_click(params: In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.0.unwrap_or(Value::Null);
    let x = params
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| brp_err("input.click sem `x`"))? as f32;
    let y = params
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| brp_err("input.click sem `y`"))? as f32;
    let button = params
        .get("button")
        .and_then(Value::as_str)
        .map(|raw| serde_json::from_value::<MouseButton>(json!(raw)))
        .transpose()
        .map_err(|error| brp_err(format!("botão inválido: {error}")))?
        .unwrap_or(MouseButton::Left);
    let state = params
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("click");
    if !matches!(state, "click" | "press" | "release") {
        return Err(brp_err(format!(
            "estado de rato inválido: {state} — esperado click | press | release"
        )));
    }
    if state == "release" {
        send_cursor(world, Vec2::new(x, y));
    }
    {
        // Sistema exclusivo (`&mut World` não combina com `ResMut`): o
        // recurso vai buscado à mão, como no handler legado.
        let mut pending = world
            .get_resource_mut::<PendingMouse>()
            .ok_or_else(|| brp_err("PendingMouse ausente (plugin não inicializado?)"))?;
        match state {
            // O clique INTEIRO é injectado no PreUpdate do frame seguinte
            // (ver `flush_pending_mouse`). `press` = arrasto: sem release
            // automático.
            "click" | "press" => {
                pending.press = Some((x, y, button));
                pending.hold = state == "press";
            }
            // Release que chega antes de o press pendente ser injectado:
            // converter o par num clique (ver legado).
            "release" => {
                if pending.press.is_some() {
                    pending.hold = false;
                }
                pending.release = Some(button);
            }
            _ => unreachable!("validado acima"),
        }
    }
    Ok(json!({ "x": x, "y": y, "button": format!("{button:?}"), "state": state }))
}

fn input_move(params: In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.0.unwrap_or(Value::Null);
    let x = params
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| brp_err("input.move sem `x`"))? as f32;
    let y = params
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| brp_err("input.move sem `y`"))? as f32;
    send_cursor(world, Vec2::new(x, y));
    Ok(json!({ "x": x, "y": y }))
}

/// Escreve a posição do cursor: `WindowEvent` (o bevy_picking lê este) +
/// `CursorMoved` + a posição CURRÍCULA da window (o hover da bevy_ui lê
/// `Window::cursor_position`; sem isto um clique sintético nunca "estava"
/// sobre o nó). EFEITO SECUNDÁRIO conhecido: o Wayland recusa o
/// `set_cursor_position` com 1 ERROR por clique — ruído de QA documentado.
fn send_cursor(world: &mut World, position: Vec2) {
    let entity = primary_window(world).unwrap_or(Entity::PLACEHOLDER);
    let moved = CursorMoved {
        window: entity,
        position,
        delta: None,
    };
    if let Some(mut messages) = world.get_resource_mut::<Messages<bevy::window::WindowEvent>>() {
        messages.write(bevy::window::WindowEvent::CursorMoved(moved.clone()));
    }
    if let Some(mut messages) = world.get_resource_mut::<Messages<CursorMoved>>() {
        messages.write(moved);
    }
    if let Some(mut window) = world.get_mut::<Window>(entity) {
        window.set_cursor_position(Some(position));
    }
}

fn primary_window(world: &mut World) -> Option<Entity> {
    let mut query = world.query_filtered::<Entity, With<PrimaryWindow>>();
    query.iter(world).next()
}

/// KeyCode + shift para um char (alfanumérico, espaço e símbolos comuns;
/// restantes chegam como `Unidentified` com o campo `text` preenchido — os
/// consumidores de texto leem `logical_key`/`text`, não o `key_code`).
/// Espelho do mapa do engine legado. Função pura para testes.
fn keycode_for_char(char: char) -> (KeyCode, bool) {
    if let Some(digit) = char.to_digit(10) {
        let variant = match digit {
            0 => KeyCode::Digit0,
            1 => KeyCode::Digit1,
            2 => KeyCode::Digit2,
            3 => KeyCode::Digit3,
            4 => KeyCode::Digit4,
            5 => KeyCode::Digit5,
            6 => KeyCode::Digit6,
            7 => KeyCode::Digit7,
            8 => KeyCode::Digit8,
            _ => KeyCode::Digit9,
        };
        return (variant, false);
    }
    if char.is_ascii_uppercase() {
        return (letter_keycode(char.to_ascii_lowercase()), true);
    }
    if char.is_ascii_lowercase() {
        return (letter_keycode(char), false);
    }
    if char == ' ' {
        return (KeyCode::Space, false);
    }
    match char {
        '!' => (KeyCode::Digit1, true),
        '@' => (KeyCode::Digit2, true),
        '#' => (KeyCode::Digit3, true),
        '$' => (KeyCode::Digit4, true),
        '%' => (KeyCode::Digit5, true),
        '^' => (KeyCode::Digit6, true),
        '&' => (KeyCode::Digit7, true),
        '*' => (KeyCode::Digit8, true),
        '(' => (KeyCode::Digit9, true),
        ')' => (KeyCode::Digit0, true),
        '-' => (KeyCode::Minus, false),
        '_' => (KeyCode::Minus, true),
        '=' => (KeyCode::Equal, false),
        '+' => (KeyCode::Equal, true),
        '.' => (KeyCode::Period, false),
        ',' => (KeyCode::Comma, false),
        '/' => (KeyCode::Slash, false),
        ';' => (KeyCode::Semicolon, false),
        '\'' => (KeyCode::Quote, false),
        _ => (
            KeyCode::Unidentified(NativeKeyCode::Unidentified),
            false,
        ),
    }
}

fn letter_keycode(char: char) -> KeyCode {
    match char {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        _ => KeyCode::KeyZ,
    }
}

fn flush_pending_keys(world: &mut World) {
    let now = world
        .get_resource::<Time<Real>>()
        .map(|time| time.elapsed_secs_f64())
        .unwrap_or(0.0);
    let mut pending = match world.get_resource_mut::<PendingKeys>() {
        Some(mut pending) => std::mem::take(&mut pending.0),
        None => return,
    };
    if let Some(mut releases) = world.get_resource_mut::<PendingReleases>() {
        let mut later = Vec::with_capacity(releases.0.len());
        for (at, key, text) in releases.0.drain(..) {
            if at <= now {
                pending.push((key, ButtonState::Released, text));
            } else {
                later.push((at, key, text));
            }
        }
        releases.0 = later;
    }
    for (key, state, text) in pending {
        apply_key(world, key, state, text);
    }
}

/// Injecção do clique sintético (press neste frame, release no seguinte).
/// Mesma defesa do engine legado: sem input/mensagens (app de teste) não há
/// onde clicar — descarta em silêncio; sem botão EM BAIXO não há release
/// (um release órfão injectado era `just_released` fantasma na UI).
fn flush_pending_mouse(
    mut pending: ResMut<PendingMouse>,
    input: Option<ResMut<ButtonInput<MouseButton>>>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    messages: Option<MessageWriter<MouseButtonInput>>,
    cursor_moved: Option<MessageWriter<CursorMoved>>,
    mut window_events: Option<MessageWriter<bevy::window::WindowEvent>>,
) {
    let (Some(mut input), Some(mut messages)) = (input, messages) else {
        pending.press = None;
        pending.release = None;
        return;
    };
    if let Some((x, y, button)) = pending.press.take() {
        // Release atrasado do clique ANTERIOR sai primeiro: um único slot de
        // release — cliques rápidos em sequência deixavam o botão antigo
        // preso. Só solta o que está mesmo em baixo.
        if let Some(previous) = pending.release.take()
            && input.pressed(previous)
        {
            let entity = windows.single().map(|(e, _)| e).unwrap_or(Entity::PLACEHOLDER);
            let released = MouseButtonInput {
                button: previous,
                state: ButtonState::Released,
                window: entity,
            };
            if let Some(events) = window_events.as_mut() {
                events.write(bevy::window::WindowEvent::MouseButtonInput(released));
            }
            messages.write(released);
            input.release(previous);
        }
        let entity = windows
            .single_mut()
            .map(|(entity, mut window)| {
                // A posição CURRÍCULA da window é o que o hover da bevy_ui
                // lê; sem isto um clique sintético nunca "estava" sobre o nó.
                window.set_cursor_position(Some(Vec2::new(x, y)));
                entity
            })
            .unwrap_or(Entity::PLACEHOLDER);
        let moved = CursorMoved {
            window: entity,
            position: Vec2::new(x, y),
            delta: None,
        };
        let pressed = MouseButtonInput {
            button,
            state: ButtonState::Pressed,
            window: entity,
        };
        // O picking lê `WindowEvent`, não `CursorMoved` — o winit escreve os
        // dois.
        if let Some(events) = window_events.as_mut() {
            events.write(bevy::window::WindowEvent::CursorMoved(moved.clone()));
            events.write(bevy::window::WindowEvent::MouseButtonInput(pressed));
        }
        if let Some(mut cursor_moved) = cursor_moved {
            cursor_moved.write(moved);
        }
        messages.write(pressed);
        input.press(button);
        if std::mem::take(&mut pending.hold) {
            // arrasto: o botão fica em baixo até ao `release` explícito
        } else {
            pending.release = Some(button);
        }
        return;
    }
    if let Some(button) = pending.release.take()
        && input.pressed(button)
    {
        let entity = windows.single().map(|(e, _)| e).unwrap_or(Entity::PLACEHOLDER);
        let released = MouseButtonInput {
            button,
            state: ButtonState::Released,
            window: entity,
        };
        if let Some(events) = window_events.as_mut() {
            events.write(bevy::window::WindowEvent::MouseButtonInput(released));
        }
        messages.write(released);
        input.release(button);
    }
}

fn apply_key(world: &mut World, key: KeyCode, state: ButtonState, text: Option<String>) {
    if let Some(mut input) = world.get_resource_mut::<ButtonInput<KeyCode>>() {
        match state {
            ButtonState::Pressed => input.press(key),
            ButtonState::Released => input.release(key),
        }
    }
    if let Some(mut messages) = world.get_resource_mut::<Messages<KeyboardInput>>() {
        messages.write(KeyboardInput {
            key_code: key,
            logical_key: logical_key_for(key, text.as_deref()),
            state,
            text: text.map(Into::into),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }
}

/// O `logical_key` é o que os consumidores de UI lêem (`blur_ui_inputs_on_enter`
/// despacha por `Key::Enter|Escape`; o `TextInput` do Bevy por
/// `Key::Character`/`Backspace`) — só o `key_code` não chega. Um Enter sem
/// texto chegava como `Unidentified` e a barra de fala nunca submetia; o
/// engine legado (`bevy_ext_viber_engine::bridge`) tem o mesmo mapa por um bug
/// idêntico do `vael-tools debug key backspace`.
fn logical_key_for(key: KeyCode, text: Option<&str>) -> Key {
    if let Some(text) = text {
        return Key::Character(text.into());
    }
    match key {
        KeyCode::Space => Key::Space,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::CapsLock => Key::CapsLock,
        KeyCode::NumLock => Key::NumLock,
        KeyCode::ScrollLock => Key::ScrollLock,
        KeyCode::Pause => Key::Pause,
        KeyCode::ContextMenu => Key::ContextMenu,
        KeyCode::PrintScreen => Key::PrintScreen,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::SuperLeft | KeyCode::SuperRight => Key::Super,
        _ => Key::Unidentified(NativeKey::Unidentified),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_key_maps_ui_keys_without_text() {
        // A barra de fala submete no Enter e apaga no Backspace: sem o mapa
        // chegam como `Unidentified` e o `<UiInput>` ignora-os.
        assert_eq!(logical_key_for(KeyCode::Enter, None), Key::Enter);
        assert_eq!(logical_key_for(KeyCode::NumpadEnter, None), Key::Enter);
        assert_eq!(logical_key_for(KeyCode::Escape, None), Key::Escape);
        assert_eq!(logical_key_for(KeyCode::Backspace, None), Key::Backspace);
        assert_eq!(logical_key_for(KeyCode::Space, None), Key::Space);
        assert_eq!(logical_key_for(KeyCode::ArrowLeft, None), Key::ArrowLeft);
        // Com texto, o caracter manda (é o caminho do `input.text`).
        assert_eq!(
            logical_key_for(KeyCode::KeyP, Some("p")),
            Key::Character("p".into())
        );
        // Teclas sem representação mantêm o comportamento de antes.
        assert_eq!(
            logical_key_for(KeyCode::F5, None),
            Key::Unidentified(NativeKey::Unidentified)
        );
    }

    #[test]
    fn frame_stats_compute_percentiles_and_hitches() {
        let mut stats = FrameStats::default();
        for i in 0..100 {
            stats.push(0.016 + i as f32 * 0.0001, i as f64 * 0.016);
        }
        stats.push(0.120, 2.0);
        let snapshot = stats.snapshot();
        assert_eq!(snapshot["frames"], 101);
        assert!(snapshot["p95_ms"].as_f64().unwrap() > 16.0);
        assert_eq!(snapshot["hitches"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["hitches"][0]["frame_ms"], 120.0);
    }

    #[test]
    fn frame_stats_empty_is_zeroed() {
        let snapshot = FrameStats::default().snapshot();
        assert_eq!(snapshot["frames"], 0);
        assert_eq!(snapshot["fps"], 0.0);
    }

    #[test]
    fn memory_reads_self_status() {
        let value = memory_snapshot();
        assert!(value["rss_kb"].as_u64().unwrap_or(0) > 0);
        assert!(value["threads"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn merge_readiness_publishes_ready_and_loading() {
        // Forma igual à do `viber.ping` legado: `/ready` + `/loading/*`.
        let mut pong = json!({ "pong": true, "runtime": "vael-tools" });
        merge_readiness(
            &mut pong,
            Some(&BridgeReadiness {
                ready: false,
                timed_out: false,
                status: "a carregar modelos 3/7 (43%)".into(),
                progress: 0.43,
                last_load_secs: Some(12.5),
            }),
        );
        assert_eq!(pong["ready"], false);
        assert_eq!(pong["loading"]["status"], "a carregar modelos 3/7 (43%)");
        let progress = pong["loading"]["progress"].as_f64().unwrap();
        assert!((progress - 0.43).abs() < 1e-6);
        assert_eq!(pong["loading"]["last_load_secs"], 12.5);
        assert_eq!(pong["loading"]["timed_out"], false);
    }

    #[test]
    fn merge_readiness_without_mirror_keeps_pong() {
        // Sem espelho (apps sem o recurso) o pong mantém a forma antiga — o
        // `wait-ready` do CLI trata pong sem `ready` como compat.
        let mut pong = json!({ "pong": true, "runtime": "vael-tools" });
        merge_readiness(&mut pong, None);
        assert!(pong.get("ready").is_none());
        assert!(pong.get("loading").is_none());
    }

    #[test]
    fn tree_filter_matches_substring_case_insensitive() {
        assert!(name_matches("Fogueira da praça", None));
        assert!(name_matches("Fogueira da praça", Some("fogueira")));
        assert!(name_matches("chest_iron_01", Some("IRON")));
        assert!(!name_matches("Fogueira", Some("baú")));
    }

    #[test]
    fn text_key_events_map_control_chars_to_enter_and_tab() {
        let events = text_key_events("oi\n");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].key, KeyCode::KeyO);
        assert_eq!(events[0].text.as_deref(), Some("o"));
        // O Enter vem SEM texto: `logical_key` = `Key::Enter` (a barra de
        // fala submete) — com `Character("\n")` a barra inseria o newline.
        assert_eq!(events[2].key, KeyCode::Enter);
        assert_eq!(events[2].text, None);
        assert_eq!(text_key_events("a\tb")[1].key, KeyCode::Tab);
    }

    #[test]
    fn text_key_events_map_case_and_symbols() {
        let events = text_key_events("A!3");
        assert_eq!(events[0].key, KeyCode::KeyA);
        assert!(events[0].shift, "maiúscula pede Shift sintético");
        assert_eq!(events[1].key, KeyCode::Digit1);
        assert!(events[1].shift);
        assert_eq!(events[2].key, KeyCode::Digit3);
        assert!(!events[2].shift);
        // Espaço e símbolos simples.
        assert_eq!(keycode_for_char(' ').0, KeyCode::Space);
        assert_eq!(keycode_for_char('-').0, KeyCode::Minus);
        assert_eq!(keycode_for_char('?').0, KeyCode::Unidentified(NativeKeyCode::Unidentified));
    }
}
