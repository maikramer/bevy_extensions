//! Painel "Vael: PostFX": efeitos da câmara do jogo e a "lente" no viewport.
//!
//! Os toggles escrevem no `PostFxSettings` da cena (o jogo converte-o nos
//! gates da engine). A lente aplica à câmara do editor os efeitos baratos
//! que existem no Bevy core — bloom, vinheta e aberração cromática — para o
//! viewport ler parecido com o jogo; SSAO, DoF, volumétricos e motion blur
//! ficam para o Play.

use bevy::post_process::bloom::Bloom;
use bevy::post_process::effect_stack::{ChromaticAberration, Vignette};
use bevy::prelude::*;
use jackdaw_extension::prelude::*;
use jackdaw_extension::viewport::MainViewportCamera;

use bevy_ext_postfx::{PostFxSettings, SsaoQuality};

use crate::ui;

pub const WINDOW_ID: &str = "vael.postfx";
const TYPE: &str = "bevy_ext_postfx::PostFxSettings";

#[derive(Component)]
pub(crate) struct PostFxStatus;

/// Lente do jogo na câmara do viewport.
#[derive(Resource, Default)]
pub(crate) struct LensPreview {
    enabled: bool,
}

/// O que a lente pôs na câmara (para só tirar isso).
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
struct LensApplied {
    bloom: bool,
    vignette: bool,
    chromatic: bool,
}

pub(crate) fn app(app: &mut App) {
    app.init_resource::<LensPreview>().add_systems(
        Update,
        drive_lens.run_if(in_state(jackdaw_extension::AppState::Editor)),
    );
}

pub(crate) fn build(world: &mut World, parent: Entity) {
    ui::column(world, parent, |root| {
        ui::heading(root, "Pós-processamento");
        ui::status(root, PostFxStatus);
        ui::label(root, "Efeitos (clicar alterna)");
        ui::row(
            root,
            &[
                ("Bloom", "vael.postfx.toggle_bloom"),
                ("SSAO", "vael.postfx.toggle_ssao"),
                ("TAA", "vael.postfx.toggle_taa"),
                ("Sharpen", "vael.postfx.toggle_cas"),
                ("Exposição auto", "vael.postfx.toggle_autoexposure"),
                ("Sombras contacto", "vael.postfx.toggle_contact_shadows"),
                ("DoF", "vael.postfx.toggle_dof"),
                ("Motion blur", "vael.postfx.toggle_motion_blur"),
                ("Volumétricos", "vael.postfx.toggle_volumetrics"),
                ("Aberração", "vael.postfx.toggle_chromatic"),
                ("Vinheta", "vael.postfx.toggle_vignette"),
                ("Split-tone", "vael.postfx.toggle_splittone"),
                ("Perspetiva aérea", "vael.postfx.toggle_aerial"),
            ],
        );
        ui::label(root, "Qualidade do SSAO");
        ui::row(
            root,
            &[
                ("Low", "vael.postfx.ssao_low"),
                ("Medium", "vael.postfx.ssao_medium"),
                ("High", "vael.postfx.ssao_high"),
                ("Ultra", "vael.postfx.ssao_ultra"),
            ],
        );
        ui::row(root, &[("Lente do jogo no viewport", "vael.postfx.toggle_lens")]);
        ui::label(
            root,
            "A lente mostra bloom, vinheta e aberração no viewport. SSAO, DoF, \
             volumétricos e motion blur só no Play. Grava e o jogo a correr segue.",
        );
    });
}

pub(crate) fn refresh(
    settings: Query<&PostFxSettings>,
    lens: Res<LensPreview>,
    mut status: Query<&mut Text, With<PostFxStatus>>,
) {
    let Ok(mut status) = status.single_mut() else {
        return;
    };
    let text = match settings.iter().next() {
        None => "Sem PostFxSettings nesta cena (abre o world.bsn).".to_string(),
        Some(s) => {
            let effects = s.effects();
            let pick = |want: bool| -> String {
                let names: Vec<&str> = effects
                    .iter()
                    .filter(|(on, _, _)| *on == want)
                    .map(|(_, _, label)| *label)
                    .collect();
                if names.is_empty() {
                    "—".to_string()
                } else {
                    names.join(", ")
                }
            };
            format!(
                "Ligados: {}\nDesligados: {}\nSSAO {} · lente no viewport {}",
                pick(true),
                pick(false),
                s.ssao_quality.name(),
                if lens.enabled { "ligada" } else { "desligada" },
            )
        }
    };
    if status.0 != text {
        status.0 = text;
    }
}

fn drive_lens(
    mut commands: Commands,
    lens: Res<LensPreview>,
    settings: Query<&PostFxSettings>,
    cameras: Query<(Entity, Option<&LensApplied>), With<MainViewportCamera>>,
) {
    let wanted = settings
        .iter()
        .next()
        .filter(|_| lens.enabled)
        .map(|s| LensApplied {
            bloom: s.bloom,
            vignette: s.vignette,
            chromatic: s.chromatic_aberration,
        })
        .unwrap_or_default();
    for (camera, applied) in &cameras {
        let current = applied.copied().unwrap_or_default();
        if current == wanted {
            continue;
        }
        let mut entity = commands.entity(camera);
        match (wanted.bloom, current.bloom) {
            (true, false) => {
                entity.insert(Bloom::NATURAL);
            }
            (false, true) => {
                entity.remove::<Bloom>();
            }
            _ => {}
        }
        match (wanted.vignette, current.vignette) {
            (true, false) => {
                entity.insert(Vignette {
                    intensity: 0.25,
                    ..default()
                });
            }
            (false, true) => {
                entity.remove::<Vignette>();
            }
            _ => {}
        }
        match (wanted.chromatic, current.chromatic) {
            (true, false) => {
                entity.insert(ChromaticAberration {
                    intensity: 0.004,
                    ..default()
                });
            }
            (false, true) => {
                entity.remove::<ChromaticAberration>();
            }
            _ => {}
        }
        entity.insert(wanted);
    }
}

fn toggle(world: &mut World, key: &str, field: &str) -> OperatorResult {
    let Some(entity) = ui::settings_entity::<PostFxSettings>(world) else {
        return ui::missing::<PostFxSettings>(world);
    };
    let current = world
        .get::<PostFxSettings>(entity)
        .and_then(|s| s.effects().into_iter().find(|(_, k, _)| *k == key))
        .map(|(on, _, _)| on)
        .unwrap_or(true);
    ui::set_field(world, entity, TYPE, field, format!("{}", !current))
}

fn set_ssao(world: &mut World, quality: SsaoQuality) -> OperatorResult {
    let Some(entity) = ui::settings_entity::<PostFxSettings>(world) else {
        return ui::missing::<PostFxSettings>(world);
    };
    ui::set_field(world, entity, TYPE, "ssao_quality", format!("\"{quality:?}\""))
}

macro_rules! toggle_op {
    ($name:ident, $id:tt, $label:tt, $key:tt, $field:tt) => {
        #[operator(id = $id, label = $label, allows_undo = false)]
        pub(crate) fn $name(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
            toggle(world, $key, $field)
        }
    };
}

macro_rules! ssao_op {
    ($name:ident, $id:tt, $label:tt, $quality:expr) => {
        #[operator(id = $id, label = $label, allows_undo = false)]
        pub(crate) fn $name(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
            set_ssao(world, $quality)
        }
    };
}

toggle_op!(toggle_bloom, "vael.postfx.toggle_bloom", "PostFX: bloom", "BLOOM", "bloom");
toggle_op!(toggle_ssao, "vael.postfx.toggle_ssao", "PostFX: SSAO", "SSAO", "ssao");
toggle_op!(toggle_taa, "vael.postfx.toggle_taa", "PostFX: TAA", "TAA", "taa");
toggle_op!(toggle_cas, "vael.postfx.toggle_cas", "PostFX: sharpen", "CAS", "sharpen");
toggle_op!(
    toggle_autoexposure,
    "vael.postfx.toggle_autoexposure",
    "PostFX: exposição automática",
    "AUTOEXPOSURE",
    "auto_exposure"
);
toggle_op!(
    toggle_contact_shadows,
    "vael.postfx.toggle_contact_shadows",
    "PostFX: sombras de contacto",
    "CONTACT_SHADOWS",
    "contact_shadows"
);
toggle_op!(
    toggle_dof,
    "vael.postfx.toggle_dof",
    "PostFX: profundidade de campo",
    "DOF",
    "depth_of_field"
);
toggle_op!(
    toggle_motion_blur,
    "vael.postfx.toggle_motion_blur",
    "PostFX: motion blur",
    "MOTION_BLUR",
    "motion_blur"
);
toggle_op!(
    toggle_volumetrics,
    "vael.postfx.toggle_volumetrics",
    "PostFX: volumétricos",
    "VOLUMETRICS",
    "volumetrics"
);
toggle_op!(
    toggle_chromatic,
    "vael.postfx.toggle_chromatic",
    "PostFX: aberração cromática",
    "CHROMATIC",
    "chromatic_aberration"
);
toggle_op!(toggle_vignette, "vael.postfx.toggle_vignette", "PostFX: vinheta", "VIGNETTE", "vignette");
toggle_op!(
    toggle_splittone,
    "vael.postfx.toggle_splittone",
    "PostFX: split-tone",
    "SPLITTONE",
    "split_tone"
);
toggle_op!(
    toggle_aerial,
    "vael.postfx.toggle_aerial",
    "PostFX: perspetiva aérea",
    "AERIAL",
    "aerial"
);

ssao_op!(ssao_low, "vael.postfx.ssao_low", "PostFX: SSAO low", SsaoQuality::Low);
ssao_op!(ssao_medium, "vael.postfx.ssao_medium", "PostFX: SSAO medium", SsaoQuality::Medium);
ssao_op!(ssao_high, "vael.postfx.ssao_high", "PostFX: SSAO high", SsaoQuality::High);
ssao_op!(ssao_ultra, "vael.postfx.ssao_ultra", "PostFX: SSAO ultra", SsaoQuality::Ultra);

#[operator(
    id = "vael.postfx.toggle_lens",
    label = "PostFX: lente do jogo no viewport",
    allows_undo = false
)]
pub(crate) fn toggle_lens(_params: In<OperatorParameters>, mut lens: ResMut<LensPreview>) -> OperatorResult {
    lens.enabled = !lens.enabled;
    OperatorResult::Finished
}

pub(crate) fn register(ctx: &mut ExtensionRegistrar<'_>) {
    ctx.register_operator::<ToggleBloomOp>()
        .register_operator::<ToggleSsaoOp>()
        .register_operator::<ToggleTaaOp>()
        .register_operator::<ToggleCasOp>()
        .register_operator::<ToggleAutoexposureOp>()
        .register_operator::<ToggleContactShadowsOp>()
        .register_operator::<ToggleDofOp>()
        .register_operator::<ToggleMotionBlurOp>()
        .register_operator::<ToggleVolumetricsOp>()
        .register_operator::<ToggleChromaticOp>()
        .register_operator::<ToggleVignetteOp>()
        .register_operator::<ToggleSplittoneOp>()
        .register_operator::<ToggleAerialOp>()
        .register_operator::<SsaoLowOp>()
        .register_operator::<SsaoMediumOp>()
        .register_operator::<SsaoHighOp>()
        .register_operator::<SsaoUltraOp>()
        .register_operator::<ToggleLensOp>();
}
