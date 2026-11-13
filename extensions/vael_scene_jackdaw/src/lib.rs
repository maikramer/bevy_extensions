//! Extensão do Jackdaw para a cena do Vael.
//!
//! - **Outliner**: ícones por tipo de nó (árvore, casa, luz, portal, spawner…);
//! - **Inspector**: `ColliderSpec`/`BodySpec` em *Physics*, `Destructible`/
//!   `Harvestable` em *Gameplay*, o `WorldNode` (DSL restante) em *DSL Vael*;
//! - **Viewport**: gizmos de colisores (seleção ou perto da câmara), anéis de
//!   exclusão/portais/luzes e regiões de spawners;
//! - **Painel "Vael: Cena"**: contagens, o que ainda está em DSL e problemas
//!   (modelos/malhas em falta, valores sem sentido) com "próximo problema".
//!
//! Tudo é lido por reflexão (type path): os tipos chegam pelo schema do jogo.

mod gizmos;
mod index;
mod panel;
mod placement;
mod validate;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_api::prelude::DefaultArea;
use jackdaw_extension::AppState;
use jackdaw_extension::inspector::InspectorCategory;
use jackdaw_extension::prelude::*;

/// Id da extensão (tem de ser igual ao do manifesto do bundle).
pub const EXTENSION_ID: &str = "vael_scene_jackdaw";

/// Só o caminho de tipo do `WorldNode` (para o mapear a uma categoria sem
/// depender do `bevy_ext_world`).
#[derive(TypePath)]
#[type_path = "bevy_world_xml::bsn"]
#[type_name = "WorldNode"]
struct WorldNodePath;

#[derive(Default)]
pub struct VaelSceneExtension;

impl JackdawExtension for VaelSceneExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Vael: Cena".to_string()
    }

    fn description(&self) -> String {
        "Ícones, categorias do inspector, gizmos e verificações da cena do Vael".to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        ctx.register_operator::<gizmos::CycleGizmosOp>()
            .register_operator::<panel::NextProblemOp>()
            .register_operator::<validate::ValidateAndSaveOp>()
            .register_operator::<placement::PlaceOp>()
            .register_operator::<placement::BrushOp>()
            .register_operator::<placement::StopOp>()
            .register_operator::<placement::RadiusUpOp>()
            .register_operator::<placement::RadiusDownOp>()
            .register_operator::<placement::DensityUpOp>()
            .register_operator::<placement::DensityDownOp>()
            .register_operator::<placement::PlaceAtOp>();
        ctx.register_menu_entry::<validate::ValidateAndSaveOp>(TopLevelMenu::File);
        ctx.register_menu_entry::<gizmos::CycleGizmosOp>(TopLevelMenu::View);
        ctx.register_menu_entry::<panel::NextProblemOp>(TopLevelMenu::Tools);
        ctx.register_viewport_overlay::<gizmos::CycleGizmosOp>("Vael: gizmos da cena", true);

        ctx.register_inspector_category(InspectorCategory {
            id: "gameplay".into(),
            label: "Gameplay".into(),
            icon: Icon::Swords,
            order: 25,
        })
        .register_inspector_category(InspectorCategory {
            id: "vael_dsl".into(),
            label: "DSL Vael".into(),
            icon: Icon::Type,
            order: 60,
        })
        .register_component_category::<bevy_ext_scene::ColliderSpec>("physics")
        .register_component_category::<bevy_ext_scene::BodySpec>("physics")
        .register_component_category::<bevy_ext_scene::Destructible>("gameplay")
        .register_component_category::<bevy_ext_scene::Harvestable>("gameplay")
        .register_component_category::<WorldNodePath>("vael_dsl");

        ctx.register_entity_icon_predicate(index::icon_predicate);

        ctx.register_window(
            WindowDescriptor::new(placement::WINDOW_ID)
                .with_name("Vael: Assets")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(28)
                .with_build(|window| {
                    let parent = window.target_entity();
                    placement::build(window.world_mut(), parent);
                }),
        );
        ctx.register_window(
            WindowDescriptor::new(panel::WINDOW_ID)
                .with_name("Vael: Cena")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(29)
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
            app.init_resource::<index::SceneIndex>()
                .init_resource::<gizmos::SceneGizmos>()
                .init_resource::<panel::ProblemCursor>()
                .init_resource::<validate::LastValidation>()
                .add_observer(validate::on_saved)
                .init_resource::<placement::AssetLibrary>()
                .init_resource::<placement::Ground>()
                .add_observer(placement::on_click)
                .add_systems(
                    Update,
                    (
                        index::scan,
                        panel::refresh,
                        gizmos::draw,
                        placement::refresh_catalog,
                        placement::refresh_panel,
                        placement::drive,
                    )
                        .chain()
                        .run_if(in_state(AppState::Editor)),
                );
        }))
    }
}
