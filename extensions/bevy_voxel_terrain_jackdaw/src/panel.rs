//! The "Vael: Voxel Terrain" dock panel.
//!
//! The panel is intentionally a thin controller: the scene hierarchy and the
//! inspector carry the actual fields, while this lists the terrains/features
//! and offers the one-click feature nodes. Every mutation is an editor
//! operator, so undo and save work.

use bevy::prelude::*;
use bevy::text::FontSize;
use jackdaw_extension::prelude::*;

use bevy_ext_terrain::{VoxelArch, VoxelBridge, VoxelCave, VoxelTerrain};

use crate::world_preview::PreviewStatus;

/// Marks the status line the refresh system rewrites.
#[derive(Component)]
pub(crate) struct VoxelPanelStatus;

/// Marks the progressive world-preview status line.
#[derive(Component)]
pub(crate) struct WorldPreviewStatus;

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
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
            root.spawn((
                Text::new("Seleciona uma entidade VoxelTerrain na cena."),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb(0.75, 0.75, 0.75)),
                VoxelPanelStatus,
            ));
            root.spawn((
                Text::new("Preview do mundo: à espera da cena…"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb(0.72, 0.78, 0.72)),
                WorldPreviewStatus,
            ));
            root.spawn((
                Text::new(
                    "As features são entidades filhas: seleciona o terreno, adiciona \
                     uma e edita os campos no inspetor. O preview aparece à volta da câmara.",
                ),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgb(0.65, 0.65, 0.65)),
            ));

            for (caption, op) in [
                ("Adicionar cave", "vael.voxel.add_cave"),
                ("Adicionar arco", "vael.voxel.add_arch"),
                ("Adicionar ponte", "vael.voxel.add_bridge"),
                ("Adicionar cliff", "vael.voxel.add_cliff"),
            ] {
                root.spawn(button(ButtonProps::new(caption).call_operator(op)));
            }
            root.spawn(button(
                ButtonProps::new("Rebuild preview").call_operator("vael.voxel.rebuild"),
            ));
            root.spawn(button(
                ButtonProps::new("Rebuild world preview").call_operator("vael.preview.rebuild"),
            ));
            root.spawn(button(
                ButtonProps::new("Enquadrar mundo").call_operator("view.frame_all"),
            ));
            root.spawn(button(
                ButtonProps::new("Ligar/desligar preview")
                    .call_operator("vael.preview.toggle_world"),
            ));
        });
}

pub(super) fn refresh(
    terrains: Query<&VoxelTerrain>,
    caves: Query<(), With<VoxelCave>>,
    arches: Query<(), With<VoxelArch>>,
    bridges: Query<(), With<VoxelBridge>>,
    preview: Res<PreviewStatus>,
    mut status: Query<&mut Text, (With<VoxelPanelStatus>, Without<WorldPreviewStatus>)>,
    mut world_status: Query<&mut Text, (With<WorldPreviewStatus>, Without<VoxelPanelStatus>)>,
) {
    let Ok(mut status) = status.single_mut() else {
        return;
    };
    let count = terrains.iter().count();
    status.0 = if count == 0 {
        match preview.terrain.as_deref() {
            // O mundo do Vael usa o `<Terrain>` do DSL (heightmap `.ahgt` +
            // paint), não o componente `VoxelTerrain`: o terreno está lá.
            Some(terrain) => format!(
                "Terreno do mundo: nó Terrain (heightmap), {terrain}. Caves, arcos e \
                 pontes voxel precisam de um VoxelTerrain (nenhum nesta cena)."
            ),
            None => "Sem terreno nesta cena (nem nó Terrain nem VoxelTerrain).".to_string(),
        }
    } else {
        format!(
            "{count} terreno(s), {} cave(s), {} arco(s), {} ponte(s).",
            caves.iter().count(),
            arches.iter().count(),
            bridges.iter().count()
        )
    };

    if let Ok(mut text) = world_status.single_mut() {
        text.0 = if preview.tracked == 0 {
            "Preview do mundo: à espera da cena…".to_string()
        } else {
            let terrain = preview
                .terrain
                .as_deref()
                .map_or("sem terreno".to_string(), |terrain| {
                    format!("terreno: {terrain}")
                });
            format!(
                "Preview: {} nós, {} com modelo · {} perto ({} reais, {} caixas) · {} longe · {} à espera\n{}",
                preview.tracked,
                preview.previewable,
                preview.boxes + preview.real,
                preview.real,
                preview.boxes,
                preview.far,
                preview.queued,
                terrain
            )
        };
    }
}
