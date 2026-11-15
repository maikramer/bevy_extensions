//! Host contracts: the types the declarative UI shares with the rest of the
//! engine (locale, luau, hud, scene, world state).
//!
//! These used to live in other `viber` modules. Until each domain gets its
//! own extracted crate, the *definitions* live here and the host app
//! re-exports them (`viber::locale::Locale = bevy_ext_ui::ui::host::Locale`,
//! …), keeping every historical path — and every `#[type_path]` — intact.
//! // TODO(viber-exit): cada tipo abaixo indica o seu destino quando o domínio
//! // for extraído; o host deixa de depender deste crate para os definir.

use std::sync::Arc;

use bevy::prelude::*;
use mlua::Lua;

use bevy_ext_world::xml::NodeOrigin;

// ── locale ───────────────────────────────────────────────────────────────

/// Textos do jogo por idioma (`locale/<lang>.json`, mapa chave → string).
/// Uma string `@chave` resolve; sem entrada, fica a string original para o
/// jogo em PT não partir. `VIBER_LANG` escolhe o idioma (default `pt`).
///
/// Definição CANÓNICA no `bevy_ext_core::locale` (onda exit2/luau; TypePath
/// `viber::locale` congelado lá) — o jogo e a UI leem O MESMO recurso: este
/// módulo reexporta o tipo, o monólito (`viber::locale`) também.
pub use bevy_ext_core::locale::Locale;

// ── luau (contratos do host de scripting) ────────────────────────────────

// Os CONTRATOS dos eventos e da interação vivem agora no `bevy_ext_core`
// (exit4/F2, 2026-11-15 — os TODO(viber-exit) "mover para o crate de luau"
// fecham aqui): a UI empurra `UiAction` para A MESMA fila que o host de
// scripting drena, e o `collect` lê O MESMO componente que `set_interaction`
// escreve — com a definição na base do grafo, nem a UI dep o crate de luau
// nem o luau dep a UI para os nomear. Os TypePaths históricos
// (`viber::luau::events`/`viber::luau::components`) ficam congelados no
// core; o `to_table` (o lado Lua) acompanha o host — hoje o monólito, no
// `bevy_ext_luau` o port da F2.
pub use bevy_ext_core::scripting::{ScriptEventQueue, ScriptGameEvent, ScriptInteraction};

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
