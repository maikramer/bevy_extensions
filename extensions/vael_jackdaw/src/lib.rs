//! Extensão do Jackdaw para o Vael: painéis **Ambiente** (`bevy_elements`),
//! **PostFX** e **Flora** (`bevy_flora`), com preview no viewport.
//!
//! As definições são componentes `Reflect` em entidades do `.bsn`
//! (`world/settings/*.bsn`): o inspector edita qualquer campo; estes painéis
//! dão os atalhos (hora, clima, efeitos, espécies) e mostram o resultado no
//! viewport onde é barato — sol de preview, lente da câmara do jogo e
//! árvores reais. Cada escrita passa pelo operador `component.set` do
//! editor, por isso entra no undo e no `.bsn` gravado.

mod camera;
mod environment;
mod flora;
mod postfx;
mod primitives;
mod ui;

use bevy::prelude::*;
use jackdaw_api::prelude::DefaultArea;
use jackdaw_extension::AppState;
use jackdaw_extension::prelude::*;

/// Id da extensão (tem de ser igual ao do manifesto do bundle).
pub const EXTENSION_ID: &str = "vael_jackdaw";

#[derive(Default)]
pub struct VaelExtension;

impl JackdawExtension for VaelExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Vael: Ambiente, PostFX e Flora".to_string()
    }

    fn description(&self) -> String {
        "Definições do Vael na cena: céu/clima (bevy_elements), pós-processamento e \
         espécies de árvore (bevy_flora), com preview no viewport"
            .to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        environment::register(ctx);
        postfx::register(ctx);
        flora::register(ctx);

        ctx.register_window(
            WindowDescriptor::new(environment::WINDOW_ID)
                .with_name("Vael: Ambiente")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(30)
                .with_build(|window| {
                    let parent = window.target_entity();
                    environment::build(window.world_mut(), parent);
                }),
        );
        ctx.register_window(
            WindowDescriptor::new(postfx::WINDOW_ID)
                .with_name("Vael: PostFX")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(31)
                .with_build(|window| {
                    let parent = window.target_entity();
                    postfx::build(window.world_mut(), parent);
                }),
        );
        ctx.register_window(
            WindowDescriptor::new(flora::WINDOW_ID)
                .with_name("Vael: Flora")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(32)
                .with_build(|window| {
                    let parent = window.target_entity();
                    flora::build(window.world_mut(), parent);
                }),
        );
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        // Uma closure, não um `impl Plugin`: o `jd` trata um crate que
        // declara o seu próprio `Plugin` como jogo e não como extensão.
        Some(Box::new(|app: &mut App| {
            app.add_plugins((
                bevy_ext_elements::ElementsSettingsPlugin,
                bevy_ext_postfx::PostFxSettingsPlugin,
                bevy_ext_flora::FloraSettingsPlugin,
            ));
            if !app.is_plugin_added::<bevy_flora::prelude::FloraPlugin>() {
                app.add_plugins(bevy_flora::prelude::FloraPlugin);
            }
            environment::app(app);
            postfx::app(app);
            flora::app(app);
            primitives::app(app);
            camera::app(app);
            app.add_systems(
                Update,
                (environment::refresh, postfx::refresh, flora::refresh)
                    .run_if(in_state(AppState::Editor)),
            );
            app.add_systems(
                Update,
                (camera::measure_focus, camera::apply_world_scale)
                    .chain()
                    .run_if(in_state(AppState::Editor)),
            );
        }))
    }
}
