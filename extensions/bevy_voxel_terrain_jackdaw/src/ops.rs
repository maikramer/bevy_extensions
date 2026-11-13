//! Operators for the voxel terrain extension.
//!
//! Adding a feature goes through the editor's own `entity.add.group` and
//! `component.add` operators, so the node is a real scene node and both
//! operations land in the undo history and the saved `.bsn`.

use bevy::prelude::*;
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;
use jackdaw_extension::selection::Selection;

use crate::preview::VoxelPreviewState;

const CAVE_TYPE: &str = "bevy_ext_terrain::voxel::VoxelCave";
const ARCH_TYPE: &str = "bevy_ext_terrain::voxel::VoxelArch";
const BRIDGE_TYPE: &str = "bevy_ext_terrain::voxel::VoxelBridge";
const CLIFF_TYPE: &str = "bevy_ext_terrain::voxel::VoxelCliff";

fn add_feature(world: &mut World, name: &'static str, type_path: &'static str) -> OperatorResult {
    let Some(terrain) = world
        .get_resource::<Selection>()
        .and_then(Selection::primary)
    else {
        return OperatorResult::Cancelled;
    };
    if world
        .get::<bevy_ext_terrain::VoxelTerrain>(terrain)
        .is_none()
    {
        warn_caller(
            world,
            "vael.voxel: seleciona primeiro uma entidade VoxelTerrain",
        );
        return OperatorResult::Cancelled;
    }

    let history = CallOperatorSettings {
        creates_history_entry: true,
        ..default()
    };
    if let Err(err) = world
        .operator("entity.add.group")
        .param("name", name)
        .param("parent", terrain)
        .settings(history)
        .call()
    {
        warn_caller(world, format!("vael.voxel: {err}"));
        return OperatorResult::Cancelled;
    }

    let children: Vec<Entity> = world
        .get::<Children>(terrain)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let Some(new_entity) = children.iter().rev().copied().find(|entity| {
        world
            .get::<Name>(*entity)
            .is_some_and(|node_name| node_name.as_str() == name)
    }) else {
        warn_caller(
            world,
            "vael.voxel: o grupo criado não apareceu na hierarquia",
        );
        return OperatorResult::Cancelled;
    };

    if let Err(err) = world
        .operator("component.add")
        .param("entity", new_entity)
        .param("type_path", type_path)
        .settings(history)
        .call()
    {
        warn_caller(world, format!("vael.voxel: {err}"));
        return OperatorResult::Cancelled;
    }

    OperatorResult::Finished
}

#[operator(
    id = "vael.voxel.add_cave",
    label = "Voxel: add cave",
    allows_undo = false
)]
pub(crate) fn add_cave(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    add_feature(world, "Cave", CAVE_TYPE)
}

#[operator(
    id = "vael.voxel.add_arch",
    label = "Voxel: add arch",
    allows_undo = false
)]
pub(crate) fn add_arch(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    add_feature(world, "Arch", ARCH_TYPE)
}

#[operator(
    id = "vael.voxel.add_bridge",
    label = "Voxel: add bridge",
    allows_undo = false
)]
pub(crate) fn add_bridge(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    add_feature(world, "Bridge", BRIDGE_TYPE)
}

/// Cliff: carrier component only — the band resolves at bootstrap against
/// the carved grid, so there is no camera-local CSG preview for it.
#[operator(
    id = "vael.voxel.add_cliff",
    label = "Voxel: add cliff",
    allows_undo = false
)]
pub(crate) fn add_cliff(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    add_feature(world, "Cliff", CLIFF_TYPE)
}

/// Force the preview to rebuild after a change the dirty query cannot see
/// (for example an external heightmap edit).
#[operator(
    id = "vael.voxel.rebuild",
    label = "Voxel: rebuild preview",
    allows_undo = false
)]
pub(crate) fn rebuild_preview(
    _params: In<OperatorParameters>,
    mut state: ResMut<VoxelPreviewState>,
) -> OperatorResult {
    state.invalidate();
    OperatorResult::Finished
}

/// Real models (decoded GLBs) or AABB boxes in the viewport. Switching
/// re-evaluates the detail tiers; no preview is rebuilt from scratch.
#[operator(
    id = "vael.preview.toggle_models",
    label = "Vael: modelos reais / caixas",
    allows_undo = false
)]
pub(crate) fn toggle_models(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    {
        let mut mode = world.resource_mut::<crate::world_preview::ModelPreviewMode>();
        mode.real = !mode.real;
    }
    world
        .resource_mut::<crate::world_preview::PreviewBook>()
        .invalidate();
    OperatorResult::Finished
}

/// Re-scan the scene and rebuild the terrain/prop previews (the progressive
/// pipeline then streams them back in).
#[operator(
    id = "vael.preview.rebuild",
    label = "Vael: rebuild world preview",
    allows_undo = false
)]
pub(crate) fn rebuild_world_preview(
    _params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    world
        .resource_mut::<crate::world_preview::NodeScan>()
        .reset();
    world
        .resource_mut::<crate::world_preview::BoundsCache>()
        .invalidate();
    world
        .resource_mut::<crate::world_preview::PreviewBook>()
        .invalidate();
    world
        .resource_mut::<crate::world_preview::TerrainPreviews>()
        .invalidate();
    OperatorResult::Finished
}

/// Show/hide the model previews. The terrain preview stays: it is the world
/// reference the props sit on.
#[operator(
    id = "vael.preview.toggle_world",
    label = "Vael: preview do mundo",
    allows_undo = false
)]
pub(crate) fn toggle_world_preview(
    _params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    {
        let mut policy = world.resource_mut::<crate::world_preview::PreviewPolicy>();
        policy.enabled = !policy.enabled;
    }
    world
        .resource_mut::<crate::world_preview::PreviewBook>()
        .invalidate();
    OperatorResult::Finished
}
