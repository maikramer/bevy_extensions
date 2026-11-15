//! Definições e runtime de pós-processamento autorado na cena.
//!
//! [`PostFxSettings`] vive numa entidade do `.bsn` (o Jackdaw edita-a no
//! inspector). A engine converte-a nos mesmos gates do `config.yaml`
//! (`postfx.disabled`/`postfx.ssao`) com [`PostFxSettings::disabled_keys`];
//! o editor aplica os efeitos baratos à câmara do viewport.
//!
//! O **runtime** ([`runtime`]) é a lente da câmara do jogo — exposição, bloom
//! (diurno e nocturno), SSAO, DOF, vinheta, aberração cromática, volumétricos,
//! motion blur, CAS, split-tone e perspetiva aérea — extraída do monólito
//! `viber::postfx` (com [`PostFxState`], [`ExternalExposure`], os perfis de
//! cena `set_scene_profile`/`clear_scene_profile`, o toggle de teclas
//! ([`toggle`]) e o escalonador de qualidade ([`adaptive`])). Os contratos de
//! estado do mundo que a lente lê (atmosfera, sol, biomas, tags) vivem em
//! [`crate::env`] com o TypePath histórico congelado até os crates donos
//! aterrarem.
//!
//! O plugin de cena ([`PostFxSettingsPlugin`]) só registra tipos para o
//! editor; o [`PostFxPlugin`] (em [`runtime`]) monta a lente — pressupõe uma
//! App com `AssetPlugin`/`RenderPlugin` (como qualquer stack `Camera3d`).

use bevy::prelude::*;

pub mod adaptive;
pub mod env;
pub mod preset;
pub mod runtime;
pub mod toggle;

pub use adaptive::{
    AdaptiveQuality, AdaptiveQualityPlugin, DEFAULT_TARGET_FPS, QualityMode, tier_levers, tier_name,
};
pub use runtime::{
    BASE_BLOOM, BASE_EV100, ExternalExposure, MAX_BLOOM, MAX_DARKEN_EV, MAX_KICK_EV,
    NIGHT_BLOOM_INTENSITY, NIGHT_BLOOM_LF_BOOST, NIGHT_BLOOM_THRESHOLD, NIGHT_VIGNETTE,
    AttachCameras, AerialPass, FxDrive, PostFxPlugin, PostFxState, QualityGates,
    clear_scene_profile, fx_forced_off, fx_gate_generation, fx_off, fx_runtime_toggle,
    resolve_profile, set_quality_gates, set_scene_profile,
};
pub use toggle::{DEFAULT_BINDINGS, PostFxDebugTogglePlugin, PostFxToggleBindings, parse_bindings};

/// Que efeitos a câmara do jogo paga e com que qualidade de SSAO.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_postfx"]
pub struct PostFxSettings {
    /// Brilho do que passa do branco (fogueiras, emissivos, disco solar).
    pub bloom: bool,
    /// Oclusão de ambiente em ecrã: assenta objetos no chão.
    pub ssao: bool,
    pub ssao_quality: SsaoQuality,
    /// Antialiasing temporal (limpa o ruído das sombras/SSAO).
    pub taa: bool,
    /// Sharpening adaptativo depois do AA.
    pub sharpen: bool,
    /// Exposição automática da engine (desligada quando o céu dá o EV).
    pub auto_exposure: bool,
    /// Sombras de contacto em ecrã.
    pub contact_shadows: bool,
    /// Profundidade de campo com foco no herói.
    pub depth_of_field: bool,
    pub motion_blur: bool,
    /// God-rays (raymarch volumétrico, caro).
    pub volumetrics: bool,
    pub chromatic_aberration: bool,
    pub vignette: bool,
    /// Sombras frias / altas luzes quentes pela hora.
    pub split_tone: bool,
    /// Perspetiva aérea (névoa por distância em ecrã).
    pub aerial: bool,
}

impl Default for PostFxSettings {
    /// A stack completa da engine (o mesmo que um `config.yaml` sem `postfx`).
    fn default() -> Self {
        Self {
            bloom: true,
            ssao: true,
            ssao_quality: SsaoQuality::High,
            taa: true,
            sharpen: true,
            auto_exposure: true,
            contact_shadows: true,
            depth_of_field: true,
            motion_blur: true,
            volumetrics: true,
            chromatic_aberration: true,
            vignette: true,
            split_tone: true,
            aerial: true,
        }
    }
}

/// Qualidade do SSAO (amostras por pixel).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_postfx"]
pub enum SsaoQuality {
    Low,
    Medium,
    #[default]
    High,
    Ultra,
}

impl SsaoQuality {
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }
}

impl PostFxSettings {
    /// Cada efeito, a chave do gate da engine (`VIBER_NO_<KEY>`) e o rótulo.
    pub fn effects(&self) -> [(bool, &'static str, &'static str); 13] {
        [
            (self.bloom, "BLOOM", "Bloom"),
            (self.ssao, "SSAO", "SSAO"),
            (self.taa, "TAA", "TAA"),
            (self.sharpen, "CAS", "Sharpen (CAS)"),
            (self.auto_exposure, "AUTOEXPOSURE", "Exposição automática"),
            (
                self.contact_shadows,
                "CONTACT_SHADOWS",
                "Sombras de contacto",
            ),
            (self.depth_of_field, "DOF", "Profundidade de campo"),
            (self.motion_blur, "MOTION_BLUR", "Motion blur"),
            (self.volumetrics, "VOLUMETRICS", "Volumétricos"),
            (
                self.chromatic_aberration,
                "CHROMATIC",
                "Aberração cromática",
            ),
            (self.vignette, "VIGNETTE", "Vinheta"),
            (self.split_tone, "SPLITTONE", "Split-tone"),
            (self.aerial, "AERIAL", "Perspetiva aérea"),
        ]
    }

    /// Chaves dos gates desligados (vocabulário de `fx_off` da engine).
    pub fn disabled_keys(&self) -> Vec<&'static str> {
        self.effects()
            .into_iter()
            .filter(|(on, _, _)| !on)
            .map(|(_, key, _)| key)
            .collect()
    }

    /// Liga/desliga um efeito pela chave do gate. Devolve `false` se a chave
    /// não existe.
    pub fn set_effect(&mut self, key: &str, on: bool) -> bool {
        let slot = match key {
            "BLOOM" => &mut self.bloom,
            "SSAO" => &mut self.ssao,
            "TAA" => &mut self.taa,
            "CAS" => &mut self.sharpen,
            "AUTOEXPOSURE" => &mut self.auto_exposure,
            "CONTACT_SHADOWS" => &mut self.contact_shadows,
            "DOF" => &mut self.depth_of_field,
            "MOTION_BLUR" => &mut self.motion_blur,
            "VOLUMETRICS" => &mut self.volumetrics,
            "CHROMATIC" => &mut self.chromatic_aberration,
            "VIGNETTE" => &mut self.vignette,
            "SPLITTONE" => &mut self.split_tone,
            "AERIAL" => &mut self.aerial,
            _ => return false,
        };
        *slot = on;
        true
    }
}

/// Regista os tipos para reflexão (jogo e editor).
pub struct PostFxSettingsPlugin;

impl Plugin for PostFxSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<PostFxSettings>()
            .register_type::<SsaoQuality>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_nao_desliga_nada() {
        assert!(PostFxSettings::default().disabled_keys().is_empty());
    }

    #[test]
    fn chaves_batem_com_set_effect() {
        let mut settings = PostFxSettings::default();
        for (_, key, _) in PostFxSettings::default().effects() {
            assert!(settings.set_effect(key, false), "{key}");
        }
        assert_eq!(settings.disabled_keys().len(), 13);
    }
}
