//! Peças de UI partilhadas pelo painel.

use bevy::ecs::hierarchy::ChildSpawner;
use bevy::prelude::*;
use bevy::text::FontSize;
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;

pub(crate) fn heading(root: &mut ChildSpawner, text: &str) {
    root.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::WHITE),
    ));
}

pub(crate) fn label(root: &mut ChildSpawner, text: &str) {
    root.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(11.0),
            ..default()
        },
        TextColor(Color::srgb(0.62, 0.62, 0.66)),
    ));
}

pub(crate) fn status(root: &mut ChildSpawner, marker: impl Component) {
    root.spawn((
        Text::new("…"),
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
        TextColor(Color::srgb(0.82, 0.82, 0.86)),
        marker,
    ));
}

pub(crate) fn row(root: &mut ChildSpawner, buttons: &[(&str, &str)]) {
    root.spawn(Node {
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::Wrap,
        column_gap: px(4),
        row_gap: px(4),
        ..default()
    })
    .with_children(|row| {
        for (caption, op) in buttons {
            row.spawn(button(
                ButtonProps::new(*caption).call_operator(op.to_string()),
            ));
        }
    });
}

pub(crate) fn column(world: &mut World, parent: Entity, build: impl FnOnce(&mut ChildSpawner)) {
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
        .with_children(build);
}

/// Escreve um campo pelo operador `component.set` do editor (undo + `.bsn`).
pub(crate) fn set_field(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    field: &str,
    value: impl Into<String>,
) -> OperatorResult {
    let history = CallOperatorSettings {
        creates_history_entry: true,
        ..default()
    };
    match world
        .operator("component.set")
        .param("entity", entity)
        .param("type_path", type_path.to_string())
        .param("field", field.to_string())
        .param("value", value.into())
        .settings(history)
        .call()
    {
        Ok(_) => OperatorResult::Finished,
        Err(err) => {
            warn_caller(world, format!("cercas: {type_path}.{field}: {err}"));
            OperatorResult::Cancelled
        }
    }
}
