//! Extensão do Jackdaw para o `bevy_ext_grass`: painel **Relva**, categoria
//! no inspector e ícones para os specs de relva.
//!
//! Os dados são componentes `Reflect` em `world/settings/grass.bsn`
//! (`GrassSettingsSpec` + uma lista de `GrassStyleSpec`, uma por bioma). O
//! painel dá atalhos (ligar/desligar, densidade, nova paleta) e cada escrita
//! passa pelo operador `component.set` do editor (undo + `.bsn` gravado); o
//! jogo a correr relê o documento, como nas definições de ambiente/flora.
//!
//! Sem preview de lâminas no viewport nesta versão: o viewport do editor não
//! tem o terreno cozido do jogo; o painel/inspector são a via de autoria.

mod panel;
mod ui;

use bevy::prelude::*;
use jackdaw_api::prelude::DefaultArea;
use jackdaw_extension::AppState;
use jackdaw_extension::inspector::InspectorCategory;
use jackdaw_extension::prelude::*;

/// Id da extensão (tem de ser igual ao do manifesto do bundle).
pub const EXTENSION_ID: &str = "bevy_ext_grass_jackdaw";

#[derive(Default)]
pub struct GrassExtension;

impl JackdawExtension for GrassExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Relva (bevy_ext_grass)".to_string()
    }

    fn description(&self) -> String {
        "Settings e paletas por bioma do bevy_ext_grass: painel Relva, \
         categoria no inspector e ícones dos specs"
            .to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        panel::register(ctx);

        ctx.register_inspector_category(InspectorCategory {
            id: "grass".into(),
            label: "Relva".into(),
            icon: Icon::Trees,
            order: 35,
        })
        .register_component_category::<bevy_ext_grass::GrassSettingsSpec>("grass")
        .register_component_category::<bevy_ext_grass::GrassStyleSpec>("grass")
        .register_entity_icon("bevy_ext_grass::GrassSettingsSpec", Icon::Trees)
        .register_entity_icon("bevy_ext_grass::GrassStyleSpec", Icon::Trees);

        ctx.register_window(
            WindowDescriptor::new(panel::WINDOW_ID)
                .with_name("Relva")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(33)
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
            app.add_plugins(bevy_ext_grass::GrassSettingsSpecPlugin);
            app.add_systems(Update, panel::refresh.run_if(in_state(AppState::Editor)));
        }))
    }
}
