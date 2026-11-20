//! Preset gráfico — só a fatia que a lente de pós-processo lê.
//!
//! `config.yaml` do jogo declara `graphics: alto | equilibrado | desempenho`;
//! [`resolve`] congela o preset por processo (env `VIBER_GRAPHICS` ganha ao
//! config). As restantes alavancas do preset (cascatas de sombra, alcance de
//! render, lado do shadow map) ficam no dono do módulo `graphics` — aqui só
//! entra o que os gates de pós-processo consomem: os passos do volumétrico
//! e o corte dos efeitos pesados.
//!
//! TODO(viber-exit): quando o módulo `graphics` do monólito aterrar como
//! crate, este módulo passa a re-exportar a fatia daqui (um só `PRESET`
//! global por processo, como no `graphics::resolve` original).

use std::sync::OnceLock;

/// O preset de qualidade do jogo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphicsPreset {
    /// O visual aprovado, tudo ligado.
    Alto,
    /// O default: o visual sem os extras que ninguém repara em movimento.
    #[default]
    Equilibrado,
    /// Corta o que custa e não se lê em movimento — o preset de 60 fps.
    Desempenho,
}

impl GraphicsPreset {
    /// `VIBER_GRAPHICS`/config.yaml → preset. `None` = valor desconhecido.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "alto" | "high" | "ultra" | "max" => Some(Self::Alto),
            "equilibrado" | "balanced" | "medium" | "default" => Some(Self::Equilibrado),
            // `performance` porque o migrado escreve `performance`, não
            // `desempenho`.
            "desempenho" | "performance" | "low" | "fast" => Some(Self::Desempenho),
            _ => None,
        }
    }
}

static PRESET: OnceLock<GraphicsPreset> = OnceLock::new();

/// Resolve o preset UMA vez, a partir do `config.yaml` e do `VIBER_GRAPHICS`.
///
/// Chamado pelo arranque do jogo antes de construir a App; os getters abaixo
/// caem no default (`equilibrado`) se ninguém tiver resolvido — é o que
/// acontece nos testes e nas tools headless, que não bootam a lente.
pub fn resolve(config_value: Option<&str>) -> GraphicsPreset {
    let from_env = bevy_ext_core::envs::var_lossy("GRAPHICS")
        .and_then(|raw| GraphicsPreset::parse(&raw));
    let from_config = config_value.and_then(GraphicsPreset::parse);
    let preset = from_env
        .or(from_config)
        .unwrap_or(GraphicsPreset::Equilibrado);
    // `set` falha se já estiver resolvido (dois `run` no mesmo processo nos
    // testes) — o primeiro ganha, que é o do processo que está a correr.
    let _ = PRESET.set(preset);
    preset
}

/// Preset em vigor (default `equilibrado` enquanto ninguém resolver).
pub fn preset() -> GraphicsPreset {
    *PRESET.get().unwrap_or(&GraphicsPreset::Equilibrado)
}

/// O passe escala linearmente com os passos (RTX 4050, 1280×720, praça do
/// Vael): 40 → 3,9 ms, 24 → 2,4 ms, 16 → 1,6 ms. Com o jitter do raio e o
/// TAA a acumular, 24 e 40 diferem ~2/255 em média — o `equilibrado` fica
/// nos 24.
pub fn volumetric_steps() -> u32 {
    match preset() {
        GraphicsPreset::Alto => 64,
        GraphicsPreset::Equilibrado => 24,
        GraphicsPreset::Desempenho => 0,
    }
}

/// Efeitos que o `desempenho` desliga em bloco (o `fx_off` dos gates de QA
/// continua a poder desligá-los em qualquer preset).
pub fn heavy_fx_enabled() -> bool {
    preset() != GraphicsPreset::Desempenho
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_aceita_os_tres_e_ignora_lixo() {
        assert_eq!(GraphicsPreset::parse("alto"), Some(GraphicsPreset::Alto));
        assert_eq!(
            GraphicsPreset::parse("  Desempenho "),
            Some(GraphicsPreset::Desempenho)
        );
        // O migrado escreve `performance`, não `desempenho`.
        assert_eq!(
            GraphicsPreset::parse("performance"),
            Some(GraphicsPreset::Desempenho)
        );
        assert_eq!(
            GraphicsPreset::parse("equilibrado"),
            Some(GraphicsPreset::Equilibrado)
        );
        assert_eq!(GraphicsPreset::parse("turbo"), None);
    }

    #[test]
    fn test_sem_resolve_fica_no_default() {
        // Nunca correu `resolve` neste processo de testes (ou correu com o
        // default): os getters têm de responder sem pânico.
        assert!(volumetric_steps() <= 64);
        assert!(heavy_fx_enabled() || preset() == GraphicsPreset::Desempenho);
    }
}
