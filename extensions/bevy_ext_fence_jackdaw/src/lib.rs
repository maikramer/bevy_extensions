//! Extensão do Jackdaw para o `bevy_ext_fence`: preview das cercas no viewport,
//! painel **Cercas**, categoria no inspector e ícones.
//!
//! O que se autora é um `FenceSpec` (`fence-points`, `fence-style`, …) numa
//! entidade; as predefinições são `FenceStyle` em `world/settings/fences.bsn`.
//! O preview usa o mesmo crate do jogo, pelo que o viewport diz a verdade
//! sobre o aspecto — só não tem o terreno (os postes ficam à cota autorada).

mod panel;
mod preview;
mod ui;

use bevy::prelude::*;
use jackdaw_api::prelude::DefaultArea;
use jackdaw_extension::AppState;
use jackdaw_extension::inspector::InspectorCategory;
use jackdaw_extension::prelude::*;

/// Id da extensão (tem de ser igual ao do manifesto do bundle).
pub const EXTENSION_ID: &str = "bevy_ext_fence_jackdaw";

#[derive(Default)]
pub struct FenceExtension;

impl JackdawExtension for FenceExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Cercas (bevy_ext_fence)".to_string()
    }

    fn description(&self) -> String {
        "Cercas procedurais do bevy_ext_fence: preview no viewport, painel Cercas, \
         categoria no inspector e ícones dos specs"
            .to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        panel::register(ctx);

        ctx.register_inspector_category(InspectorCategory {
            id: "fence".into(),
            label: "Cercas".into(),
            icon: Icon::Fence,
            order: 36,
        })
        .register_component_category::<bevy_ext_fence::FenceSpec>("fence")
        .register_component_category::<bevy_ext_fence::FenceStyle>("fence")
        .register_component_category::<bevy_ext_fence::FenceSettings>("fence")
        .register_entity_icon("bevy_ext_scene::FenceSpec", Icon::Fence);

        ctx.register_window(
            WindowDescriptor::new(panel::WINDOW_ID)
                .with_name("Cercas")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(34)
                .with_build(|window| {
                    let parent = window.target_entity();
                    panel::build(window.world_mut(), parent);
                }),
        );
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        // Uma closure, não um `impl Plugin`: o `jd` trata um crate que
        // declara o seu próprio `Plugin` como jogo e não como extensão.
        Some(Box::new(|app: &mut App| {
            app.add_plugins(bevy_ext_fence::FenceSettingsPlugin);
            preview::app(app);
            app.add_systems(Update, panel::refresh.run_if(in_state(AppState::Editor)));
        }))
    }
}
