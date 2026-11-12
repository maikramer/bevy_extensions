//! Operators for the voxel terrain extension.
//!
//! Adding a feature goes through the editor's own `entity.add.group` and
//! `component.add` operators, so the node is a real scene node and both
//! operations land in the undo history and the saved `.bsn`.

use bevy::prelude::*;
use jackdaw_editor_core::prelude::*;

use crate::preview::VoxelPreviewState;

/// Add a feature child to the selected `VoxelTerrain`.
#[operator(
    id = "vael.voxel.add_feature",
    label = "Voxel: add feature",
    allows_undo = false,
    params(kind(String, doc = "cave | arch | bridge"))
)]
pub(crate) fn add_feature(In(params): In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some(kind) = params.as_str("kind").map(str::to_ascii_lowercase) else {
        return OperatorResult::Cancelled;
    };
    let (name, type_path) = match kind.as_str() {
        "cave" => ("Cave", "bevy_ext_terrain::voxel::VoxelCave"),
        "arch" => ("Arch", "bevy_ext_terrain::voxel::VoxelArch"),
        "bridge" => ("Bridge", "bevy_ext_terrain::voxel::VoxelBridge"),
        _ => return OperatorResult::Cancelled,
    };

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
            "vael.voxel.add_feature: seleciona primeiro uma entidade VoxelTerrain",
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
        warn_caller(world, format!("vael.voxel.add_feature: {err}"));
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
            "vael.voxel.add_feature: o grupo criado não apareceu na hierarquia",
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
        warn_caller(world, format!("vael.voxel.add_feature: {err}"));
        return OperatorResult::Cancelled;
    }

    OperatorResult::Finished
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
