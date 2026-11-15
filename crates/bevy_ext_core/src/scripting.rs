//! Contratos do host de scripting Luau que mais do que um crate toca.
//!
//! Viviam no `bevy_ext_ui::ui::host` (exit3/wave-c — com TODO(viber-exit)
//! "mover para o crate de luau"). O destino canónico é AQUI (2026-11-15,
//! exit4/F2): a UI declarativa empurra `UiAction` para A MESMA fila que o
//! host de scripting drena, e o componente de interação é escrito pelo host
//! e lido pelo `collect` da UI — com a definição nos dois lados, ou um deles
//! dependeria do outro num ciclo (`ui` não pode dep `luau` e vice-versa
//! quando o `luau` instala `viber.ui`). Os TypePaths históricos
//! (`viber::luau::events`, `viber::luau::components`) ficam congelados —
//! BRP/editor/saves não partem.

use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;

/// Alvo de interação registado por script (`viber.set_interaction`) — o
/// prompt "[tecla] label" aparece quando o player está perto. A UI publica o
/// mais próximo em alcance (`ui::collect::collect_ui_prompt`).
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::luau::components"]
pub struct ScriptInteraction {
    pub label: String,
    pub key: KeyCode,
    /// Distância máxima player↔alvo (m).
    pub range: f32,
}

/// Fila de entrada dos eventos engine→Lua (producers → host de scripting).
///
/// O `UiAction` da UI declarativa entra aqui e o host de scripting faz
/// fan-out para as filas por-path (`viber.events()`). Os produtores usam
/// `Option<ResMut>`: apps mínimas de teste sem o recurso continuam a
/// compilar e a correr.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::luau::events"]
pub struct ScriptEventQueue(pub Vec<ScriptGameEvent>);

impl ScriptEventQueue {
    /// Atalho para produtores.
    pub fn push(&mut self, event: ScriptGameEvent) {
        self.0.push(event);
    }
}

/// Evento engine→Lua: estado do jogo que mudou FORA dos scripts (kill, dano
/// ao herói, level-up, cliques de UI…). Pull model: os produtores empurram
/// para [`ScriptEventQueue`]; o host faz fan-out e o script drena com
/// `viber.events()` → array `{type=..., ...}`.
#[derive(Debug, Clone, Reflect)]
#[type_path = "viber::luau::events"]
pub enum ScriptGameEvent {
    /// Criatura morreu. `name` = nome da entidade; `entity` = bits.
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
    /// Algo entrou no vault.
    Collect { item: String, amount: u32 },
    /// Quest entregue com sucesso (recompensas já aplicadas).
    QuestDone { id: String },
    /// Subida de nível (deteção nos vitais).
    LevelUp { level: u32 },
    /// Ação da UI declarativa (`viber.ui.action("buy", "potion")`) — encaminhada
    /// a TODOS os scripts subscritos; os handlers nativos também correm, salvo
    /// as ações reclamadas por `viber.ui.own_action`.
    UiAction { name: String, arg: String },
    /// Uma cinemática acabou (`skipped` = o jogador saltou-a).
    CinematicEnd { id: String, skipped: bool },
    /// A cena acabou de carregar; `entry` = a entrada de chegada (vazio = nenhuma).
    SceneLoaded { scene: String, entry: String },
    /// A cena `scene` vai ser descarregada.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_game_event_kind_names() {
        assert_eq!(
            ScriptGameEvent::UiAction {
                name: "buy".into(),
                arg: "potion".into()
            }
            .kind(),
            "ui_action"
        );
        assert_eq!(ScriptGameEvent::PlayerDied.kind(), "player_died");
    }
}
