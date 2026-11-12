//! The "Vael: Voxel Terrain" dock panel.
//!
//! The panel is intentionally a thin controller: the scene hierarchy and the
//! inspector carry the actual fields, while this lists the terrains/features
//! and offers the one-click feature nodes. Every mutation is an editor
//! operator, so undo and save work.

use bevy::prelude::*;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, button};

use bevy_ext_terrain::{VoxelArch, VoxelBridge, VoxelCave, VoxelTerrain};

/// Marks the status line the refresh system rewrites.
#[derive(Component)]
struct VoxelPanelStatus;

pub(super) fn build(world: &mut World, parent: Entity) {
    world
        .spawn((
            ChildOf(parent),
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                padding: UiRect::all(px(8)),
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Voxel Terrain"),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
            root.spawn((
                Text::new("Seleciona uma entidade VoxelTerrain na cena."),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.75, 0.75, 0.75)),
                VoxelPanelStatus,
            ));
            root.spawn((
                Text::new(
                    "As features são entidades filhas: seleciona o terreno, adiciona \
                     uma e edita os campos no inspetor. O preview aparece à volta da câmara.",
                ),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb(0.65, 0.65, 0.65)),
            ));

            for (caption, kind) in [
                ("Adicionar cave", "cave"),
                ("Adicionar arco", "arch"),
                ("Adicionar ponte", "bridge"),
            ] {
                root.spawn((
                    button(ButtonProps::new(caption).call_operator("vael.voxel.add_feature")),
                    ButtonOperatorCall::new("vael.voxel.add_feature").with_param("kind", kind),
                ));
            }
            root.spawn((
                button(ButtonProps::new("Rebuild preview").call_operator("vael.voxel.rebuild")),
                ButtonOperatorCall::new("vael.voxel.rebuild"),
            ));
        });
}

pub(super) fn refresh(
    terrains: Query<&VoxelTerrain>,
    caves: Query<(), With<VoxelCave>>,
    arches: Query<(), With<VoxelArch>>,
    bridges: Query<(), With<VoxelBridge>>,
    mut status: Query<&mut Text, With<VoxelPanelStatus>>,
) {
    let Ok(mut status) = status.single_mut() else {
        return;
    };
    let count = terrains.iter().count();
    status.0 = if count == 0 {
        "Sem VoxelTerrain nesta cena. Adiciona o componente a uma entidade.".to_string()
    } else {
        format!(
            "{count} terreno(s), {} cave(s), {} arco(s), {} ponte(s).",
            caves.iter().count(),
            arches.iter().count(),
            bridges.iter().count()
        )
    };
}
