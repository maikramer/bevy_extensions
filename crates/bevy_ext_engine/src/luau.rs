//! Contratos de scripting — os tipos que atravessam a fronteira entre o
//! runtime Luau (monólito) e os consumidores extraídos (cinemáticas,
//! folhagem): a fila de eventos engine→Lua, a marca de script por entidade e
//! o registry de SFX.
//!
//! Extraído do `luau/{events,components,sfx}.rs` do monólito; o monólito
//! reexporta para `viber::luau::{ScriptEventQueue, ScriptGameEvent,
//! LuaScriptRef, sfx_clip_from_str, …}` continuarem a resolver — UM tipo só
//! no ECS: o runtime empurra para a fila daqui e as cinemáticas daqui drenam
//! a mesma fila.
//!
//! O host Luau (instalação/fan-out dos eventos) continua no monólito. O
//! método [`ScriptGameEvent::to_table`] precisa do runtime mlua e fica
//! atrás da feature `luau`.

use bevy::prelude::*;

use bevy_ext_core::SfxClip;

/// Teto da fila de eventos por-path (`viber.events`): transbordo descarta o
/// evento NOVO (warn 1× por path) — um script que não drena não cria lag.
pub const EVENT_QUEUE_CAP: usize = 64;

/// Evento engine→Lua: estado do jogo que mudou FORA dos scripts (kill, dano
/// ao herói, level-up, cliques de UI…). É a base do desacoplamento — sem ela,
/// lógica como "ao matar X, Y acontece" só podia viver em Rust.
///
/// Pull model: os produtores empurram para [`ScriptEventQueue`]; o
/// `luau_update` do monólito faz fan-out para as filas por-path (cada script
/// que chamou `viber.events()` pelo menos uma vez) e o script drena com
/// `viber.events()` → array `{type=..., ...}`.
#[derive(Debug, Clone, Reflect)]
#[type_path = "viber::luau"]
pub enum ScriptGameEvent {
    /// Criatura morreu (`kill_creature` do melee, ou HP de entidade a zero
    /// por `viber.entity_damage`). `name` = nome da entidade; `entity` = bits.
    Kill { name: String, entity: i64 },
    /// Um projétil tocou um corpo. O jogo aplica o dano.
    Hit {
        entity: i64,
        amount: f32,
        faction: String,
        x: f32,
        y: f32,
        z: f32,
    },
    /// O herói morreu (a pousar no ponto de respawn).
    PlayerDied,
    /// Algo entrou no vault (`grant_loot` de colheita, `viber.vault_add` com
    /// `report_collect`, loot de chest…).
    Collect { item: String, amount: u32 },
    /// Quest entregue com sucesso (recompensas já aplicadas).
    QuestDone { id: String },
    /// Subida de nível (deteção no `vitals`).
    LevelUp { level: u32 },
    /// Ação da UI declarativa (`viber.ui.action("buy", "potion")`) — encaminhada
    /// a TODOS os scripts subscritos; os handlers nativos também correm, salvo
    /// as ações reclamadas por `viber.ui.own_action`.
    UiAction { name: String, arg: String },
    /// Uma cinemática acabou (`skipped` = o jogador saltou-a).
    CinematicEnd { id: String, skipped: bool },
    /// A cena acabou de carregar (`viber.change_scene` / portal); `entry` = a entrada de
    /// chegada (vazio = nenhuma).
    SceneLoaded { scene: String, entry: String },
    /// A cena `scene` vai ser descarregada (último aviso antes de as entidades morrerem).
    SceneUnloading { scene: String },
}

impl ScriptGameEvent {
    /// `type` do evento em Lua.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Kill { .. } => "kill",
            Self::Hit { .. } => "hit",
            Self::PlayerDied => "player_died",
            Self::Collect { .. } => "collect",
            Self::QuestDone { .. } => "quest_done",
            Self::LevelUp { .. } => "level_up",
            Self::UiAction { .. } => "ui_action",
            Self::CinematicEnd { .. } => "cinematic_end",
            Self::SceneLoaded { .. } => "scene_loaded",
            Self::SceneUnloading { .. } => "scene_unloading",
        }
    }

    /// Converte para a tabela Lua que `viber.events()` devolve. Uma tabela
    /// NOVA por chamada (partilhar referência entre filas deixava um script
    /// mutar o evento de outro).
    ///
    /// Atrás da feature `luau`: precisa do runtime mlua, que os consumidores
    /// sem host Luau não pagam.
    #[cfg(feature = "luau")]
    pub fn to_table(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Table> {
        let t = lua.create_table()?;
        t.raw_set("type", self.kind())?;
        match self {
            Self::Kill { name, entity } => {
                t.raw_set("name", name.clone())?;
                t.raw_set("entity", *entity)?;
            }
            Self::Hit {
                entity,
                amount,
                faction,
                x,
                y,
                z,
            } => {
                t.raw_set("entity", *entity)?;
                t.raw_set("amount", *amount)?;
                t.raw_set("faction", faction.clone())?;
                t.raw_set("x", *x)?;
                t.raw_set("y", *y)?;
                t.raw_set("z", *z)?;
            }
            Self::PlayerDied => {}
            Self::Collect { item, amount } => {
                t.raw_set("item", item.clone())?;
                t.raw_set("amount", *amount)?;
            }
            Self::QuestDone { id } => {
                t.raw_set("id", id.clone())?;
            }
            Self::LevelUp { level } => {
                t.raw_set("level", *level)?;
            }
            Self::UiAction { name, arg } => {
                t.raw_set("name", name.clone())?;
                t.raw_set("arg", arg.clone())?;
            }
            Self::CinematicEnd { id, skipped } => {
                t.raw_set("id", id.clone())?;
                t.raw_set("skipped", *skipped)?;
            }
            Self::SceneLoaded { scene, entry } => {
                t.raw_set("scene", scene.clone())?;
                t.raw_set("entry", entry.clone())?;
            }
            Self::SceneUnloading { scene } => {
                t.raw_set("scene", scene.clone())?;
            }
        }
        Ok(t)
    }
}

/// Fila de entrada dos eventos engine→Lua (producers → `luau_update`). Os
/// produtores usam `Option<ResMut>`: apps mínimas de teste sem o recurso
/// continuam a compilar e a correr.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::luau"]
pub struct ScriptEventQueue(pub Vec<ScriptGameEvent>);

impl ScriptEventQueue {
    /// Atalho para produtores.
    pub fn push(&mut self, event: ScriptGameEvent) {
        self.0.push(event);
    }
}

/// A component marking an entity as owned by a Luau script (`scripts/<path>`
/// relative to the world directory).
///
/// WIRED-BY-ORCHESTRATOR: inserted by the spawn step; o runtime do monólito
/// observa-a (`on_add` / update / `on_remove`) e os consumidores extraídos
/// (folhagem, cinemáticas) usam-na como filtro.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::luau"]
pub struct LuaScriptRef {
    /// Script path relative to `world_dir/scripts/` (e.g. `"doors/gate.lua"`).
    pub path: String,
}

// ── registry de SFX (`viber.sound`) ─────────────────────────────────────

/// Registry nome-clip Lua (`viber.sound`) — case-insensitive, tabela única.
/// Os gatilhos nativos usam as variantes directamente; os scripts passam por
/// aqui. Aliases aceitáveis além do nome canónico (ex. `level_up`) ficam em
/// linhas próprias apontando à mesma variante.
pub const SFX_NAME_REGISTRY: &[(&str, SfxClip)] = &[
    ("hit", SfxClip::Hit),
    ("whoosh", SfxClip::Whoosh),
    ("harvest", SfxClip::Harvest),
    ("ui", SfxClip::Ui),
    ("chop_hit", SfxClip::ChopHit),
    ("chop_break", SfxClip::ChopBreak),
    ("mine_hit", SfxClip::MineHit),
    ("mine_break", SfxClip::MineBreak),
    ("levelup", SfxClip::LevelUp),
    ("level_up", SfxClip::LevelUp),
    ("quest_complete", SfxClip::QuestDone),
    ("quest_done", SfxClip::QuestDone),
    ("travel", SfxClip::Travel),
    ("loot", SfxClip::Loot),
    ("chest_open", SfxClip::Loot),
    ("footstep", SfxClip::Footstep),
    ("footstep_water", SfxClip::FootstepWater),
    ("hurt", SfxClip::Hurt),
    ("heal", SfxClip::Heal),
    ("game_over", SfxClip::GameOver),
    ("quest_accept", SfxClip::QuestAccept),
    ("notification", SfxClip::Notification),
    ("coin", SfxClip::Coin),
    ("buy", SfxClip::Buy),
    ("error", SfxClip::Error),
    ("save", SfxClip::Save),
    ("load", SfxClip::Load),
    ("shop_open", SfxClip::ShopOpen),
    ("enemy_hurt", SfxClip::EnemyHurt),
    ("enemy_death", SfxClip::EnemyDeath),
    ("wolf_growl", SfxClip::WolfGrowl),
    ("growl", SfxClip::WolfGrowl),
    ("slime_squish", SfxClip::SlimeSquish),
    ("boss_roar", SfxClip::BossRoar),
    ("roar", SfxClip::BossRoar),
    ("shield_block", SfxClip::ShieldBlock),
    ("block", SfxClip::ShieldBlock),
    ("door_open", SfxClip::DoorOpen),
    ("door_close", SfxClip::DoorClose),
    ("bomb_drop", SfxClip::BombDrop),
    ("jump", SfxClip::Jump),
    ("dash", SfxClip::Dash),
    ("crash", SfxClip::Crash),
    ("nitro", SfxClip::Nitro),
    ("skid", SfxClip::Skid),
    ("bow_shot", SfxClip::BowShot),
    ("arrow_hit", SfxClip::ArrowHit),
    ("fire_crackle", SfxClip::FireCrackle),
    ("item_drop", SfxClip::ItemDrop),
    ("thunder", SfxClip::Thunder),
];

/// Nome de clip SFX Lua (`"hit"`, `"UI"`) → [`SfxClip`]
/// (case-insensitive via [`SFX_NAME_REGISTRY`]; desconhecido = `None`).
pub fn sfx_clip_from_str(name: &str) -> Option<SfxClip> {
    let lower = name.to_ascii_lowercase();
    SFX_NAME_REGISTRY
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map(|(_, clip)| *clip)
}

/// Índice do clip de gesto para um pedido `viber.gesture(name)` (pure fn).
///
/// Tenta cada alternativa (separadas por `,` ou `|`) por ordem; dentro de
/// cada uma, o match EXACTO normalizado ganha à substring — ambos os lados
/// passam por [`bevy_ext_core::animation::normalize_clip_name`] (caixa,
/// `_`/`-` e prefixos de ferramenta caem fora), por isso `"foldarms"`
/// encontra `Animator3D_FoldArms`. Sem correspondência = `None` (o chamador
/// avisa 1× e ignora, sem crash).
pub fn match_gesture_clip(
    animator: &bevy_ext_core::animation::CharacterAnimator,
    request: &str,
) -> Option<usize> {
    let names: Vec<String> = animator
        .clip_names
        .iter()
        .map(|n| bevy_ext_core::animation::normalize_clip_name(n))
        .collect();
    for want in request.split([',', '|']) {
        let want = bevy_ext_core::animation::normalize_clip_name(want);
        if want.is_empty() {
            continue;
        }
        if let Some(i) = names.iter().position(|n| *n == want) {
            return Some(i);
        }
        if let Some(i) = names.iter().position(|n| n.contains(&want)) {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Um clip novo no enum SEM linha aqui falha o teste do monólito
    /// (`test_lua_registry_covers_all_clips`); esta réplica local garante o
    /// mesmo para o registry movido.
    #[test]
    fn test_lua_registry_covers_all_clips() {
        for clip in bevy_ext_core::SFX_CLIPS_ALL {
            assert!(
                SFX_NAME_REGISTRY.iter().any(|(_, c)| c == clip),
                "clip {clip:?} sem linha no registry `viber.sound`"
            );
        }
    }

    #[test]
    fn test_sfx_clip_from_str_is_case_insensitive_and_aliases() {
        assert_eq!(sfx_clip_from_str("HIT"), Some(SfxClip::Hit));
        assert_eq!(sfx_clip_from_str("level_up"), Some(SfxClip::LevelUp));
        assert_eq!(sfx_clip_from_str("quest_complete"), Some(SfxClip::QuestDone));
        assert_eq!(sfx_clip_from_str("inexistente"), None);
    }

    #[test]
    fn test_match_gesture_clip_prefers_exact_over_substring() {
        let animator = bevy_ext_core::animation::CharacterAnimator {
            clip_names: vec!["swordidle".into(), "idle".into()],
            nodes: vec![],
            durations: vec![],
            player: Entity::PLACEHOLDER,
            state: None,
            current: None,
            action_time: 0.0,
            locked: false,
            air_time: 0.0,
            speed: 0.0,
            last_pos: None,
            locomotion: None,
        };
        assert_eq!(
            match_gesture_clip(&animator, "idle"),
            Some(1),
            "match exato `idle` ganha à substring em `swordidle`"
        );
        assert_eq!(match_gesture_clip(&animator, "foldarms"), None);
        assert_eq!(match_gesture_clip(&animator, "foldarms|idle"), Some(1));
    }
}
