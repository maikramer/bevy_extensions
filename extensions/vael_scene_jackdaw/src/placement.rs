//! Biblioteca de assets e pincel de props.
//!
//! A biblioteca lista os modelos que a cena já usa, por categoria, com o nº
//! de instâncias. Escolhido um modelo:
//! - **Colocar**: cada clique no viewport põe uma cópia no chão;
//! - **Pincel**: clicar/arrastar espalha cópias num círculo (raio e
//!   densidade no painel), com rotação e escala aleatórias.
//!
//! Cada cópia é um `entity.duplicate` de uma instância existente do mesmo
//! modelo, por isso herda tudo o que ela tem (colisor, corpo, destrutível,
//! colheita, LOD, estação de flora…), e fica no mesmo grupo do Outliner. Um
//! traço inteiro é um passo de undo (`with_history_span`). A altura do chão
//! vem do mesmo `.ahgt` que o jogo usa.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::prelude::*;
use bevy_ext_terrain::AhgtHeightField;
use bevy_voxel_terrain::HeightField;
use jackdaw_extension::document::SceneFilePath;
use jackdaw_extension::op::{warn_caller, with_history_span};
use jackdaw_extension::prelude::*;
use jackdaw_extension::selection::Selection;
use jackdaw_extension::viewport::ViewportCursor;

use crate::index::{SceneIndex, project_root};

pub const WINDOW_ID: &str = "vael.assets";
const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
/// Botões de modelo mostrados por categoria.
const LIST_MAX: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Kind {
    #[default]
    Plant,
    Rock,
    Building,
    Creature,
    Prop,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Plant, Kind::Rock, Kind::Building, Kind::Creature, Kind::Prop];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Plant => "Plantas",
            Kind::Rock => "Rochas",
            Kind::Building => "Edifícios",
            Kind::Creature => "Criaturas",
            Kind::Prop => "Props",
        }
    }

    pub fn of(url: &str) -> Self {
        let url = url.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|needle| url.contains(needle));
        if has(&["/characters/", "/creatures/"]) {
            Kind::Creature
        } else if has(&["/forest/", "/vegetation/", "tree", "pine", "bush", "cactus", "fern", "reed", "flower", "shrub"]) {
            Kind::Plant
        } else if has(&["rock", "stone", "cliff", "boulder", "/terrain/", "cairn", "crystal"]) {
            Kind::Rock
        } else if has(&["/village/", "/infra/", "/interiors/", "/farm/", "house", "wall", "gate", "tower", "bridge", "shack", "hut", "chapel", "barn", "fence"]) {
            Kind::Building
        } else {
            Kind::Prop
        }
    }
}

/// Um modelo da cena: quantas instâncias e uma delas para copiar.
#[derive(Clone, Debug)]
pub struct AssetEntry {
    pub url: String,
    pub name: String,
    pub count: usize,
    pub exemplar: Entity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Off,
    Place,
    Brush,
}

#[derive(Resource)]
pub struct AssetLibrary {
    pub kind: Kind,
    pub entries: BTreeMap<Kind, Vec<AssetEntry>>,
    pub chosen: Option<AssetEntry>,
    pub mode: Mode,
    pub radius: f32,
    pub density: usize,
    generation: u64,
    /// Lista do painel por reconstruir.
    pub dirty: bool,
    stroke_timer: f32,
    rng: u64,
}

impl Default for AssetLibrary {
    fn default() -> Self {
        Self {
            kind: Kind::Plant,
            entries: BTreeMap::new(),
            chosen: None,
            mode: Mode::Off,
            radius: 8.0,
            density: 4,
            generation: u64::MAX,
            dirty: true,
            stroke_timer: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl AssetLibrary {
    fn random(&mut self) -> f32 {
        // xorshift64*: determinístico por sessão, sem dependências.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// O heightfield do mundo aberto (carregado à primeira colocação).
#[derive(Resource, Default)]
pub struct Ground {
    key: Option<(PathBuf, u32, u32)>,
    field: Option<AhgtHeightField>,
}

/// Botão de um modelo na lista (índice na categoria atual).
#[derive(Component)]
pub struct AssetSlot(pub usize);

/// Botão de categoria.
#[derive(Component)]
pub struct KindSlot(pub Kind);

/// Contentor da lista de modelos (reconstruído quando a categoria muda).
#[derive(Component)]
pub struct AssetList;

#[derive(Component)]
pub struct AssetStatus;

/// Reconstrói o catálogo quando o índice da cena muda.
pub fn refresh_catalog(index: Res<SceneIndex>, mut library: ResMut<AssetLibrary>) {
    if library.generation == index.generation {
        return;
    }
    library.generation = index.generation;
    let mut by_url: BTreeMap<String, (usize, Entity)> = BTreeMap::new();
    for (entity, info) in &index.nodes {
        // Só props soltos: `GLTFLoader` são partes visuais de composições e
        // templates de scatter, que não se copiam sozinhas.
        if info.tag != "GltfScene" {
            continue;
        }
        if let Some(url) = &info.url {
            let entry = by_url.entry(url.clone()).or_insert((0, *entity));
            entry.0 += 1;
        }
    }
    let mut entries: BTreeMap<Kind, Vec<AssetEntry>> = BTreeMap::new();
    for (url, (count, exemplar)) in by_url {
        let name = url
            .rsplit('/')
            .next()
            .unwrap_or(&url)
            .trim_end_matches(".glb")
            .trim_end_matches("_lod0")
            .replace('_', " ");
        entries.entry(Kind::of(&url)).or_default().push(AssetEntry {
            url,
            name,
            count,
            exemplar,
        });
    }
    for list in entries.values_mut() {
        list.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    }
    // A escolha atual sobrevive se o modelo ainda existe (o exemplar pode ter
    // mudado).
    if let Some(chosen) = &library.chosen {
        let url = chosen.url.clone();
        library.chosen = entries
            .values()
            .flatten()
            .find(|entry| entry.url == url)
            .cloned();
    }
    library.entries = entries;
    library.dirty = true;
}

fn text(root: &mut bevy::ecs::hierarchy::ChildSpawner, value: &str, size: f32, color: Color, marker: impl Bundle) {
    root.spawn((
        Text::new(value),
        TextFont {
            font_size: bevy::text::FontSize::Px(size),
            ..default()
        },
        TextColor(color),
        marker,
    ));
}

fn wrap_row() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::Wrap,
        column_gap: px(4),
        row_gap: px(4),
        ..default()
    }
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
            text(root, "Assets", 14.0, Color::WHITE, ());
            text(root, "…", 12.0, Color::srgb(0.82, 0.82, 0.86), AssetStatus);
            root.spawn(wrap_row()).with_children(|row| {
                for kind in Kind::ALL {
                    row.spawn((button(ButtonProps::new(kind.label())), KindSlot(kind)));
                }
            });
            root.spawn(wrap_row()).with_children(|row| {
                for (caption, op) in [
                    ("Colocar", "vael.assets.place"),
                    ("Pincel", "vael.assets.brush"),
                    ("Parar", "vael.assets.stop"),
                    ("Raio −", "vael.assets.radius_down"),
                    ("Raio +", "vael.assets.radius_up"),
                    ("Densidade −", "vael.assets.density_down"),
                    ("Densidade +", "vael.assets.density_up"),
                ] {
                    row.spawn(button(ButtonProps::new(caption).call_operator(op.to_string())));
                }
            });
            root.spawn((wrap_row(), AssetList));
            text(
                root,
                "Escolhe um modelo e \"Colocar\" (um por clique) ou \"Pincel\" (espalha no \
                 raio ao clicar/arrastar). Esc ou \"Parar\" sai. Cada cópia herda colisor, \
                 colheita e LOD de uma instância existente; Ctrl+Z desfaz o traço.",
                11.0,
                Color::srgb(0.62, 0.62, 0.66),
                (),
            );
        });
}

/// Reconstrói os botões dos modelos e a linha de estado.
pub fn refresh_panel(
    mut commands: Commands,
    mut library: ResMut<AssetLibrary>,
    lists: Query<Entity, With<AssetList>>,
    mut status: Query<&mut Text, With<AssetStatus>>,
) {
    if let Ok(mut status) = status.single_mut() {
        let chosen = library
            .chosen
            .as_ref()
            .map_or("nenhum".to_string(), |c| format!("{} ({}×)", c.name, c.count));
        let mode = match library.mode {
            Mode::Off => "parado".to_string(),
            Mode::Place => "a colocar (clique no viewport)".to_string(),
            Mode::Brush => format!(
                "pincel: raio {} m, {} por toque",
                library.radius, library.density
            ),
        };
        let line = format!("{} · modelo: {chosen} · {mode}", library.kind.label());
        if status.0 != line {
            status.0 = line;
        }
    }
    if !library.dirty {
        return;
    }
    let Ok(list) = lists.single() else {
        return;
    };
    library.dirty = false;
    commands.entity(list).despawn_related::<Children>();
    let entries = library.entries.get(&library.kind).cloned().unwrap_or_default();
    commands.entity(list).with_children(|row| {
        if entries.is_empty() {
            row.spawn(Text::new("(nenhum modelo desta categoria na cena)"));
        }
        for (index, entry) in entries.iter().take(LIST_MAX).enumerate() {
            row.spawn((
                button(ButtonProps::new(format!("{} ({})", entry.name, entry.count))),
                AssetSlot(index),
            ));
        }
    });
}

/// Cliques nos botões de categoria/modelo (os botões do Jackdaw não passam
/// parâmetros ao operador, por isso o índice vai num componente).
pub fn on_click(
    click: On<bevy::ui_widgets::Activate>,
    slots: Query<&AssetSlot>,
    kinds: Query<&KindSlot>,
    mut library: ResMut<AssetLibrary>,
) {
    let target = click.entity;
    if let Ok(kind) = kinds.get(target) {
        library.kind = kind.0;
        library.dirty = true;
    } else if let Ok(slot) = slots.get(target) {
        let entry = library
            .entries
            .get(&library.kind)
            .and_then(|list| list.get(slot.0))
            .cloned();
        if let Some(entry) = entry {
            library.chosen = Some(entry);
            if library.mode == Mode::Off {
                library.mode = Mode::Place;
            }
        }
    }
}

macro_rules! library_op {
    ($name:ident, $id:tt, $label:tt, |$lib:ident| $body:block) => {
        #[operator(id = $id, label = $label, allows_undo = false)]
        pub fn $name(_params: In<OperatorParameters>, mut $lib: ResMut<AssetLibrary>) -> OperatorResult {
            $body;
            OperatorResult::Finished
        }
    };
}

library_op!(place, "vael.assets.place", "Vael: colocar o modelo escolhido", |lib| {
    lib.mode = Mode::Place;
});
library_op!(brush, "vael.assets.brush", "Vael: pincel de props", |lib| {
    lib.mode = Mode::Brush;
});
library_op!(stop, "vael.assets.stop", "Vael: parar de colocar", |lib| {
    lib.mode = Mode::Off;
});
library_op!(radius_up, "vael.assets.radius_up", "Vael: pincel maior", |lib| {
    lib.radius = (lib.radius * 1.5).min(80.0);
});
library_op!(radius_down, "vael.assets.radius_down", "Vael: pincel menor", |lib| {
    lib.radius = (lib.radius / 1.5).max(1.0);
});
library_op!(density_up, "vael.assets.density_up", "Vael: mais props por toque", |lib| {
    lib.density = (lib.density + 1).min(32);
});
library_op!(density_down, "vael.assets.density_down", "Vael: menos props por toque", |lib| {
    lib.density = lib.density.saturating_sub(1).max(1);
});

/// Carrega o `.ahgt` do `Terrain` da cena (uma vez por terreno).
fn ground(world: &mut World) -> bool {
    let root = project_root(world.get_resource::<SceneFilePath>());
    let spec = world
        .resource::<SceneIndex>()
        .nodes
        .values()
        .find_map(|info| info.terrain.clone());
    let (Some(root), Some((heightmap, size, max))) = (root, spec) else {
        return false;
    };
    let path = root.join(&heightmap);
    let key = (path.clone(), size.to_bits(), max.to_bits());
    if world.resource::<Ground>().key.as_ref() == Some(&key) {
        return world.resource::<Ground>().field.is_some();
    }
    let field = match AhgtHeightField::from_path_with(&path, Some(size), Some(max)) {
        Ok(field) => Some(field),
        Err(err) => {
            warn!("vael assets: heightmap {}: {err:#}", path.display());
            None
        }
    };
    let mut ground = world.resource_mut::<Ground>();
    ground.key = Some(key);
    ground.field = field;
    ground.field.is_some()
}

/// Primeiro ponto onde o raio desce abaixo do terreno (raymarch + bissecção).
fn hit_ground(field: &AhgtHeightField, ray: Ray3d) -> Option<Vec3> {
    let dir = *ray.direction;
    let above = |t: f32| {
        let p = ray.origin + dir * t;
        p.y - field.sample(p.x, p.z)
    };
    let mut t = 0.0;
    let step: f32 = 2.0;
    if above(0.0) < 0.0 {
        return None;
    }
    while t < 8000.0 {
        let next = t + step.max(t * 0.01);
        if above(next) < 0.0 {
            let (mut lo, mut hi) = (t, next);
            for _ in 0..20 {
                let mid = (lo + hi) * 0.5;
                if above(mid) < 0.0 {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let p = ray.origin + dir * hi;
            return Some(Vec3::new(p.x, field.sample(p.x, p.z), p.z));
        }
        t = next;
    }
    None
}

/// Cliques no viewport em modo Colocar/Pincel.
pub fn drive(
    mut commands: Commands,
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    cursor: ViewportCursor,
    mut library: ResMut<AssetLibrary>,
) {
    if library.mode == Mode::Off {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        library.mode = Mode::Off;
        return;
    }
    if cursor.blocked_by_overlay() || cursor.viewport_pointer().is_none() {
        return;
    }
    let Some(ray) = cursor.ray() else {
        return;
    };
    let fire = match library.mode {
        Mode::Place => mouse.just_pressed(MouseButton::Left),
        Mode::Brush => {
            library.stroke_timer -= time.delta_secs();
            let go = mouse.just_pressed(MouseButton::Left)
                || (mouse.pressed(MouseButton::Left) && library.stroke_timer <= 0.0);
            if go {
                library.stroke_timer = 0.2;
            }
            go
        }
        Mode::Off => false,
    };
    if !fire {
        return;
    }
    let Some(chosen) = library.chosen.clone() else {
        commands.queue(|world: &mut World| {
            warn_caller(world, "vael: escolhe primeiro um modelo no painel Vael: Assets");
        });
        return;
    };
    // Pontos: um no cursor (Colocar) ou `density` no círculo (Pincel).
    let mut offsets = vec![Vec2::ZERO];
    if library.mode == Mode::Brush {
        offsets.clear();
        for _ in 0..library.density {
            let angle = library.random() * std::f32::consts::TAU;
            let r = library.radius * library.random().sqrt();
            offsets.push(Vec2::new(angle.cos() * r, angle.sin() * r));
        }
    }
    let jitter: Vec<(f32, f32)> = offsets
        .iter()
        .map(|_| (library.random() * 360.0, 0.85 + library.random() * 0.3))
        .collect();
    let brush = library.mode == Mode::Brush;
    commands.queue(move |world: &mut World| {
        place_copies(world, &chosen, Some(ray), Vec3::ZERO, &offsets, &jitter, brush);
    });
}

/// Assenta cópias de `chosen` no chão: à volta do ponto onde `ray` toca o
/// terreno, ou de `at` (x, z) quando não há raio. Devolve quantas ficaram.
fn place_copies(
    world: &mut World,
    chosen: &AssetEntry,
    ray: Option<Ray3d>,
    at: Vec3,
    offsets: &[Vec2],
    jitter: &[(f32, f32)],
    brush: bool,
) -> usize {
    if !ground(world) {
        warn_caller(world, "vael: sem terreno (`Terrain` com heightmap) para assentar o modelo");
        return 0;
    }
    let ground = world.remove_resource::<Ground>().expect("ground");
    let field = ground.field.as_ref().expect("field");
    let center = match ray {
        Some(ray) => hit_ground(field, ray),
        None => Some(Vec3::new(at.x, field.sample(at.x, at.z), at.z)),
    };
    let Some(center) = center else {
        world.insert_resource(ground);
        return 0;
    };
    let points: Vec<Vec3> = offsets
        .iter()
        .map(|o| {
            let (x, z) = (center.x + o.x, center.z + o.y);
            Vec3::new(x, field.sample(x, z), z)
        })
        .collect();
    world.insert_resource(ground);
    let chosen = chosen.clone();
    let jitter = jitter.to_vec();

        let exemplar_scale = world
            .get::<GlobalTransform>(chosen.exemplar)
            .map_or(Vec3::ONE, |g| g.to_scale_rotation_translation().0);
        let label = if brush {
            format!("Vael: pincel {} ×{}", chosen.name, points.len())
        } else {
            format!("Vael: colocar {}", chosen.name)
        };
        // Chamadas aninhadas (dentro de um operador) não entram no histórico
        // por omissão: sem isto o traço não se desfazia.
        let history = CallOperatorSettings {
            creates_history_entry: true,
            ..default()
        };
        let placed = with_history_span(world, label, |world| {
            let mut placed = 0;
            for (point, (yaw, scale)) in points.iter().zip(&jitter) {
                if world
                    .operator("entity.duplicate")
                    .param("entity", chosen.exemplar)
                    .settings(history.clone())
                    .call()
                    .is_err()
                {
                    continue;
                }
                let Some(copy) = world.resource::<Selection>().primary() else {
                    continue;
                };
                let mut call = world
                    .operator("entity.set_transform")
                    .param("entity", copy)
                    .param("x", point.x as f64)
                    .param("y", point.y as f64)
                    .param("z", point.z as f64)
                    .param("world", true)
                    .settings(history.clone());
                // O pincel varia rotação e escala; "Colocar" mantém as do
                // exemplar.
                if brush {
                    let s = exemplar_scale * *scale;
                    call = call
                        .param("yaw", *yaw as f64)
                        .param("sx", s.x as f64)
                        .param("sy", s.y as f64)
                        .param("sz", s.z as f64);
                }
                let _ = call.call();
                // A cópia é um nó novo: a proveniência do original (ficheiro +
                // caminho) faria dois nós com a mesma origem no loader.
                for (field, value) in [("origin_file", "\"\""), ("origin_path", "[]")] {
                    let _ = world
                        .operator("component.set")
                        .param("entity", copy)
                        .param("type_path", WORLD_NODE.to_string())
                        .param("field", field.to_string())
                        .param("value", value.to_string())
                        .settings(history.clone())
                        .call();
                }
                placed += 1;
            }
            placed
        });
        if placed == 0 {
            warn_caller(world, format!("vael: não foi possível copiar {}", chosen.name));
        }
        placed
}

#[operator(
    id = "vael.assets.place_at",
    label = "Vael: colocar um modelo em (x, z)",
    description = "Coloca `count` cópias do modelo `url` (uma instância existente é \
                   copiada, com colisor/colheita/LOD) num raio à volta de (x, z), \
                   assentes no terreno. Um passo de undo.",
    allows_undo = false,
    params(
        url(String, doc = "Modelo, ex. /assets/meshes/forest/pine_dark_lod0.glb."),
        x(f64, doc = "X do centro (mundo)."),
        z(f64, doc = "Z do centro (mundo)."),
        count(i64, doc = "Cópias (1 a 64). Defaults to 1."),
        radius(f64, doc = "Raio de espalhamento em metros. Defaults to 0.")
    )
)]
pub fn place_at(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some(url) = params.as_str("url").map(str::to_string) else {
        warn_caller(world, "vael: falta `url`");
        return OperatorResult::Cancelled;
    };
    let x = params.as_float("x").unwrap_or(0.0) as f32;
    let z = params.as_float("z").unwrap_or(0.0) as f32;
    let count = params.as_int("count").unwrap_or(1).clamp(1, 64) as usize;
    let radius = params.as_float("radius").unwrap_or(0.0).max(0.0) as f32;
    let chosen = world
        .resource::<AssetLibrary>()
        .entries
        .values()
        .flatten()
        .find(|entry| entry.url == url || entry.url.trim_start_matches('/') == url.trim_start_matches('/'))
        .cloned();
    let Some(chosen) = chosen else {
        warn_caller(world, format!("vael: `{url}` não tem nenhuma instância na cena para copiar"));
        return OperatorResult::Cancelled;
    };
    let (offsets, jitter) = {
        let mut library = world.resource_mut::<AssetLibrary>();
        let offsets: Vec<Vec2> = (0..count)
            .map(|i| {
                if count == 1 && radius == 0.0 {
                    return Vec2::ZERO;
                }
                let angle = library.random() * std::f32::consts::TAU + i as f32;
                let r = radius * library.random().sqrt();
                Vec2::new(angle.cos() * r, angle.sin() * r)
            })
            .collect();
        let jitter: Vec<(f32, f32)> = offsets
            .iter()
            .map(|_| (library.random() * 360.0, 0.85 + library.random() * 0.3))
            .collect();
        (offsets, jitter)
    };
    let brush = count > 1 || radius > 0.0;
    match place_copies(world, &chosen, None, Vec3::new(x, 0.0, z), &offsets, &jitter, brush) {
        0 => OperatorResult::Cancelled,
        _ => OperatorResult::Finished,
    }
}
