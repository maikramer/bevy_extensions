//! Shared contracts for the `bevy_*` engine packages.
//!
//! This crate is the bottom of the dependency graph: it has no engine services
//! and no game types. Packages depend on it instead of reaching into each
//! other for a marker, a schedule set or a resource.
//!
//! The types here were moved out of the `viber` monolith in Phase 1; the root
//! re-exports them so existing `viber::…` paths keep working.

pub mod a11y;
pub mod animation;
pub mod camera;
pub mod dialogue;
pub mod input_map;
pub mod locale;
pub mod rng;
pub mod script_kind;
pub mod vitals;

pub use rng::Rng;
pub use script_kind::{KILL_XP, kill_xp, script_kind};
pub use vitals::{DEFAULT_HEALTH, Health, Hostile};

use bevy::math::Vec3;
use bevy::prelude::*;

/// Marker for the entity driven by a player controller.
///
/// Engine services (terrain streaming, navigation, interaction) should query
/// this marker instead of the concrete controller component. A game attaches
/// it to whichever entity owns its controller; the Viber runtime attaches it
/// to `player::Player` and the Vael attaches it to its own controller.
#[derive(Debug, Default, Clone, Copy, Component, Reflect)]
#[reflect(Component, Default)]
// Congela o TypePath histórico (o crate mudou de nome para bevy_ext_core).
#[type_path = "bevy_engine_core"]
pub struct PlayerBody;

/// UI/menu state that suppresses movement and gameplay input.
///
/// A game can extend the meaning of each flag; the engine only reads
/// [`Self::any`] to gate input and gameplay systems.
#[derive(Debug, Clone, Default, Resource, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::menus"]
pub struct MenusOpen {
    pub modal: bool,
    pub shop: bool,
    /// Full-screen travel/fast-travel panel.
    pub travel: bool,
    /// Regional map.
    pub map: bool,
    /// A cinematic is playing.
    pub cinematic: bool,
}

impl MenusOpen {
    pub fn any(&self) -> bool {
        self.modal || self.shop || self.travel || self.map || self.cinematic
    }
}

/// Para quê a App foi montada: o jogo de si próprio, ou o editor
/// in-process (cena congelada, portais e trocas de cena silenciados).
///
/// Nascido em `viber::app`; o host e o jogo (composição própria em
/// `src/app.rs` do Vael) leem o MESMO resource — um enum pequeno de mais
/// para pagar uma dependência entre crates. (O `bevy_ext_scene_runtime`,
/// que também o lia, foi apagado a 2026-11-15 — órfão.)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Resource)]
pub enum RunMode {
    /// `viber run`: o jogo.
    #[default]
    Game,
    /// Modo de edição in-process: cena congelada.
    Edit,
}

/// A toast requested by a script (`viber.toast`) or by gameplay code.
///
/// The HUD consumes it; the bridge logs it. Kept in `core` so the save layer
/// can load/save pending toasts without depending on the Luau runtime.
#[derive(Debug, Clone, bevy::ecs::message::Message)]
pub struct ScriptToast(pub String);

/// Bus volumes from `<AudioMixer master music sfx voice ambience>`.
///
/// The resource always exists: a derived `Default` with zeroed fields would
/// silence every bus, so [`Default`] is implemented explicitly with `1.0`.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::music"]
pub struct AudioMixerSettings {
    pub master: f32,
    pub music: f32,
    pub sfx: f32,
    pub voice: f32,
    pub ambience: f32,
}

impl Default for AudioMixerSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 1.0,
            sfx: 1.0,
            voice: 1.0,
            ambience: 1.0,
        }
    }
}

/// System ordering inside one UI frame: collect → build → bind → script →
/// style.
///
/// Explicit sets, because the order is the contract: a script has to see this
/// frame's bindings, and the restyle has to run after whatever the script
/// changed, or every class toggle would land a frame late.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiSet {
    /// Snapshot gameplay state into the UI data and list sources.
    Collect,
    /// Rebuild data-driven repeaters whose source changed.
    Build,
    /// Push bindings into elements and publish the script view.
    Bind,
    /// Apply the mutations scripts queued.
    Script,
    /// Recompute styles and mirror widget values into their child nodes.
    Style,
    /// The second half of styling (widget syncs and motion); the systems
    /// tuple in one chain has a ceiling and the style pass outgrew it.
    StyleLate,
}

/// Height query used by services that must not depend on the terrain crate.
pub trait TerrainQuery: Send + Sync + 'static {
    /// Terrain height at `(x, z)`, or `None` outside the field / over water.
    fn height_at(&self, x: f32, z: f32) -> Option<f32>;
}

/// Path query used by services that must not depend on the navigation crate.
pub trait NavQuery: Send + Sync + 'static {
    /// A world-space path from `from` to `to`, or `None` when there is no
    /// navmesh / no path.
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>>;
}

/// Sonda acima dos pés ao perguntar `surface_below` pelo chão.
pub const GROUND_PROBE: f32 = 0.05;

/// Speed base do herói (`BASE_SPEED` do controller original).
pub const BASE_SPEED: f32 = 4.0;

/// Estado do herói controlado por código (subconjunto do `PlayerController`).
///
/// Vive no core para os serviços da engine (terreno, navegação, UI, áudio)
/// dependerem do contrato e não do monólito. O `TypePath` histórico é
/// preservado para o BRP, o `.bsn` e os saves.
#[derive(Debug, Component, Reflect)]
#[reflect(Component, Default)]
#[type_path = "viber::player"]
pub struct Player {
    pub speed: f32,
    pub sprint_multiplier: f32,
    pub jump_height: f32,
    pub rotation_speed: f32,
    pub vel_y: f32,
    pub vel_x: f32,
    pub vel_z: f32,
    pub grounded: bool,
    pub can_jump: bool,
    pub is_jumping: bool,
    pub jump_cooldown: f32,
    pub last_grounded_time: f32,
    pub jump_buffer_time: f32,
    pub last_steer_time: f32,
    pub last_moving_time: f32,
    pub step_timer: f32,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            speed: BASE_SPEED,
            sprint_multiplier: 1.5,
            jump_height: 2.3,
            rotation_speed: 10.0,
            vel_y: 0.0,
            vel_x: 0.0,
            vel_z: 0.0,
            grounded: true,
            can_jump: true,
            is_jumping: false,
            jump_cooldown: 0.0,
            last_grounded_time: f32::NEG_INFINITY,
            jump_buffer_time: f32::NEG_INFINITY,
            last_steer_time: f32::NEG_INFINITY,
            last_moving_time: f32::INFINITY,
            step_timer: 0.0,
        }
    }
}

/// Marca um herói acabado de teleportar; o controller segura-o na superfície
/// analítica enquanto o collider da coluna de destino assa.
#[derive(Debug, Component, Reflect)]
#[reflect(Component, Default)]
#[type_path = "viber::player"]
pub struct TeleportSettle {
    /// Frames restantes de tutela.
    pub frames: u8,
}

impl Default for TeleportSettle {
    fn default() -> Self {
        Self { frames: 30 }
    }
}

// ── física de efeitos (pura) ─────────────────────────────────────────────

/// Decaimento exponencial do knockback (por segundo).
pub const KNOCKBACK_DECAY: f32 = 6.0;
/// Velocidade mínima abaixo da qual o knockback termina.
pub const KNOCKBACK_EPSILON: f32 = 0.05;
/// Duração da queda de um destrutível (s).
pub const FALL_DURATION: f32 = 0.9;
/// Ângulo total da queda (graus).
pub const FALL_ANGLE_DEG: f32 = 88.0;

/// Velocidade horizontal residual de um impacto (m/s).
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::physics_fx"]
pub struct Knockback {
    pub velocity: Vec3,
}

/// Destrutível a tombar (`break-style: fall`).
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::physics_fx"]
pub struct Falling {
    pub axis: Vec3,
    pub timer: f32,
    pub initial: Quat,
}

pub fn knockback_after(direction: Vec3, strength: f32) -> Knockback {
    let mut dir = direction;
    dir.y = 0.0;
    Knockback {
        velocity: dir.normalize_or_zero() * strength,
    }
}

pub fn knockback_decay_factor(dt: f32) -> f32 {
    if KNOCKBACK_DECAY > 0.0 {
        (-KNOCKBACK_DECAY * dt).exp()
    } else {
        1.0
    }
}

pub fn knockback_travel_factor(dt: f32) -> f32 {
    if KNOCKBACK_DECAY > 0.0 {
        (1.0 - knockback_decay_factor(dt)) / KNOCKBACK_DECAY
    } else {
        dt
    }
}

pub fn push_velocity(displacement: Vec3) -> Vec3 {
    displacement.with_y(0.0) * KNOCKBACK_DECAY
}

pub fn fall_rotation(axis: Vec3, angle_deg: f32) -> Quat {
    let axis = axis.try_normalize().unwrap_or(Vec3::X);
    Quat::from_axis_angle(axis, angle_deg.to_radians())
}

pub fn fall_angle(t: f32, duration: f32) -> f32 {
    let phase = (t / duration).clamp(0.0, 1.0);
    FALL_ANGLE_DEG * phase * phase
}

pub fn radial_strength(distance: f32, radius: f32, strength: f32) -> Option<f32> {
    if radius.is_nan() || radius <= 0.0 || !distance.is_finite() || distance > radius {
        return None;
    }
    Some(strength * (1.0 - 0.6 * (distance / radius)))
}

// ── câmara conduzida por script ──────────────────────────────────────────

/// Altura do pivô olhado pela câmara de terceira pessoa.
pub const LOOK_PIVOT_HEIGHT: f32 = 1.5;

#[derive(Debug, Clone, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::camera"]
pub struct ScriptCameraFree {
    pub active: bool,
    pub pos: Vec3,
    pub look: Vec3,
}

#[derive(Debug, Clone, Copy, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::camera"]
pub struct CameraSnapshot {
    pub pos: Vec3,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
}

// ── marcadores do HUD ────────────────────────────────────────────────────

#[derive(Debug, Clone, Reflect)]
#[type_path = "viber::hud::markers"]
pub struct HudPoint {
    pub position: Vec3,
    pub label: String,
}

#[derive(Debug, Clone, Reflect)]
#[type_path = "viber::hud::markers"]
pub struct HudBlip {
    pub position: Vec3,
    pub tower: bool,
}

#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::hud::markers"]
pub struct HudMarkers {
    pub blips: Vec<HudBlip>,
    pub beacons: Vec<Vec3>,
    pub waypoint: Option<HudPoint>,
}

// ── passos do herói ─────────────────────────────────────────────────────

/// Distância de uma passada a velocidade de caminhada (m).
pub const STEP_STRIDE_M: f32 = 2.2;
/// Abaixo desta velocidade não há passos.
pub const STEP_MIN_SPEED: f32 = 0.8;

/// Intervalo entre passos para uma velocidade (s), ou `None` parado.
pub fn step_interval(speed: f32) -> Option<f32> {
    (speed >= STEP_MIN_SPEED && speed.is_finite()).then(|| STEP_STRIDE_M / speed)
}

/// Decide um passo: decrementa o timer e devolve o intervalo no frame em
/// que o SFX deve soar. Parado/airborne reinicia o timer.
pub fn footstep_due(step_timer: &mut f32, grounded: bool, speed: f32, dt: f32) -> Option<f32> {
    *step_timer -= dt;
    match step_interval(speed).filter(|_| grounded) {
        Some(interval) if *step_timer <= 0.0 => {
            *step_timer = interval;
            Some(interval)
        }
        _ => {
            if !grounded || speed < STEP_MIN_SPEED {
                *step_timer = 0.0;
            }
            None
        }
    }
}

// ── configuração de interior e fronteira do mundo ────────────────────────

/// `<WorldBorder>` config.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct WorldBorderConfig {
    pub radius: f32,
    pub warn_seconds: f32,
    pub margin: f32,
}

pub const INTERIOR_CAMERA_DISTANCE: f32 = 22.0;
pub const INTERIOR_CAMERA_PITCH_DEG: f32 = 58.0;
pub const INTERIOR_FLOOR_Y: f32 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct InteriorSceneConfig {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub room_size: [f32; 2],
    pub camera_distance: f32,
    pub camera_pitch_deg: f32,
    pub camera_yaw_deg: f32,
    pub room_origin: [f32; 2],
}

impl Default for InteriorSceneConfig {
    fn default() -> Self {
        Self {
            min: [0.0, 0.0],
            max: [0.0, 0.0],
            room_size: [0.0, 0.0],
            camera_distance: INTERIOR_CAMERA_DISTANCE,
            camera_pitch_deg: INTERIOR_CAMERA_PITCH_DEG,
            camera_yaw_deg: 0.0,
            room_origin: [0.0, 0.0],
        }
    }
}

impl InteriorSceneConfig {
    pub fn contains(&self, x: f32, z: f32) -> bool {
        x >= self.min[0] && x <= self.max[0] && z >= self.min[1] && z <= self.max[1]
    }

    pub fn room_center(&self, x: f32, z: f32) -> [f32; 2] {
        let center_of = |min: f32, max: f32, origin: f32, p: f32, step: f32| {
            if step <= 0.0 {
                return 0.5 * (min + max);
            }
            let k = ((p - origin) / step).round();
            (origin + k * step).clamp(min, max)
        };
        [
            center_of(
                self.min[0],
                self.max[0],
                self.room_origin[0],
                x,
                self.room_size[0],
            ),
            center_of(
                self.min[1],
                self.max[1],
                self.room_origin[1],
                z,
                self.room_size[1],
            ),
        ]
    }
}

#[derive(Debug, Resource, Default, Clone, Copy, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct InteriorLighting {
    pub active: bool,
}

// ── áudio/eventos (tipos puros) ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
#[type_path = "viber::ambient"]
pub enum SfxClip {
    Hit,
    Whoosh,
    Harvest,
    Ui,
    /// Golpe de machado em árvore (colheita nativa, `harvest.rs`).
    ChopHit,
    /// Árvore a cair (break `fall`).
    ChopBreak,
    /// Picareta na rocha.
    MineHit,
    /// Rocha a estilhaçar (break `shatter`/`burst`).
    MineBreak,
    // ── Passe de Juice (WS-A): clips partilhados com o WS-D ──────────────
    /// Subida de nível (jingle).
    LevelUp,
    /// Missão concluída (jingle).
    QuestDone,
    /// Viagem rápida (whoosh longo).
    Travel,
    /// Passo do herói em terra/erva (seco, ~0.15 s).
    Footstep,
    /// Passo do herói em água rasa (salpico, ~0.2 s).
    FootstepWater,
    /// Loot apanhado / baú aberto.
    Loot,
    // ── Passe de som completo (pool partilhada) ──────────────────────────
    /// Herói sofre dano (grunt de impacto).
    Hurt,
    /// Cura (brilho de recuperação).
    Heal,
    /// Herói morre (sting descendente).
    GameOver,
    /// Missão aceite (chime curto de confirmação).
    QuestAccept,
    /// Notificação suave (toast de sistema).
    Notification,
    /// Moedas a cair / ouro apanhado.
    Coin,
    /// Compra feita (transação do mercador).
    Buy,
    /// Acção recusada (sem ouro, inventário cheio…).
    Error,
    /// Jogo guardado.
    Save,
    /// Jogo carregado.
    Load,
    /// Loja aberta (sino acolhedor).
    ShopOpen,
    /// Criatura atingida (impacto de carne).
    EnemyHurt,
    /// Criatura morre (último suspiro).
    EnemyDeath,
    /// Lobo a agredir (growl).
    WolfGrowl,
    /// Slime (squish).
    SlimeSquish,
    /// Boss (roar).
    BossRoar,
    /// Guarda absorveu o golpe (metal na madeira).
    ShieldBlock,
    /// Porta a abrir.
    DoorOpen,
    /// Porta a fechar.
    DoorClose,
    /// Bomba lançada (fuso a arder).
    BombDrop,
    /// Herói salta.
    Jump,
    /// Dash do herói (rajada de ar).
    Dash,
    /// Kart: batida/parede (corrida).
    Crash,
    /// Kart: nitro a entrar.
    Nitro,
    /// Kart: derrapagem (loop curto disparado por aresta).
    Skid,
    /// Arco: corda solta.
    BowShot,
    /// Flecha a cravar-se (impacto).
    ArrowHit,
    /// Fogo a crepitar (one-shot/loop curto).
    FireCrackle,
    /// Item a cair ao chão.
    ItemDrop,
    /// Trovão distante (one-shot; o `bevy_elements` emite `Thunder`).
    Thunder,
}

/// Todos os clips, pela ordem do enum — alimenta o preload dos handles, o
/// audit (`analyze` valida a existência de cada ficheiro) e o test que
/// garante o registry Lua completo.
pub const SFX_CLIPS_ALL: &[SfxClip] = &[
    SfxClip::Hit,
    SfxClip::Whoosh,
    SfxClip::Harvest,
    SfxClip::Ui,
    SfxClip::ChopHit,
    SfxClip::ChopBreak,
    SfxClip::MineHit,
    SfxClip::MineBreak,
    SfxClip::LevelUp,
    SfxClip::QuestDone,
    SfxClip::Travel,
    SfxClip::Footstep,
    SfxClip::FootstepWater,
    SfxClip::Loot,
    SfxClip::Hurt,
    SfxClip::Heal,
    SfxClip::GameOver,
    SfxClip::QuestAccept,
    SfxClip::Notification,
    SfxClip::Coin,
    SfxClip::Buy,
    SfxClip::Error,
    SfxClip::Save,
    SfxClip::Load,
    SfxClip::ShopOpen,
    SfxClip::EnemyHurt,
    SfxClip::EnemyDeath,
    SfxClip::WolfGrowl,
    SfxClip::SlimeSquish,
    SfxClip::BossRoar,
    SfxClip::ShieldBlock,
    SfxClip::DoorOpen,
    SfxClip::DoorClose,
    SfxClip::BombDrop,
    SfxClip::Jump,
    SfxClip::Dash,
    SfxClip::Crash,
    SfxClip::Nitro,
    SfxClip::Skid,
    SfxClip::BowShot,
    SfxClip::ArrowHit,
    SfxClip::FireCrackle,
    SfxClip::ItemDrop,
    SfxClip::Thunder,
];


impl SfxClip {
    /// Clip RELATIVO ao `sfx_dir` do config.yaml do jogo
    /// (`config.sfx_path(clip.file())` dá o path AssetServer).
    pub fn file(self) -> &'static str {
        match self {
            SfxClip::Hit => "hit.ogg",
            SfxClip::Whoosh => "whoosh.ogg",
            SfxClip::Harvest => "harvest.ogg",
            SfxClip::Ui => "ui.ogg",
            SfxClip::ChopHit => "combat/chop_hit.ogg",
            SfxClip::ChopBreak => "combat/chop_break.ogg",
            SfxClip::MineHit => "combat/mine_hit.ogg",
            SfxClip::MineBreak => "combat/mine_break.ogg",
            SfxClip::LevelUp => "ui/levelup.ogg",
            SfxClip::QuestDone => "ui/quest_complete.ogg",
            SfxClip::Travel => "combat/swing.ogg",
            SfxClip::Footstep => "ambient/footstep.ogg",
            SfxClip::FootstepWater => "ambient/footstep_water.ogg",
            SfxClip::Loot => "world/chest_open.ogg",
            SfxClip::Hurt => "player/hurt.ogg",
            SfxClip::Heal => "player/heal.ogg",
            SfxClip::GameOver => "ui/game_over.ogg",
            SfxClip::QuestAccept => "ui/quest_accept.ogg",
            SfxClip::Notification => "ui/notification.ogg",
            SfxClip::Coin => "ui/coin.ogg",
            SfxClip::Buy => "ui/buy.ogg",
            SfxClip::Error => "ui/error.ogg",
            SfxClip::Save => "ui/save.ogg",
            SfxClip::Load => "ui/load.ogg",
            SfxClip::ShopOpen => "ui/shop_open.ogg",
            SfxClip::EnemyHurt => "creatures/enemy_hurt.ogg",
            SfxClip::EnemyDeath => "creatures/enemy_death.ogg",
            SfxClip::WolfGrowl => "creatures/wolf_growl.ogg",
            SfxClip::SlimeSquish => "creatures/slime_squish.ogg",
            SfxClip::BossRoar => "creatures/boss_roar.ogg",
            SfxClip::ShieldBlock => "combat/shield_block.ogg",
            SfxClip::DoorOpen => "world/door_open.ogg",
            SfxClip::DoorClose => "world/door_close.ogg",
            SfxClip::BombDrop => "world/bomb_drop.ogg",
            SfxClip::Crash => "vehicles/crash.ogg",
            SfxClip::Nitro => "vehicles/nitro.ogg",
            SfxClip::Skid => "vehicles/skid.ogg",
            SfxClip::Jump => "player/jump.ogg",
            SfxClip::Dash => "player/dash.ogg",
            SfxClip::BowShot => "combat/bow_shot.ogg",
            SfxClip::ArrowHit => "combat/arrow_hit.ogg",
            SfxClip::FireCrackle => "world/fire_crackle.ogg",
            SfxClip::ItemDrop => "world/item_drop.ogg",
            SfxClip::Thunder => "ambient/thunder.ogg",
        }
    }
}


/// Toca um SFX (volume cai com a distância ao oyente — a câmara).
#[derive(Debug, Clone, Copy, bevy::ecs::message::Message)]
pub struct SfxEvent {
    pub clip: SfxClip,
    /// Posição no mundo; `None` = som de interface (volume cheio).
    pub position: Option<Vec3>,
}

/// Toca um SFX com um GANHO explícito (0..1 linear), sem a atenuação de
/// 40 m do [`SfxEvent`]. É para sons cuja distância já foi calculada na
/// fonte — o trovão do `bevy_elements` traz `distance_m`/`loudness` e a
/// atenuação de 40 m clampava-o a 0,05 sempre.
#[derive(Debug, Clone, Copy, bevy::ecs::message::Message)]
pub struct SfxLoudEvent {
    pub clip: SfxClip,
    /// Posição no mundo (informativa), em unidades de mundo.
    pub position: Option<Vec3>,
    /// Ganho linear 0..1, aplicado tal-e-qual.
    pub volume: f32,
}

/// Loops ambientes carregados por path fixo, relativos ao `sfx_dir`.
pub const AMBIENT_LOOP_FILES: &[&str] = &[
    "world/water_lake.ogg",
    "world/water_flow.ogg",
    "world/water_waterfall.ogg",
    "ambient/rain_loop.ogg",
];

/// O emissor de chuva da engine (partículas CPU) está ligado?
///
/// O `bevy_elements` desenha a chuva em GPU; com o clima ligado no Vael os
/// dois donos davam chuva dupla. A engine continua a manter as ondulações no
/// terreno real (`rain_ripple_spawner`) e o loop de áudio
/// (`rain_ambience_driver`); só o emissor de gotas é desligado.
#[derive(Resource, Debug, Clone, Copy)]
pub struct EngineRainVisuals(pub bool);

impl Default for EngineRainVisuals {
    fn default() -> Self {
        Self(true)
    }
}

// ── locomoção/IA e marcadores de gameplay (contratos) ────────────────────
//
// Tipos que a navegação (`bevy_ext_nav`) e outros serviços da engine leem
// como contrato e que o monólito re-exporta nos módulos originais
// (`viber::ai`, `viber::vitals`, `viber::physics`, `viber::ragdoll`,
// `viber::scene_loader`) durante a transição.
//
// Os vitais (`Health`/`Hostile`/dano) são a definição CANÓNICA de
// [`crate::vitals`] — o monólito (`viber::vitals`), o `bevy_ext_engine` e o
// `bevy_ext_ai` re-exportam esse módulo; a raiz mantém os três nomes para
// quem os lia aqui.

/// Velocity-space locomotion shared by the Rust FSM (`enemy_ai`) and the
/// Luau scripts (`viber.move_towards` / `move_by` / `face_towards`).
///
/// Producers state a *desire* for the frame ([`Self::drive`],
/// [`Self::look_at`]) and `apply_ai_locomotion` is the single consumer that
/// turns it into movement: it ramps the velocity toward the desire at the
/// profile's acceleration, steps the transform, snaps Y to the carved surface
/// and turns the yaw at the profile's turn rate. Before this existed both
/// producers wrote `Transform::translation` and `Transform::rotation` outright
/// — full speed on the first frame and a hard 180° snap on every retarget.
///
/// The desire is a *one-frame* contract: the consumer clears it. A producer
/// that stops asking decelerates to a stop instead of freezing mid-stride.
#[derive(Debug, Clone, Copy, Component, Default, Reflect)]
#[reflect(Component, Default)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::ai"]
pub struct AiLocomotion {
    /// Current planar velocity (m/s). Integrated, never assigned by producers.
    pub velocity: Vec2,
    /// Desired planar velocity (m/s) for this frame.
    desired: Vec2,
    /// A producer asked for something this frame (distinguishes "stand still"
    /// from "nobody is driving me").
    driven: bool,
    /// One-frame look-at direction (XZ), honoured only below
    /// `FACING_MOVE_SPEED`.
    facing: Option<Vec2>,
    /// Where the producer is actually trying to GO, when it knows.
    ///
    /// The desired velocity alone is not enough for a pathfinder: projecting it
    /// forward only ever names a point on the straight line the character
    /// cannot take, which is precisely the line the wall is on. `viber.move_towards`
    /// and the FSM both know their destination, so they say it, and the
    /// navigation bridge paths to the real thing.
    goal: Option<Vec2>,
}

impl AiLocomotion {
    /// Asks for a planar velocity (m/s) this frame, with no stated destination.
    ///
    /// Use [`Self::drive_to`] whenever the destination IS known: without it the
    /// navigation bridge can only guess where the character is heading.
    pub fn drive(&mut self, desired: Vec2) {
        self.desired = desired;
        self.driven = true;
        self.goal = None;
    }

    /// Asks to travel toward `goal` at `speed` (m/s), from `here`.
    ///
    /// Never asks for more speed than closes the remaining gap this frame, so
    /// arrival is the same as it always was.
    pub fn drive_to(&mut self, here: Vec2, goal: Vec2, speed: f32, dt: f32) {
        let offset = goal - here;
        if offset.length_squared() <= 1e-8 || speed <= 0.0 {
            self.halt();
            self.goal = Some(goal);
            return;
        }
        let reach = offset.length() / dt.max(1e-4);
        self.desired = offset.normalize() * speed.min(reach);
        self.driven = true;
        self.goal = Some(goal);
    }

    /// Asks to stand still this frame (decelerating, not freezing).
    pub fn halt(&mut self) {
        self.desired = Vec2::ZERO;
        self.driven = true;
        self.goal = None;
    }

    /// Asks to look toward a direction, if the character is slow enough to be
    /// allowed to (see `FACING_MOVE_SPEED`).
    pub fn look_at(&mut self, dir: Vec2) {
        if dir.length_squared() > 1e-8 {
            self.facing = Some(dir.normalize());
        }
    }

    /// Current planar speed (m/s).
    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }

    /// The velocity asked for this frame, before the acceleration ramp.
    ///
    /// The navigation bridge reads it to know where the character *wants* to
    /// go, replaces it with a navigated answer, and hands it back — which is
    /// why the scripts never learn that pathfinding exists.
    pub fn desired(&self) -> Vec2 {
        self.desired
    }

    /// A producer asked for something this frame.
    pub fn is_driven(&self) -> bool {
        self.driven
    }

    /// Where the producer is trying to go, when it said so.
    pub fn goal(&self) -> Option<Vec2> {
        self.goal
    }

    /// The one-frame look-at direction, when a producer asked for one.
    ///
    /// O consumidor só a honra abaixo de `FACING_MOVE_SPEED` — acima disso o
    /// corpo aponta para onde vai.
    pub fn facing(&self) -> Option<Vec2> {
        self.facing
    }

    /// The desire has been consumed: clears ask, driver, facing and goal.
    ///
    /// É o fecho do contrato de um frame — o consumidor único chama-o depois
    /// de integrar o passo; o frame seguinte começa parado a menos que um
    /// produtor volte a pedir.
    pub fn consume(&mut self) {
        self.desired = Vec2::ZERO;
        self.driven = false;
        self.facing = None;
        self.goal = None;
    }
}

/// Behaviour state of one `EnemyCreature`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::ai"]
pub enum EnemyState {
    /// Patrol around `home`; the [`Default`].
    #[default]
    Wander,
    /// Viu ou ouviu: vira-se para o ponto, ainda sem correr.
    Alert,
    /// Run at the player while it can see them.
    Chase,
    /// Perdeu o alvo: vai ao último ponto e desiste.
    Search,
}

impl EnemyState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wander => "wander",
            Self::Alert => "alert",
            Self::Chase => "chase",
            Self::Search => "search",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "wander" => Some(Self::Wander),
            "alert" => Some(Self::Alert),
            "chase" => Some(Self::Chase),
            "search" => Some(Self::Search),
            _ => None,
        }
    }
}

/// A spawned creature: authored stats plus its current behaviour state.
///
/// `Default` is Wander with `home` unset — `enemy_ai` latches the real home
/// to the spawn position on the first tick.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component, Default)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::ai"]
pub struct EnemyCreature {
    /// Chase speed in m/s (wander runs at `WANDER_SPEED_FRACTION` × this).
    pub speed: f32,
    /// Player distance that flips Wander → Chase.
    pub aggro_radius: f32,
    /// Chase stops (attack range) once this close to the player.
    pub attack_radius: f32,
    /// Patrol anchor (XZ); `None` until `enemy_ai` latches it to the spawn
    /// position on the first tick — um sentinela `Vec2::ZERO` re-latchava o
    /// home a cada tick em criaturas spawnadas em (0,0) (patrulha derivava).
    pub home: Option<Vec2>,
    pub state: EnemyState,
}

impl Default for EnemyCreature {
    fn default() -> Self {
        Self {
            speed: 2.0,
            aggro_radius: 18.0,
            attack_radius: 2.2,
            home: None,
            state: EnemyState::Wander,
        }
    }
}

/// System set do consumidor único de [`AiLocomotion`], para que quem escreve
/// o pedido de locomoção (a ponte de navegação) se ordene antes dele sem
/// nomear a função.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AiLocomotionSystems;

// ── contratos partilhados animação ↔ IA (exit4/G1) ──────────────────────
//
// O perfil de locomoção é calibrado pela ANIMAÇÃO (`observe`, pelos speeds
// comandados) e lido pela IA (`apply_ai_locomotion`, accel/turn_rate); o
// hit-react é inserido pelas vias de dano e observado pelos sentidos; o
// `LuaScriptRef` liga a entidade ao chunk Luau e é o filtro "scripted" dos
// sentidos/folhagem. UM tipo por contrato — os caminhos de reflexão
// históricos (`viber::animation::…`, `viber::luau::components::…`) ficam
// congelados aqui e os módulos originais reexportam.

/// Default yaw rate (rad/s) of a character with no authored profile.
///
/// ~7 rad/s turns 180° in a quarter second: fast enough that a creature still
/// commits to a new target immediately, slow enough that the eye reads a turn
/// instead of a teleported orientation.
pub const DEFAULT_TURN_RATE: f32 = 7.0;

/// Default acceleration (m/s²) toward the desired velocity.
///
/// At 12 m/s² a 4 m/s chase reaches full speed in a third of a second — the
/// ramp is what the animation's own low-pass (`SPEED_SMOOTH_TAU`) was fighting
/// when the AI went from standing to full speed inside one frame.
pub const DEFAULT_ACCEL: f32 = 12.0;

/// Nominais de CREATURE sem perfil medido (o `MotionSpeeds::CREATURE` da
/// animação: wander ~1.5 m/s, chase ~4 m/s) — o default do perfil tem de
/// coincidir com o que a animação assume enquanto nada foi observado.
const PROFILE_DEFAULT_WALK: f32 = 1.5;
/// Ver [`PROFILE_DEFAULT_WALK`].
const PROFILE_DEFAULT_RUN: f32 = 4.0;

/// Speed below which a commanded step counts as "not moving" for calibration.
const CALIBRATION_FLOOR: f32 = 0.05;

/// A walk nominal this close to the run nominal means we only ever saw ONE
/// speed, so the pair is synthesised instead of measured.
const CALIBRATION_SPREAD: f32 = 0.85;

/// Nominal walk as a fraction of run, when only one speed was ever observed.
const CALIBRATION_WALK_FRACTION: f32 = 0.45;

/// Per-rig locomotion nominals — what "walking" and "running" mean for THIS
/// character, plus how fast it may turn and accelerate.
///
/// Authoring is optional by design (mission premise "ease over knobs"): the
/// component calibrates itself from the speeds the entity is actually commanded
/// at, through [`Self::observe`]. The Luau scripts pass their speed to
/// `viber.move_towards`, and the Rust FSM has it in `EnemyCreature::speed`, so
/// both paths teach it the truth without a single line of authoring. An
/// explicit `locomotion="walk: 1.1; run: 4.6"` (or `viber.set_locomotion`)
/// latches [`Self::authored`] and stops the calibration.
///
/// Os clamps de playback de clip (`speeds()`/`gates()`) ficam no módulo de
/// animação do host — são domínio de clip, não de locomoção.
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
// Caminho histórico (`viber::animation::LocomotionProfile`).
#[type_path = "viber::animation"]
pub struct LocomotionProfile {
    /// Nominal walking speed (m/s) of the rig's walk clip.
    pub walk: f32,
    /// Nominal running speed (m/s) of the rig's run clip.
    pub run: f32,
    /// Maximum yaw rate (rad/s). The whole reason a creature stops spinning
    /// 180° in one frame when it picks a new wander target.
    pub turn_rate: f32,
    /// Acceleration (m/s²) toward the desired velocity.
    pub accel: f32,
    /// Nominals came from authoring, not from observation.
    pub authored: bool,
    /// Slowest non-zero speed ever commanded (`INFINITY` until the first one).
    observed_min: f32,
    /// Fastest speed ever commanded.
    observed_max: f32,
}

impl Default for LocomotionProfile {
    fn default() -> Self {
        Self {
            walk: PROFILE_DEFAULT_WALK,
            run: PROFILE_DEFAULT_RUN,
            turn_rate: DEFAULT_TURN_RATE,
            accel: DEFAULT_ACCEL,
            authored: false,
            observed_min: f32::INFINITY,
            observed_max: 0.0,
        }
    }
}

impl LocomotionProfile {
    /// A profile with nominals the world author (or a script) stated outright.
    pub fn authored(walk: f32, run: f32) -> Self {
        Self {
            walk: walk.max(1e-3),
            run: run.max(walk).max(1e-3),
            authored: true,
            ..Self::default()
        }
    }

    /// Feeds one commanded speed into the calibration.
    ///
    /// Returns `true` when the nominals moved, so callers can skip change
    /// detection noise on the component.
    pub fn observe(&mut self, commanded: f32) -> bool {
        if self.authored || !commanded.is_finite() || commanded <= CALIBRATION_FLOOR {
            return false;
        }
        let min = self.observed_min.min(commanded);
        let max = self.observed_max.max(commanded);
        if min == self.observed_min && max == self.observed_max {
            return false;
        }
        self.observed_min = min;
        self.observed_max = max;
        // Two clearly separate speeds (wander vs chase) ARE the two nominals.
        // One speed only — a townsfolk that never runs — gives the walk
        // nominal, and the run nominal is synthesised above it so the rig keeps
        // a headroom instead of flipping to the run clip on its own walk.
        self.run = if min < max * CALIBRATION_SPREAD {
            self.walk = min;
            max
        } else {
            self.walk = max;
            max / CALIBRATION_WALK_FRACTION
        };
        true
    }

    /// True once the profile has something better than the module defaults.
    pub fn is_calibrated(&self) -> bool {
        self.authored || self.observed_max > 0.0
    }
}

/// Pedido de hit-react (flinch ao levar dano): inserido pelas vias de dano
/// (golpe de melee no land, `PlayerHurt` físico no herói). Consumido pelo
/// hit-react system do host, que o remove SEMPRE — um pedido órfão nunca
/// re-dispara mais tarde. Os sentidos o observam para sintetizar audição.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
// Caminho histórico (`viber::animation::HitReact`).
#[type_path = "viber::animation"]
pub struct HitReact;

/// Entidade conduzida por um script Luau (`scripts/<path>` relativo ao
/// diretório do mundo).
///
/// WIRED-BY-ORCHESTRATOR: inserted by the spawn step; o runtime do host
/// observa-a (`on_add` / update / `on_remove`) e os consumidores extraídos
/// (folhagem, cinemáticas, sentidos) usam-na como filtro.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
// Caminho congelado da whitelist do espelho Luau (`viber::luau::components::…`
// em `luau/ecs.rs` do monólito); o default do Rust mudaria ao sair daí.
#[type_path = "viber::luau::components"]
pub struct LuaScriptRef {
    /// Script path relative to `world_dir/scripts/` (e.g. `"doors/gate.lua"`).
    pub path: String,
}

/// O schedule que (re)carrega uma cena: os mesmos sistemas do `Startup` que
/// montam o mundo.
#[derive(bevy::ecs::schedule::ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneLoad;

/// Marks a voxel COLUMN that currently owns a terrain trimesh collider.
///
/// O collider vive na entidade da coluna (um trimesh por coluna, triângulos
/// dos caixas transvoxel), não nas caixas — uma aresta interna a cada 64 m
/// em vez de a cada 32 m, e zero superfícies coincidentes na pilha vertical.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::physics"]
pub struct VoxelCollider;

/// Corpo de ragdoll ligado ao cadáver que o originou. Se o cadáver for
/// despawnado antes dos 2,5 s (corpse lifetime 1,4 s, troca de cena), o
/// `RagdollActive` desaparecia com ele e os corpos ficavam para sempre no
/// mundo da física.
#[derive(Component, Reflect)]
#[reflect(Component)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::ragdoll"]
pub struct RagdollBodyOf(pub Entity);


