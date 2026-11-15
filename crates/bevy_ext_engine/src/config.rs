//! Config do jogo.yaml — parte extraída do `config.rs` do monólito; o
//! monólito reexporta para `viber::config::ReflectionsConfig` continuar a
//! resolver. O resto da config (GameConfig/AssetsConfig/GameDirs/SaveConfig)
//! continua no monólito.

use serde::Deserialize;

/// Reflexos (SSR) da água e do chão molhado — secção `reflections:` do
/// `game.yaml`.
///
/// O passe (`water_ssr` aqui no grab-bag) faz raymarch screen-space na água
/// e, com chuva, no chão molhado. O chão é o que cobre o ecrã inteiro quando
/// chove; `wet-ground: false` deixa lá o sheen PBR barato (`walls_b.w` no
/// shader do terreno) e reserva o raymarch para a água.
#[derive(Debug, Clone, Default, Deserialize, bevy::prelude::Reflect)]
#[serde(rename_all = "kebab-case")]
#[type_path = "viber::config"]
pub struct ReflectionsConfig {
    /// `auto` (default: SSR só no preset `alto`) | `off` | `water` (só água)
    /// | `full` (água + chão molhado). `VIBER_WATER_SSR` sobrepõe-se.
    #[serde(default)]
    pub mode: Option<String>,
    /// O chão molhado entra no raymarch? `false` é o modo barato.
    #[serde(default)]
    pub wet_ground: Option<bool>,
    /// Passos do raymarch (default 24). Menos passos = reflexo mais curto.
    #[serde(default)]
    pub steps: Option<u32>,
    /// Alcance do raymarch em metros (default 140).
    #[serde(default)]
    pub max_distance: Option<f32>,
}
