//! Pós-processamento — SHIM sobre o crate `bevy_ext_postfx` (exit3/wave-c).
//!
//! A lente inteira (gates, exposição/bloom, split-tone, keys da lua/dourada,
//! tochas, DOF, névoa volumétrica, perspetiva aérea, o passe e os seus
//! shaders inline) vive no crate; este módulo reexporta a superfície
//! (`runtime` + `env` + raiz) — os consumidores internos (`app.rs`,
//! `luau/fx.rs`, `bridge`, `adaptive_quality.rs`) e o caminho
//! `viber::postfx::*` continuam a resolver para O MESMO tipo.
//!
//! O que aqui ficou é o glue que o crate não pode ter (os TODO(viber-exit)
//! dele nomeiam este arquivo):
//!
//! * [`resolve_profile`] — converte o `PostFxConfig` do `game.yaml`
//!   (`crate::config`, serde) para o contrato do crate;
//! * [`publish_cinematic_focus`] — copia o foco de DOF do playback de
//!   cinemáticas para o recurso [`PostFxCinematicFocus`] que o
//!   `drive_dof_focus` do crate lê (o tipo do playback não foi extraído);
//! * [`swap_taa_for_dlss`] (feature `dlss`) — troca o AA da câmara pelo
//!   upscale NGX depois de [`attach_postfx_to_cameras`] equipar a lente;
//!   o `DlssSuperResolutionSupported` e o config do jogo são do integrador.

pub use bevy_ext_postfx::*;
pub use bevy_ext_postfx::env::*;
pub use bevy_ext_postfx::runtime::*;

use bevy::prelude::*;

use crate::cinematic::CinematicPlayback;

/// Perfil do jogo: [`resolve_profile`] do crate recebe o contrato
/// (`PostFxConfig` do crate); o `game.yaml` é lido para o `crate::config`
/// (serde com doc-comments próprios) — a conversão é a única cola.
pub fn resolve_profile(config: Option<&crate::config::PostFxConfig>) {
    let converted = config.map(|config| PostFxConfig {
        disabled: config.disabled.clone(),
        ssao: config.ssao.clone(),
    });
    bevy_ext_postfx::resolve_profile(converted.as_ref());
}

/// Copia o foco de DOF da cinemática corrente para o contrato do crate.
///
/// Um sistema de uma linha: depois do [`crate::cinematic::tick_cinematic`]
/// (que move o playback) e antes do [`FxDrive`] (que lê), para o
/// `drive_dof_focus` ver o foco do MESMO frame — o que o monólito fazia lendo
/// o playback directamente.
pub fn publish_cinematic_focus(
    cinematic: Option<Res<CinematicPlayback>>,
    mut focus: ResMut<PostFxCinematicFocus>,
) {
    focus.0 = cinematic.as_deref().and_then(|c| c.focus());
}

/// DLSS (feature `dlss`): troca o AA que [`attach_postfx_to_cameras`] equipou
/// pelo upscale NGX nas câmaras de janela.
///
/// O crate insere TAA (ou FXAA) incondicionalmente — o pedido DLSS e o
/// `DlssSuperResolutionSupported` são do integrador (config.yaml + feature),
/// por isso a troca corre DEPOIS dele: tira o AA raster (TAA/FXAA — o jitter
/// e o `ShadowFilteringMethod::Temporal` continuam, o bevy injeta-os a partir
/// do componente `Dlss`) e insere o `Dlss`. Sem suporte na máquina (sem
/// RTX/NGX/Vulkan) avisa UMA vez e deixa o TAA — a câmara nunca fica sem AA.
#[cfg(feature = "dlss")]
pub fn swap_taa_for_dlss(
    mut commands: Commands,
    dlss_supported: Option<Res<bevy::anti_alias::dlss::DlssSuperResolutionSupported>>,
    config: Option<Res<crate::config::GameConfig>>,
    cameras: Query<
        (
            Entity,
            &bevy::camera::RenderTarget,
        ),
        (
            With<Camera3d>,
            // Só câmaras já equipadas pela lente (o attach corre antes).
            With<bevy::post_process::bloom::Bloom>,
            Without<bevy::anti_alias::dlss::Dlss<
                bevy::anti_alias::dlss::DlssSuperResolutionFeature,
            >>,
        ),
    >,
    mut warned: Local<bool>,
) {
    let setting = crate::dlss::requested(
        config
            .as_deref()
            .and_then(|config| config.dlss.as_deref()),
    );
    let Some(mode) = crate::dlss::quality_mode(setting) else {
        return;
    };
    let Some(_) = dlss_supported.as_deref() else {
        if !*warned {
            warn!(
                "dlss: pedido {} mas sem suporte nesta máquina \
                 (RTX/NGX/backend Vulkan) — TAA",
                setting.label()
            );
            *warned = true;
        }
        return;
    };
    for (camera, target) in &cameras {
        // Só câmaras de JANELA (a lente não equipa offscreen).
        if !matches!(target, bevy::camera::RenderTarget::Window(_)) {
            continue;
        }
        info!(
            "dlss: {} ativo (substitui o TAA; VIBER_DLSS/config.yaml desligam)",
            setting.label()
        );
        commands
            .entity(camera)
            .insert((
                bevy::anti_alias::dlss::Dlss::<
                    bevy::anti_alias::dlss::DlssSuperResolutionFeature,
                > {
                    perf_quality_mode: mode,
                    ..Default::default()
                },
                // O filtro temporal das sombras (espiral Jimenez) mantém-se.
                bevy::light::ShadowFilteringMethod::Temporal,
            ))
            // O AA raster que o attach inseriu (TAA ou FXAA) sai — o DLSS faz
            // o seu próprio anti-alias temporal.
            .remove::<(
                bevy::anti_alias::taa::TemporalAntiAliasing,
                bevy::anti_alias::fxaa::Fxaa,
            )>();
    }
}
