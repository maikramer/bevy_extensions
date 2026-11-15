//! Painel "Vael: Cena": o que o mundo tem, o que já é tipado, problemas.

use bevy::ecs::hierarchy::ChildSpawner;
use bevy::prelude::*;
use bevy::text::FontSize;
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;

use crate::gizmos::SceneGizmos;
use crate::index::SceneIndex;

pub const WINDOW_ID: &str = "vael.scene";

#[derive(Component)]
pub struct SceneStatus;

#[derive(Component)]
pub struct ProblemList;

/// Cursor do "próximo problema".
#[derive(Resource, Default)]
pub struct ProblemCursor {
    next: usize,
}

fn text(root: &mut ChildSpawner, value: &str, size: f32, color: Color, marker: impl Bundle) {
    root.spawn((
        Text::new(value),
        TextFont {
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(color),
        marker,
    ));
}

pub fn build(world: &mut World, parent: Entity) {
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
            text(root, "Cena", 14.0, Color::WHITE, ());
            text(root, "…", 12.0, Color::srgb(0.82, 0.82, 0.86), SceneStatus);
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(4),
                row_gap: px(4),
                ..default()
            })
            .with_children(|row| {
                for (caption, op) in [
                    ("Gizmos: modo", "vael.scene.gizmos"),
                    ("Próximo problema", "vael.scene.next_problem"),
                    ("Validar e gravar", "vael.scene.validate_and_save"),
                ] {
                    row.spawn(button(ButtonProps::new(caption).call_operator(op.to_string())));
                }
            });
            text(root, "Problemas", 12.0, Color::WHITE, ());
            text(root, "", 11.0, Color::srgb(0.95, 0.70, 0.55), ProblemList);
            text(
                root,
                "Colisor verde, destrutível laranja, seleção branca; anéis: exclusão \
                 (vermelho), portal (roxo), luz (amarelo); laranja alto: região de spawner. \
                 Colisor/corpo/destrutível/colheita editam-se tipados no inspector \
                 (Physics / Gameplay); o resto do DSL fica em \"DSL Vael\".",
                11.0,
                Color::srgb(0.62, 0.62, 0.66),
                (),
            );
        });
}

/// Entidades com problemas, ordem estável (por nome).
pub fn problems(index: &SceneIndex) -> Vec<(Entity, String)> {
    let mut out: Vec<(Entity, String)> = index
        .nodes
        .iter()
        .flat_map(|(entity, info)| {
            let who = info
                .name
                .clone()
                .unwrap_or_else(|| format!("{} {entity}", info.tag));
            info.problems
                .iter()
                .map(move |problem| (*entity, format!("{who}: {problem}")))
        })
        .collect();
    // Cinemáticas: ids repetidos, filmes sem planos e gatilhos órfãos (só se
    // vê com a cena inteira, não por nó).
    let cinematics = crate::cinematics::collect(index);
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for row in &cinematics {
        *counts.entry(row.id.as_str()).or_default() += 1;
    }
    for row in &cinematics {
        let who = format!("Cinematic {}", row.id);
        if !row.id.is_empty() && counts.get(row.id.as_str()).copied().unwrap_or(0) > 1 {
            out.push((row.entity, format!("{who}: id de cinemática repetido")));
        }
        if row.shots.is_empty() {
            out.push((row.entity, format!("{who}: cinemática sem planos")));
        }
    }
    let ids: std::collections::HashSet<&str> =
        cinematics.iter().map(|row| row.id.as_str()).collect();
    for (entity, info) in &index.nodes {
        let Some(trigger) = &info.trigger else {
            continue;
        };
        let id = trigger.cinematic.trim();
        if id.is_empty() || ids.contains(id) {
            continue;
        }
        let who = info
            .name
            .clone()
            .unwrap_or_else(|| format!("{} {entity}", info.tag));
        out.push((*entity, format!("{who}: gatilho para cinemática inexistente `{id}`")));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

pub fn refresh(
    index: Res<SceneIndex>,
    gizmos: Res<SceneGizmos>,
    validation: Res<crate::validate::LastValidation>,
    mut status: Query<&mut Text, (With<SceneStatus>, Without<ProblemList>)>,
    mut list: Query<&mut Text, (With<ProblemList>, Without<SceneStatus>)>,
    mut seen: Local<Option<(u64, crate::gizmos::GizmoMode)>>,
) {
    if *seen == Some((index.generation, gizmos.mode)) && !validation.is_changed() {
        return;
    }
    *seen = Some((index.generation, gizmos.mode));
    let count = |f: &dyn Fn(&crate::index::NodeInfo) -> bool| index.nodes.values().filter(|i| f(i)).count();
    let colliders = count(&|i| i.collider.is_some());
    let bodies = count(&|i| i.body);
    let destructibles = count(&|i| i.destructible.is_some());
    let harvest = count(&|i| i.harvest.is_some());
    let legacy = count(&|i| i.legacy_attrs > 0);
    let cameras = count(&|i| i.camera.is_some());
    let interiors = count(&|i| i.interior.is_some());
    let cinematics = count(&|i| i.tag == "Cinematic");
    let triggers = count(&|i| i.trigger.is_some());
    let found = problems(&index);
    if let Ok(mut status) = status.single_mut() {
        status.0 = format!(
            "{} nós do mundo\nTipados: {colliders} colisores · {bodies} corpos · {destructibles} \
             destrutíveis · {harvest} colheitas\nCâmara: {cameras} · interiores: {interiors} · \
             cinemáticas: {cinematics} · gatilhos: {triggers}\nAinda em DSL (migráveis): {legacy}\n\
             Gizmos: {} · problemas: {}",
            index.nodes.len(),
            gizmos.mode.label(),
            found.len()
        );
        if let Some(summary) = &validation.summary {
            status.0.push('\n');
            status.0.push_str(summary);
        }
    }
    if let Ok(mut list) = list.single_mut() {
        list.0 = if found.is_empty() {
            "nenhum".into()
        } else {
            let mut lines: Vec<String> = found.iter().take(8).map(|(_, p)| format!("• {p}")).collect();
            if found.len() > 8 {
                lines.push(format!("… e mais {}", found.len() - 8));
            }
            lines.join("\n")
        };
    }
}

#[operator(
    id = "vael.scene.next_problem",
    label = "Vael: selecionar o próximo problema da cena",
    allows_undo = false
)]
pub fn next_problem(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let found = problems(world.resource::<SceneIndex>());
    if found.is_empty() {
        warn_caller(world, "vael: a cena não tem problemas conhecidos");
        return OperatorResult::Cancelled;
    }
    let at = {
        let mut cursor = world.resource_mut::<ProblemCursor>();
        let at = cursor.next % found.len();
        cursor.next = at + 1;
        at
    };
    let (entity, problem) = found[at].clone();
    info!("vael: problema {}/{}: {problem}", at + 1, found.len());
    if world
        .operator("selection.select")
        .param("entity", entity)
        .call()
        .is_ok()
    {
        let _ = world.operator("view.frame_selected").call();
    }
    OperatorResult::Finished
}
