//! Cinemáticas: modelo do documento, painel "Vael: Cinemáticas" e gizmos.
//!
//! As cinemáticas vivem em `world/cinematics.bsn` como `<Cinematic id>` com
//! filhos `<Shot from look to look-to duration fov ease anchor>`. O painel lê
//! os nós do [`SceneIndex`], mostra a timeline (tempos acumulados como o
//! parser os encadeia), selecciona planos no Outliner, toca/pára no jogo pela
//! bridge e cria planos a partir da câmara do viewport.
//!
//! O `viber cinematic` da CLI é XML-only (`SceneDoc`/`world.xml`, aposentado),
//! por isso a autoria é aqui: o painel escreve no documento do Jackdaw.

use std::collections::HashSet;

use bevy::ecs::hierarchy::ChildSpawner;
use bevy::prelude::*;
use bevy::text::FontSize;
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;
use jackdaw_extension::viewport::MainViewportCamera;

use crate::index::{NodeInfo, SceneIndex};

pub const WINDOW_ID: &str = "vael.cinematics";
/// `type_path` do componente que guarda a tag/attrs do nó no documento.
const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";

#[derive(Component)]
pub struct CinematicStatus;

#[derive(Component)]
pub struct PoseNote;

/// Um plano, com o tempo já acumulado como o parser o lê.
#[derive(Clone, Debug)]
pub struct ShotRow {
    pub entity: Entity,
    pub start: f32,
    pub duration: f32,
    pub from: Vec3,
    pub to: Vec3,
    pub look: Vec3,
    pub look_to: Vec3,
    pub fov: Option<f32>,
    pub ease: String,
    pub anchor: String,
}

/// Uma cinemática do documento.
#[derive(Clone, Debug)]
pub struct CinematicRow {
    pub id: String,
    pub entity: Entity,
    pub shots: Vec<ShotRow>,
    /// Fim da cena (planos + elementos temporizados), como o runtime.
    pub duration: f32,
    pub letterbox: bool,
    pub hide_hud: bool,
    pub skippable: bool,
    /// Gatilhos de área que apontam para este `id` (entidade, `once`).
    pub triggers: Vec<(Entity, bool)>,
}

#[derive(Resource, Default)]
pub struct Cinematics {
    pub rows: Vec<CinematicRow>,
    generation: u64,
}

/// A cinemática/plano em foco no painel (índices na lista ordenada).
#[derive(Resource, Default)]
pub struct CinematicFocus {
    pub index: usize,
    pub shot: usize,
    /// Último resultado de uma acção (tocar, pose, novo plano).
    pub note: String,
}

impl Cinematics {
    pub fn focused(&self, focus: &CinematicFocus) -> Option<&CinematicRow> {
        if self.rows.is_empty() {
            return None;
        }
        self.rows.get(focus.index.min(self.rows.len() - 1))
    }
}

fn num(text: Option<&str>, default: f32) -> f32 {
    text.and_then(|text| text.trim().parse().ok())
        .filter(|value: &f32| value.is_finite())
        .unwrap_or(default)
}

fn vec3(text: Option<&str>) -> Option<Vec3> {
    let values: Vec<f32> = text?
        .split_whitespace()
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    (values.len() == 3).then(|| Vec3::new(values[0], values[1], values[2]))
}

fn flag(text: Option<&str>, default: bool) -> bool {
    match text.map(str::trim) {
        Some("false" | "0" | "no") => false,
        Some("true" | "1" | "yes") => true,
        _ => default,
    }
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn shot_row(entity: Entity, info: &NodeInfo) -> Option<ShotRow> {
    let from = vec3(attr(&info.attrs, "from"))?;
    let look = vec3(attr(&info.attrs, "look"))?;
    Some(ShotRow {
        entity,
        start: 0.0,
        duration: num(attr(&info.attrs, "duration"), 4.0).max(0.0),
        from,
        to: vec3(attr(&info.attrs, "to")).unwrap_or(from),
        look,
        look_to: vec3(attr(&info.attrs, "look-to")).unwrap_or(look),
        fov: attr(&info.attrs, "fov")
            .map(|_| num(attr(&info.attrs, "fov"), 60.0)),
        ease: attr(&info.attrs, "ease").unwrap_or("linear").to_string(),
        anchor: attr(&info.attrs, "anchor").unwrap_or("world").to_string(),
    })
}

/// Lê as cinemáticas do índice. Os planos são ordenados pela posição entre
/// irmãos (a ordem do documento é a que o parser encadeia).
pub fn collect(index: &SceneIndex) -> Vec<CinematicRow> {
    let mut rows: Vec<CinematicRow> = index
        .nodes
        .iter()
        .filter(|(_, info)| info.tag == "Cinematic")
        .map(|(entity, info)| {
            let mut ordered: Vec<(usize, ShotRow)> = index
                .nodes
                .iter()
                .filter(|(_, shot)| shot.tag == "Shot" && shot.parent == Some(*entity))
                .filter_map(|(shot_entity, shot)| {
                    Some((shot.order.unwrap_or(usize::MAX), shot_row(*shot_entity, shot)?))
                })
                .collect();
            ordered.sort_by_key(|(order, _)| *order);
            let mut clock = 0.0f32;
            let mut shots: Vec<ShotRow> = Vec::new();
            for (_, mut shot) in ordered {
                shot.start = clock;
                clock += shot.duration;
                shots.push(shot);
            }
            // Elementos temporizados (`Line`/`Title`/`Fade`/`Sound`) no mesmo
            // relógio absoluto: o `<Sound>` conta o instante do cue.
            let timed = index
                .nodes
                .values()
                .filter(|node| node.parent == Some(*entity))
                .flat_map(|node| {
                    let at = num(attr(&node.attrs, "at"), 0.0);
                    match node.tag.as_str() {
                        "Line" | "Title" | "Fade" => Some(at + num(attr(&node.attrs, "duration"), 1.0)),
                        "Sound" => Some(at),
                        _ => None,
                    }
                })
                .fold(0.0f32, f32::max);
            let id = attr(&info.attrs, "id").unwrap_or("").to_string();
            let triggers: Vec<(Entity, bool)> = index
                .nodes
                .iter()
                .filter_map(|(trigger_entity, node)| {
                    let trigger = node.trigger.as_ref()?;
                    (trigger.cinematic.trim() == id).then_some((*trigger_entity, trigger.once))
                })
                .collect();
            CinematicRow {
                id,
                entity: *entity,
                duration: clock.max(timed),
                letterbox: flag(attr(&info.attrs, "letterbox"), true),
                hide_hud: flag(attr(&info.attrs, "hide-hud"), true),
                skippable: flag(attr(&info.attrs, "skippable"), true),
                triggers,
                shots,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

pub fn refresh(
    index: Res<SceneIndex>,
    mut cinematics: ResMut<Cinematics>,
    mut focus: ResMut<CinematicFocus>,
    mut status: Query<&mut Text, (With<CinematicStatus>, Without<PoseNote>)>,
    mut note: Query<&mut Text, (With<PoseNote>, Without<CinematicStatus>)>,
) {
    let changed = cinematics.generation != index.generation;
    if changed {
        cinematics.generation = index.generation;
        cinematics.rows = collect(&index);
        focus.shot = 0;
    }
    if !changed && !focus.is_changed() {
        return;
    }
    if let Ok(mut status) = status.single_mut() {
        status.0 = match cinematics.focused(&focus) {
            None => "Nenhuma <Cinematic> no documento.".to_string(),
            Some(row) => {
                let total = row.shots.len();
                let check = |on: bool| if on { "✓" } else { "—" };
                let triggers = if row.triggers.is_empty() {
                    "sem gatilho".to_string()
                } else {
                    let once = row.triggers.iter().any(|(_, once)| *once);
                    format!(
                        "{} gatilho(s){}",
                        row.triggers.len(),
                        if once { " (uma vez)" } else { "" }
                    )
                };
                let mut text = format!(
                    "{} · {:.1}s · {} plano(s){} · letterbox {} · hud oculto {} · saltável {} · {}\n",
                    row.id,
                    row.duration,
                    total,
                    if total == 0 { " (sem planos!)" } else { "" },
                    check(row.letterbox),
                    check(row.hide_hud),
                    check(row.skippable),
                    triggers,
                );
                if row.shots.is_empty() {
                    text.push_str("Adiciona um plano com a câmara do viewport.\n");
                }
                for (index, shot) in row.shots.iter().enumerate() {
                    let mark = if index == focus.shot { "▶" } else { " " };
                    text.push_str(&format!(
                        "{mark} {index:>2} {start:>5.1}s {duration:>4.1}s  from {from}  look {look}  {ease}{fov}\n",
                        start = shot.start,
                        duration = shot.duration,
                        from = vec_text(shot.from),
                        look = vec_text(shot.look),
                        ease = shot.ease,
                        fov = format!(
                            "{}{}",
                            shot.fov
                                .map(|fov| format!("  fov {fov:.0}"))
                                .unwrap_or_default(),
                            if shot.anchor == "world" {
                                String::new()
                            } else {
                                format!("  anchor {}", shot.anchor)
                            }
                        ),
                    ));
                }
                text
            }
        };
    }
    if let Ok(mut note) = note.single_mut() {
        note.0 = focus.note.clone();
    }
}

fn vec_text(value: Vec3) -> String {
    format!(
        "{} {} {}",
        bevy_ext_scene::num(value.x),
        bevy_ext_scene::num(value.y),
        bevy_ext_scene::num(value.z)
    )
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
            text(root, "Cinemáticas", 14.0, Color::WHITE, ());
            text(root, "…", 11.0, Color::srgb(0.82, 0.82, 0.86), CinematicStatus);
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(4),
                row_gap: px(4),
                ..default()
            })
            .with_children(|row| {
                for (caption, op) in [
                    ("◀ filme", "vael.cinematics.prev"),
                    ("filme ▶", "vael.cinematics.next"),
                    ("◀ plano", "vael.cinematics.prev_shot"),
                    ("plano ▶", "vael.cinematics.next_shot"),
                    ("Seleccionar plano", "vael.cinematics.select_shot"),
                ] {
                    row.spawn(button(ButtonProps::new(caption).call_operator(op.to_string())));
                }
            });
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(4),
                row_gap: px(4),
                ..default()
            })
            .with_children(|row| {
                for (caption, op) in [
                    ("Tocar no jogo", "vael.cinematics.play"),
                    ("Parar", "vael.cinematics.stop"),
                    ("Novo plano (viewport)", "vael.cinematics.add_shot"),
                    ("Pose da câmara", "vael.cinematics.pose"),
                ] {
                    row.spawn(button(ButtonProps::new(caption).call_operator(op.to_string())));
                }
            });
            text(root, "", 11.0, Color::srgb(0.70, 0.85, 0.70), PoseNote);
            text(
                root,
                "Toca no jogo pelo bridge (`VIBER_TOOLS_PORT`, por omissão 15702) — o jogo tem \
                 de estar a correr com o Play. \"Novo plano\" põe um <Shot> no fim do filme com a \
                 pose da câmara do viewport; ajusta os números no inspector.",
                11.0,
                Color::srgb(0.62, 0.62, 0.66),
                (),
            );
        });
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

/// A cinemática em foco (entidade e id).
fn focused(world: &World) -> Option<(Entity, String)> {
    let cinematics = world.get_resource::<Cinematics>()?;
    let focus = world.get_resource::<CinematicFocus>()?;
    let row = cinematics.focused(focus)?;
    Some((row.entity, row.id.clone()))
}

fn select(world: &mut World, entity: Entity) {
    if world
        .operator("selection.select")
        .param("entity", entity)
        .call()
        .is_ok()
    {
        let _ = world.operator("view.frame_selected").call();
    }
}

#[operator(
    id = "vael.cinematics.prev",
    label = "Vael: cinemática anterior",
    allows_undo = false
)]
pub fn prev(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    step(world, -1)
}

#[operator(
    id = "vael.cinematics.next",
    label = "Vael: cinemática seguinte",
    allows_undo = false
)]
pub fn next(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    step(world, 1)
}

fn step(world: &mut World, delta: isize) -> OperatorResult {
    let count = world
        .get_resource::<Cinematics>()
        .map(|cinematics| cinematics.rows.len())
        .unwrap_or(0);
    if count == 0 {
        warn_caller(world, "vael: o documento não tem nenhuma <Cinematic>");
        return OperatorResult::Cancelled;
    }
    {
        let mut focus = world.resource_mut::<CinematicFocus>();
        let at = focus.index.min(count - 1) as isize + delta;
        focus.index = at.rem_euclid(count as isize) as usize;
        focus.shot = 0;
        focus.note.clear();
    }
    if let Some((entity, _)) = focused(world) {
        select(world, entity);
    }
    OperatorResult::Finished
}

#[operator(
    id = "vael.cinematics.prev_shot",
    label = "Vael: plano anterior",
    allows_undo = false
)]
pub fn prev_shot(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    shot_step(world, -1)
}

#[operator(
    id = "vael.cinematics.next_shot",
    label = "Vael: plano seguinte",
    allows_undo = false
)]
pub fn next_shot(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    shot_step(world, 1)
}

fn shot_step(world: &mut World, delta: isize) -> OperatorResult {
    let shots = world
        .get_resource::<Cinematics>()
        .and_then(|cinematics| {
            let focus = world.get_resource::<CinematicFocus>()?;
            cinematics
                .focused(focus)
                .map(|row| row.shots.iter().map(|shot| shot.entity).collect::<Vec<_>>())
        })
        .unwrap_or_default();
    if shots.is_empty() {
        warn_caller(world, "vael: esta cinemática não tem planos");
        return OperatorResult::Cancelled;
    }
    {
        let mut focus = world.resource_mut::<CinematicFocus>();
        let at = focus.shot.min(shots.len() - 1) as isize + delta;
        focus.shot = at.rem_euclid(shots.len() as isize) as usize;
    }
    select(world, shots[world.resource::<CinematicFocus>().shot]);
    OperatorResult::Finished
}

#[operator(
    id = "vael.cinematics.select_shot",
    label = "Vael: seleccionar o plano em foco",
    allows_undo = false
)]
pub fn select_shot(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some((cinematics, focus)) = world
        .get_resource::<Cinematics>()
        .zip(world.get_resource::<CinematicFocus>())
    else {
        return OperatorResult::Cancelled;
    };
    let Some(shot) = cinematics
        .focused(focus)
        .and_then(|row| row.shots.get(focus.shot))
    else {
        warn_caller(world, "vael: nenhum plano em foco");
        return OperatorResult::Cancelled;
    };
    let entity = shot.entity;
    select(world, entity);
    OperatorResult::Finished
}

fn set_note(world: &mut World, note: String) {
    if let Some(mut focus) = world.get_resource_mut::<CinematicFocus>() {
        focus.note = note;
    }
}

#[operator(
    id = "vael.cinematics.play",
    label = "Vael: tocar a cinemática no jogo",
    allows_undo = false
)]
pub fn play(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some((_, id)) = focused(world) else {
        warn_caller(world, "vael: nenhuma cinemática em foco");
        return OperatorResult::Cancelled;
    };
    match crate::bridge::call("viber.cinematic.play", serde_json::json!({ "id": id })) {
        Ok(result) => {
            let duration = result
                .get("duration")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            info!("vael: a tocar `{id}` ({duration:.1}s) no jogo");
            set_note(world, format!("A tocar `{id}` no jogo."));
            OperatorResult::Finished
        }
        Err(error) => {
            warn_caller(world, format!("vael: tocar `{id}` falhou: {error}"));
            set_note(world, format!("Tocar falhou: {error}"));
            OperatorResult::Cancelled
        }
    }
}

#[operator(
    id = "vael.cinematics.stop",
    label = "Vael: parar a cinemática no jogo",
    allows_undo = false
)]
pub fn stop(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    match crate::bridge::call("viber.cinematic.stop", serde_json::json!({})) {
        Ok(result) => {
            let stopped = result
                .get("stopped")
                .and_then(|value| value.as_str())
                .unwrap_or("—");
            info!("vael: cinemática parada ({stopped})");
            set_note(world, format!("Parada ({stopped})."));
            OperatorResult::Finished
        }
        Err(error) => {
            warn_caller(world, format!("vael: parar falhou: {error}"));
            set_note(world, format!("Parar falhou: {error}"));
            OperatorResult::Cancelled
        }
    }
}

/// Pose actual da câmara do viewport: (olho, ponto olhado a 10 m).
fn viewport_pose(world: &mut World) -> Option<(Vec3, Vec3)> {
    let mut cameras = world.query_filtered::<&GlobalTransform, With<MainViewportCamera>>();
    let camera = *cameras.iter(world).next()?;
    let eye = camera.translation();
    Some((eye, eye + camera.forward() * 10.0))
}

#[operator(
    id = "vael.cinematics.pose",
    label = "Vael: pose da câmara do viewport como texto",
    allows_undo = false
)]
pub fn pose(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some((eye, look)) = viewport_pose(world) else {
        warn_caller(world, "vael: sem câmara de viewport");
        return OperatorResult::Cancelled;
    };
    let text = format!("from=\"{}\" look=\"{}\"", vec_text(eye), vec_text(look));
    info!("vael: {text}");
    set_note(world, text);
    OperatorResult::Finished
}

#[operator(
    id = "vael.cinematics.add_shot",
    label = "Vael: novo plano a partir da câmara do viewport",
    description = "Cria um <Shot> no fim da cinemática em foco com a pose da câmara do \
                   viewport (um passo de undo).",
    allows_undo = true
)]
pub fn add_shot(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some((cinematic, id)) = focused(world) else {
        warn_caller(world, "vael: nenhuma cinemática em foco");
        return OperatorResult::Cancelled;
    };
    let Some((eye, look)) = viewport_pose(world) else {
        warn_caller(world, "vael: sem câmara de viewport");
        return OperatorResult::Cancelled;
    };
    let from_text = vec_text(eye);
    let look_text = vec_text(look);
    let before: HashSet<Entity> = children(world, cinematic).into_iter().collect();
    let added = world
        .operator("entity.add.empty")
        .param("parent", cinematic)
        .param("name", "Shot")
        .param("pos", eye)
        .call();
    if !matches!(added, Ok(OperatorResult::Finished)) {
        let note = format!("from=\"{from_text}\" look=\"{look_text}\"");
        warn_caller(
            world,
            format!("vael: criar o nó do plano falhou; a pose é {note}"),
        );
        set_note(world, note);
        return OperatorResult::Cancelled;
    }
    let Some(new_entity) = children(world, cinematic)
        .into_iter()
        .find(|entity| !before.contains(entity))
    else {
        warn_caller(world, "vael: o nó do plano não apareceu debaixo da cinemática");
        return OperatorResult::Cancelled;
    };
    let attrs = serde_json::json!([
        { "key": "from", "value": from_text },
        { "key": "look", "value": look_text },
        { "key": "duration", "value": "4" },
    ])
    .to_string();
    let tag = set_world_node(world, new_entity, "tag", "\"Shot\"".to_string());
    let attrs_ok = set_world_node(world, new_entity, "attrs", attrs);
    let pose_ok = set_world_node(world, new_entity, "pose", "false".to_string());
    if !(tag && attrs_ok && pose_ok) {
        let note = format!("from=\"{from_text}\" look=\"{look_text}\"");
        warn_caller(
            world,
            format!(
                "vael: o nó foi criado mas os atributos do <Shot> não foram escritos; \
                 a pose é {note} (inspector › DSL Vael › tag/attrs)."
            ),
        );
        set_note(world, note);
        return OperatorResult::Cancelled;
    }
    select(world, new_entity);
    info!("vael: plano novo em `{id}` (from {from_text} · look {look_text})");
    set_note(
        world,
        format!("Plano novo em `{id}`: from=\"{from_text}\" look=\"{look_text}\"."),
    );
    OperatorResult::Finished
}

/// Escreve um campo do `WorldNode` do nó pelo operador do editor (entra no
/// undo e na gravação). `PropertyValue` só aceita `&'static str`, por isso o
/// valor vai como `String`.
fn set_world_node(
    world: &mut World,
    entity: Entity,
    field: &'static str,
    value: String,
) -> bool {
    matches!(
        world
            .operator("component.set")
            .param("entity", entity)
            .param("type_path", WORLD_NODE)
            .param("field", field)
            .param("value", value)
            .call(),
        Ok(OperatorResult::Finished)
    )
}

fn children(world: &mut World, parent: Entity) -> Vec<Entity> {
    world
        .get::<Children>(parent)
        .map(|children| children.iter().collect())
        .unwrap_or_default()
}
