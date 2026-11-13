//! Host contracts: the types the declarative UI shares with the rest of the
//! engine (locale, luau, hud, scene, world state).
//!
//! These used to live in other `viber` modules. Until each domain gets its
//! own extracted crate, the *definitions* live here and the host app
//! re-exports them (`viber::locale::Locale = bevy_ext_ui::ui::host::Locale`,
//! …), keeping every historical path — and every `#[type_path]` — intact.
//! // TODO(viber-exit): cada tipo abaixo indica o seu destino quando o domínio
//! // for extraído; o host deixa de depender deste crate para os definir.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use mlua::Lua;

use bevy_ext_world::xml::NodeOrigin;

// ── locale ───────────────────────────────────────────────────────────────

/// Textos do jogo por idioma (`locale/<lang>.json`, mapa chave → string).
/// Uma string `@chave` resolve; sem entrada, fica a string original para o
/// jogo em PT não partir. `VIBER_LANG` escolhe o idioma (default `pt`).
///
/// // TODO(viber-exit): mover para o futuro crate de locale; o `TypePath`
/// // histórico está congelado para os saves/BRP não mudarem de mão.
#[derive(Resource, Clone, Debug, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::locale"]
pub struct Locale {
    pub lang: String,
    pub table: HashMap<String, String>,
    /// Pasta dos `locale/<lang>.json` (para a troca em runtime).
    pub dir: std::path::PathBuf,
}

impl Locale {
    /// Resolve uma chave `@nome` contra a tabela; sem entrada (ou sem `@`),
    /// devolve o texto original — o mesmo contrato de `dialogue::localized`.
    pub fn resolve(&self, text: &str) -> String {
        let Some(key) = text.strip_prefix('@') else {
            return text.to_string();
        };
        self.table
            .get(key)
            .cloned()
            .unwrap_or_else(|| text.to_string())
    }

    /// Carrega `dir/<lang>.json`. Um ficheiro ausente deixa a tabela vazia
    /// (as strings originais ficam) — o jogo não parte.
    pub fn load(dir: &std::path::Path, lang: &str) -> Self {
        let path = dir.join(format!("{lang}.json"));
        let table = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<HashMap<String, String>>(&text).ok())
            .unwrap_or_default();
        Self {
            lang: lang.to_string(),
            table,
            dir: dir.to_path_buf(),
        }
    }

    /// Troca o idioma sem reiniciar.
    pub fn set_lang(&mut self, lang: &str) {
        if lang.is_empty() || lang == self.lang {
            return;
        }
        *self = Self::load(&self.dir, lang);
    }
}

// ── luau (contratos do host de scripting) ────────────────────────────────

/// Alvo de interação registado por script (`viber.set_interaction`) — o
/// prompt "[tecla] label" aparece quando o player está perto. A UI publica o
/// mais próximo em alcance (`ui::collect::collect_ui_prompt`).
///
/// // TODO(viber-exit): mover para o crate de luau quando for extraído;
/// // `TypePath` histórico `viber::luau::components` congelado.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::luau::components"]
pub struct ScriptInteraction {
    pub label: String,
    pub key: KeyCode,
    /// Distância máxima player↔alvo (m).
    pub range: f32,
}

/// Fila de entrada dos eventos engine→Lua (producers → `luau_update`).
///
/// O `UiAction` desta UI entra aqui e o host de scripting faz fan-out para
/// as filas por-path (`viber.events()`). Os produtores usam `Option<ResMut>`:
/// apps mínimas de teste sem o recurso continuam a compilar e a correr.
///
/// // TODO(viber-exit): mover para o crate de luau (ao lado do `to_table`,
/// // que fica do lado Lua); `TypePath` histórico congelado.
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
///
/// // TODO(viber-exit): mover para o crate de luau; o `ScriptGameEvent::kind`
/// // (o `type` em Lua) acompanha a variante.
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

/// O handle Luau que a UI usa para instalar a tabela `viber.ui`.
///
/// O host de scripting (dono da VM) publica uma COPIA do mesmo handle
/// (`UiScriptHost(host.lua.clone())` — `mlua::Lua` é `Arc` por dentro, as
/// duas cópias partilham a MESMA VM) e o jogo pinha
/// [`install_ui_script_api`](super::install_ui_script_api) antes do
/// `luau_update`. `Option<ResMut>`: apps mínimas de teste sem host não o têm.
///
/// // TODO(viber-exit): quando o crate de luau extrair o `LuaScriptHost`,
/// // este contracto pode fundir-se lá (ou o host passa a inserir este
/// // newtype directamente do plugin dele).
#[derive(Resource, Clone)]
pub struct UiScriptHost(pub Lua);

// ── hud (marcadores partilhados com o HUD nativo) ────────────────────────

/// HUD suprimido por script (`viber.ui.set_visible("hud", false)`).
///
/// O `#hud` declarativo é só uma parte do HUD: minimapa, bússola e seta são
/// widgets NATIVOS. O título/morte escondem o HUD inteiro; sem isto os
/// widgets nativos ficavam por cima do véu.
///
/// // TODO(viber-exit): mover para o crate do HUD; TypePath histórico congelado.
#[derive(Resource, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::hud"]
pub struct HudHidden(pub bool);

/// Radius of the minimap in world meters (authored `range` attribute).
/// A UI re-assenta o `bottom` do minimapa nas janelas estreitas (`ui::scale`).
///
/// // TODO(viber-exit): mover para o crate do HUD; TypePath histórico congelado.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect)]
#[reflect(Component)]
#[type_path = "viber::hud::minimap"]
pub struct MinimapRange(pub f32);

/// Fonte de TEXTO da UI declarativa (`UiText`, tooltips, listas). O host a
/// insere no arranque — tipicamente a mesma fonte do HUD (`HudAssets` do
/// Viber): `UiFont(hud.font.clone())`. Sem o recurso, o rebuild de listas e
/// `viber.ui.create` caem na fonte omissa da Bevy (`Handle::default()`) e os
/// tooltips não aparecem (o antigo `Option<Res<HudAssets>>` tinha o mesmo
/// efeito).
#[derive(Resource, Clone, Debug, Default, Reflect)]
#[reflect(Resource)]
pub struct UiFont(pub Handle<Font>);

impl UiFont {
    /// A fonte do host, ou a omissa da Bevy (handle default) para apps
    /// mínimas que não a publicam — nunca panica.
    pub fn get_or_default(world: &World) -> Handle<Font> {
        world
            .get_resource::<Self>()
            .map(|font| font.0.clone())
            .unwrap_or_default()
    }
}

// ── scene (proveniência) ─────────────────────────────────────────────────

/// O nó XML que declarou uma entidade viva (posto pelo spawn a partir de
/// `EntitySpec::source`). A árvore declarativa carimba-o em cada elemento
/// para o editor e o `viber.scene.entities` os endereçarem como qualquer
/// outro nó.
///
/// // TODO(viber-exit): mover para o crate de cena/mundo que extrair o
/// // `EntitySpec`; TypePath histórico congelado.
#[derive(Component, Debug, Clone, PartialEq, Reflect)]
#[reflect(Component)]
#[type_path = "viber::scene"]
pub struct SceneSource(#[reflect(ignore)] pub Arc<NodeOrigin>);

// ── estado do mundo publicado pelo host ──────────────────────────────────

/// O retrato do MUNDO que os binds do HUD leem — publicado pelo host a cada
/// frame (ou só quando muda): relógio do `<DayCycle>`, chuva do `<Weather>`,
/// zona activa (`<BiomeRegion>` actual do herói) e o `display-name` dessa
/// região. A UI é consumidora: os tipos-fonte (`DayCycleState`,
/// `WeatherState`, `CurrentBiome`/`BiomeRegions`) continuam no host, que faz
/// o espelho (um sistema de 10 linhas por campo) — sem duplicar o estado do
/// mundo neste crate.
///
/// Campos `Option`/zero = mundo sem esse sistema (o bind fica no omissão,
/// como antes quando o recurso não existia).
#[derive(Resource, Debug, Clone, Default, Reflect)]
#[reflect(Resource)]
pub struct UiWorldState {
    /// Minuto do dia do relógio do mundo (`DayCycleState::minute_of_day`);
    /// `None` = mundo sem `<DayCycle>` (sem bind de relógio).
    pub minute_of_day: Option<f32>,
    /// Intensidade de chuva contínua 0..1 (`WeatherState::rain`).
    pub rain: f32,
    /// Id da região de bioma activa (`CurrentBiome::id`); `None` = vale central.
    pub zone: Option<String>,
    /// `display-name` autoral DA região activa (já resolvido pelo host a
    /// partir das `BiomeRegions`); vazio = derivar o título do id.
    pub zone_display: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_resolves_keys_and_keeps_unknown_text() {
        let mut locale = Locale::default();
        locale
            .table
            .insert("menu.title".into(), "Missões".into());
        assert_eq!(locale.resolve("@menu.title"), "Missões");
        // Sem entrada, o texto original fica (o jogo em PT não parte).
        assert_eq!(locale.resolve("@missing.key"), "@missing.key");
        // Sem `@` nem passa pela tabela.
        assert_eq!(locale.resolve("100/100"), "100/100");
    }

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
