//! Jackdaw editor extension for the voxel terrain scene components.

mod ops;
mod panel;
mod preview;
mod world_preview;

use bevy::prelude::*;
use jackdaw_api::prelude::DefaultArea;
use jackdaw_extension::AppState;
use jackdaw_extension::prelude::*;

pub use preview::{VoxelPreviewChunk, VoxelPreviewState};

/// Editor extension id, matching the package/manifest id (the loader
/// requires the id the library registers to equal the bundle's manifest id).
pub const EXTENSION_ID: &str = "bevy_voxel_terrain_jackdaw";
/// Dock window with the voxel terrain panel.
pub const WINDOW_ID: &str = "vael.voxel.terrain";

/// The Jackdaw extension: window, operators and menu entries.
#[derive(Default)]
pub struct VoxelTerrainExtension;

impl JackdawExtension for VoxelTerrainExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Vael: Voxel Terrain".to_string()
    }

    fn description(&self) -> String {
        "Terreno 3D do Vael: VoxelTerrain, caves, arcos e pontes".to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        ctx.register_operator::<ops::AddCaveOp>()
            .register_operator::<ops::AddArchOp>()
            .register_operator::<ops::AddBridgeOp>()
            .register_operator::<ops::AddCliffOp>()
            .register_operator::<ops::RebuildPreviewOp>()
            .register_operator::<ops::ToggleModelsOp>()
            .register_operator::<ops::RebuildWorldPreviewOp>()
            .register_operator::<ops::ToggleWorldPreviewOp>();
        ctx.register_menu_entry::<ops::RebuildPreviewOp>(TopLevelMenu::Tools);
        ctx.register_menu_entry::<ops::RebuildWorldPreviewOp>(TopLevelMenu::Tools);
        ctx.register_menu_entry::<ops::ToggleModelsOp>(TopLevelMenu::View);
        ctx.register_menu_entry::<ops::ToggleWorldPreviewOp>(TopLevelMenu::View);

        ctx.register_window(
            WindowDescriptor::new(WINDOW_ID)
                .with_name("Vael: Voxel Terrain")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(35)
                .with_build(|window| {
                    let parent = window.target_entity();
                    panel::build(window.world_mut(), parent);
                }),
        );
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        // A closure, not a `Plugin` impl: jackdaw classifies a crate that
        // declares its own `Plugin` as a game and builds it as a binary
        // instead of an extension dylib.
        Some(Box::new(|app: &mut App| {
            // O editor oficial carrega os `.bsn` sem o `WorldNode` do Vael;
            // sem isto os attrs do DSL não chegam à cena e o preview nativo
            // vê só `Transform`/`GltfSource`.
            app.register_type::<bevy_ext_world::bsn::WorldNode>()
                .register_type::<bevy_ext_world::bsn::WorldProp>();
            app.register_type::<bevy_ext_terrain::VoxelTerrain>()
                .register_type::<bevy_ext_terrain::VoxelCave>()
                .register_type::<bevy_ext_terrain::VoxelChamber>()
                .register_type::<bevy_ext_terrain::VoxelShaft>()
                .register_type::<bevy_ext_terrain::VoxelArch>()
                .register_type::<bevy_ext_terrain::VoxelArchProfile>()
                .register_type::<bevy_ext_terrain::VoxelBridge>()
                .register_type::<bevy_ext_terrain::VoxelBridgeStyle>()
                .register_type::<bevy_ext_terrain::VoxelCliff>();

            app.init_resource::<VoxelPreviewState>()
                .init_resource::<world_preview::ModelPreviewMode>()
                .init_resource::<world_preview::GlbCache>()
                .init_resource::<world_preview::NodeScan>()
                .init_resource::<world_preview::PreviewPolicy>()
                .init_resource::<world_preview::PreviewBook>()
                .init_resource::<world_preview::BoundsCache>()
                .init_resource::<world_preview::PreviewAssets>()
                .init_resource::<world_preview::TerrainPreviews>()
                .init_resource::<world_preview::FarMesh>()
                .init_resource::<world_preview::AutoFrame>()
                .init_resource::<world_preview::PreviewStatus>();
            app.add_systems(
                Update,
                (
                    preview::mark_dirty,
                    preview::rebuild_preview,
                    world_preview::scan_world_nodes,
                    world_preview::refresh_previews,
                    world_preview::poll_bounds,
                    world_preview::poll_models,
                    world_preview::retire_boxes,
                    world_preview::poll_terrain,
                    world_preview::frame_world_once,
                    world_preview::cleanup_previews,
                    world_preview::rebuild_far_mesh,
                    world_preview::update_status,
                )
                    .chain()
                    .run_if(in_state(AppState::Editor)),
            );
            app.add_systems(Update, panel::refresh.run_if(in_state(AppState::Editor)));
            app.add_observer(world_preview::redirect_preview_selection);
        }))
    }
}
