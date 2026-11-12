//! Jackdaw editor extension for the voxel terrain scene components.

mod ops;
mod panel;
mod preview;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_editor_core::prelude::*;

pub use preview::{VoxelPreviewChunk, VoxelPreviewState};

/// Editor extension id/name shown in the Jackdaw extension list.
pub const EXTENSION_ID: &str = "vael.voxel";
/// Dock window with the voxel terrain panel.
pub const WINDOW_ID: &str = "vael.voxel.terrain";

/// Registers the voxel terrain types and the preview systems.
pub struct VoxelTerrainEditorPlugin;

impl Plugin for VoxelTerrainEditorPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<bevy_ext_terrain::VoxelTerrain>()
            .register_type::<bevy_ext_terrain::VoxelCave>()
            .register_type::<bevy_ext_terrain::VoxelChamber>()
            .register_type::<bevy_ext_terrain::VoxelShaft>()
            .register_type::<bevy_ext_terrain::VoxelArch>()
            .register_type::<bevy_ext_terrain::VoxelArchProfile>()
            .register_type::<bevy_ext_terrain::VoxelBridge>()
            .register_type::<bevy_ext_terrain::VoxelBridgeStyle>();

        app.init_resource::<VoxelPreviewState>();
        app.add_systems(
            Update,
            (preview::mark_dirty, preview::rebuild_preview)
                .chain()
                .run_if(in_state(jackdaw_editor_core::AppState::Editor)),
        );
        app.add_systems(
            Update,
            panel::refresh.run_if(in_state(jackdaw_editor_core::AppState::Editor)),
        );
    }
}

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
        ctx.register_operator::<ops::AddVoxelFeatureOp>()
            .register_operator::<ops::RebuildVoxelPreviewOp>();
        ctx.register_menu_entry::<ops::RebuildVoxelPreviewOp>(TopLevelMenu::Tools);

        ctx.register_window(
            WindowDescriptor::new(WINDOW_ID)
                .with_name("Vael: Voxel Terrain")
                .with_default_area(DefaultArea::RightDock)
                .with_priority(35)
                .with_build(|window| {
                    let parent = window.target_entity();
                    panel::build(window.world_mut(), parent);
                }),
        );
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        Some(Box::new(VoxelTerrainEditorPlugin))
    }
}
