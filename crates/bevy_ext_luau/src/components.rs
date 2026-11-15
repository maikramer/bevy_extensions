//! Componentes e mensagens da ligação entidade↔script — portados do
//! `viber::luau::components` do monólito (2026-11-15, onda final do
//! DELETE-MONOLITH). Os TypePaths ficam CONGELADOS em
//! `viber::luau::components`: são chaves de save (`harvest::stable_key` via
//! identidade), BRP e reflexão do editor — um tipo que muda de type_path
//! quebraria saves e o inspector.
//!
//! - [`LuaScriptRef`]: o componente que liga uma entidade a um chunk
//!   (`scripts/<path>`), inserido pelo spawn (vive no `bevy_ext_core`; o
//!   TypePath histórico `viber::luau::components` foi fixado lá na G-1).
//! - [`ScriptActivation`]: o "LOD de IA" — raio além do qual o script nem roda.
//! - [`ScriptInteraction`]: alvo de interação registado por script
//!   (`viber.set_interaction`), arbitrado pelo foco de interação do anfitrião
//!   (contrato no core desde exit4/F2).
//! - [`ScriptToast`]: mensagem de toast pedida por `viber.toast` (core).
//! - [`ScriptSay`]: linha falada por `viber.say`, para o jogo a apresentar.

use bevy::prelude::*;

// Os tipos de marca/interação/toast vivem nos contratos do core; este módulo
// re-exporta-os para os caminhos históricos (`viber::luau::components::*` no
// monólito) resolverem num só lugar.
pub use bevy_ext_core::scripting::ScriptInteraction;
pub use bevy_ext_core::{LuaScriptRef, ScriptToast};

/// Uma linha falada por um script (`viber.say`), já resolvida do locale e com
/// a duração final. A engine desenha o balão nativo do HUD por omissão; com
/// um apresentador externo presente, o jogo ouve esta mensagem e apresenta a
/// fala onde quiser (balão no mundo, caixa, legenda).
#[derive(Message, Debug, Clone)]
pub struct ScriptSay {
    /// Entidade que correu o script (quem falou), quando conhecida. Numa
    /// callback chamada de outro script pode ser a entidade do HUD: o jogo
    /// resolve o orador visual e cai para um toast quando não há nenhum.
    pub speaker: Option<Entity>,
    pub text: String,
    /// Duração final em segundos (nunca 0; o default do HUD já resolvido).
    pub seconds: f32,
}

/// Valores por instância dos props exportados pelo script (atributos
/// `prop-<nome>` do nó, sem o prefixo, texto cru). O tipo vem das anotações
/// `--@export` do script ([`crate::exports`]) e é aplicado em `viber.prop`.
#[derive(Debug, Clone, Default, Component, PartialEq, Reflect)]
#[reflect(Component, Default)]
#[type_path = "viber::luau::components"]
pub struct ScriptProps(#[reflect(ignore)] pub std::sync::Arc<Vec<(String, String)>>);

/// Grupos da entidade (`groups="guardas aldeia"`) — `viber.group`,
/// `viber.in_group`, `viber.call_group` e os alvos `@grupo` das conexões.
#[derive(Debug, Clone, Default, Component, PartialEq, Reflect)]
#[reflect(Component, Default)]
#[type_path = "viber::luau::components"]
pub struct Groups(#[reflect(ignore)] pub std::sync::Arc<Vec<String>>);

/// Conexões de sinal do EMISSOR (`connect-<sinal>="alvo:método; …"`): o que
/// um `viber.emit(sinal, …)` desta entidade chama.
#[derive(Debug, Clone, Default, Component, PartialEq, Reflect)]
#[reflect(Component, Default)]
#[type_path = "viber::luau::components"]
pub struct SignalConnections(
    #[reflect(ignore)] pub std::sync::Arc<Vec<(String, Vec<crate::exports::Connection>)>>,
);

impl SignalConnections {
    pub fn of(&self, signal: &str) -> &[crate::exports::Connection] {
        self.0
            .iter()
            .find(|(name, _)| name == signal)
            .map(|(_, conns)| conns.as_slice())
            .unwrap_or(&[])
    }
}

/// Raio de ativação do script ("LOD de IA"): além deste raio do player o
/// `on_update` NEM RODA — inimigo congelado (lógica + animação paradas).
/// Autoria via `activation-radius` no spawner; default 45 m.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component, Default)]
#[type_path = "viber::luau::components"]
pub struct ScriptActivation {
    pub radius: f32,
}

impl Default for ScriptActivation {
    fn default() -> Self {
        Self {
            radius: DEFAULT_ACTIVATION_RADIUS,
        }
    }
}

/// Distância padrão de congelamento total de scripts de criatura (m).
pub const DEFAULT_ACTIVATION_RADIUS: f32 = 45.0;

/// Os componentes de grupos/conexões de um nó (`None` quando não declara).
/// Um `connect-*` mal escrito avisa e cai (o lint aponta-o).
///
/// Genérico sobre os dados crus do nó (grupos + conexões `alvo:método`); o
/// anfitrião tem o tipo autoral (`NodeLinks`) e chama com os campos.
pub fn link_components(
    groups: &[String],
    connections: &[(String, String)],
    label: &str,
) -> (Option<Groups>, Option<SignalConnections>) {
    let groups = (!groups.is_empty()).then(|| Groups(std::sync::Arc::new(groups.to_vec())));
    let mut parsed = Vec::new();
    for (signal, raw) in connections {
        match crate::exports::parse_connections(raw) {
            Ok(conns) if !conns.is_empty() => parsed.push((signal.clone(), conns)),
            Ok(_) => {}
            Err(error) => log::warn!("{label}: connect-{signal}: {error}"),
        }
    }
    let connections = (!parsed.is_empty()).then(|| SignalConnections(std::sync::Arc::new(parsed)));
    (groups, connections)
}
