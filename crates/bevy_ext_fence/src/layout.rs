//! Layout de uma cerca: a linha autorada → postes → peças.
//!
//! Puro (sem ECS) e determinístico: recebe o [`FenceSpec`] já resolvido com a
//! predefinição, a moldura da entidade (onde está no mundo) e o chão, e devolve
//! um [`FenceLayout`] que [`crate::mesh`] transforma em malhas. A variação
//! (inclinação, tom, comprimento das tábuas) sai de um RNG semeado pela chave
//! da cerca: a mesma cerca nasce igual em todas as sessões.
//!
//! Regras:
//!
//! * **Postes** nos vértices e intervalados para que nenhum vão passe de
//!   `spacing`; o último vão divide-se por igual (sem tocos). Num perímetro
//!   fechado não há poste repetido na junção. Os de canto, de ponta e de
//!   portão são mais fortes. Todos descem `bury` abaixo do chão.
//! * **Chão**: a base de cada poste é a cota do terreno nesse ponto (quando o
//!   host dá chão e a cerca não diz `fence-follow="points"`).
//! * **Travessas** entre postes, com as pontas a sair da face (espigão e
//!   morta); seguem a rampa até ~15°, em degraus acima disso.
//! * **Tábuas** pregadas à frente das travessas, com o comprimento sorteado
//!   dentro de `board_length` — é daí que vêm os topos desiguais.

use std::fmt;

use bevy::prelude::*;
use bevy_ext_scene::FenceSpec;

use crate::points::parse_points;
use crate::specs::{BoardTop, Brace, FenceKind, FenceStyle, SlopeFollow, Timber};

/// Rampa a partir da qual as travessas passam a degraus (graus).
pub const STEP_SLOPE_DEG: f32 = 15.0;
/// Desvio a partir do qual um vértice é canto (graus).
const CORNER_DEG: f32 = 5.0;

/// Onde a entidade da cerca está no mundo: só a rotação em Y e a escala
/// uniforme entram na conta (uma cerca inclinada não segue o chão).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FenceFrame {
    pub origin: Vec3,
    pub yaw: f32,
    pub scale: f32,
}

impl Default for FenceFrame {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            yaw: 0.0,
            scale: 1.0,
        }
    }
}

/// Cota do chão num ponto do mundo; `None` quando não há chão ali.
pub type Ground<'a> = &'a dyn Fn(f32, f32) -> Option<f32>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// `fence-points` não é uma lista de pontos.
    Points,
    /// Menos de dois pontos (menos de três, se fechada).
    TooShort,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Points => write!(f, "fence-points não é uma lista de pontos"),
            Self::TooShort => write!(f, "a cerca precisa de pelo menos dois pontos"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// O papel de uma peça — para quem quiser variar o material por peça.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Post,
    Rail,
    Brace,
    Board,
    Stake,
    Slat,
    Hardware,
}

/// De onde vem o material de uma peça.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    /// O conjunto da predefinição.
    Primary,
    /// As varas e as cordas (`secondary_material`, quando há).
    Secondary,
    /// Ferro batido (material liso, sem textura).
    Iron,
}

/// Um poste: a base ao nível do chão e o que se vê acima dele.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Post {
    /// Pé do poste, ao nível do chão (espaço local da cerca).
    pub base: Vec3,
    /// O que aparece acima do chão (m).
    pub height: f32,
    /// Largura ao longo da cerca e espessura de através, já com o reforço.
    pub width: f32,
    pub depth: f32,
    pub strong: bool,
    /// Direcção da cerca no poste (a bissectriz, num canto).
    pub along: Vec3,
}

impl Post {
    /// Meio da maior dimensão da secção: até onde a travessa entra no poste.
    pub fn half(&self) -> f32 {
        0.5 * self.width.max(self.depth)
    }
}

/// Um vão entre dois postes consecutivos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub a: usize,
    pub b: usize,
}

/// Uma peça recta: poste, travessa, tábua, ripa, ferragem.
#[derive(Clone, Debug, PartialEq)]
pub struct Beam {
    pub from: Vec3,
    pub to: Vec3,
    /// Eixo da largura do perfil, perpendicular ao eixo da peça.
    pub across: Vec3,
    pub timber: Timber,
    /// Escala do anel de chegada (largura, profundidade): `(0, 1)` é uma ponta.
    pub end_scale: Vec2,
    /// Comprimento do topo em pirâmide para além de `to`.
    pub apex: f32,
    /// Se cada ponta leva tampa (as pontas encaixadas não precisam).
    pub caps: [bool; 2],
    pub role: Role,
    pub slot: Slot,
    /// Desvio da textura por peça: duas peças com o mesmo veio denunciam-se.
    pub uv_offset: Vec2,
    /// Tinta da peça (multiplicador linear).
    pub tint: Vec3,
    /// Cota local do chão por baixo: é dele que conta o desgaste da base.
    pub ground_y: f32,
}

/// Uma peça curva: corda em catenária, vara tecida.
#[derive(Clone, Debug, PartialEq)]
pub struct Tube {
    pub path: Vec<Vec3>,
    /// Raio em cada ponto.
    pub radius: Vec<f32>,
    pub sides: u32,
    pub slot: Slot,
    pub uv_offset: Vec2,
    pub tint: Vec3,
    pub ground_y: f32,
}

/// Um prego: disco pequeno virado para a cara de fora.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stud {
    pub at: Vec3,
    pub normal: Vec3,
    pub radius: f32,
}

/// Tudo o que a cerca tem, antes de virar malha.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FenceLayout {
    pub posts: Vec<Post>,
    pub spans: Vec<Span>,
    pub beams: Vec<Beam>,
    pub tubes: Vec<Tube>,
    pub studs: Vec<Stud>,
}

impl FenceLayout {
    /// Altura máxima acima do chão (o colisor e o enquadramento usam-na).
    pub fn max_height(&self) -> f32 {
        self.posts
            .iter()
            .map(|post| post.base.y + post.height)
            .chain(self.beams.iter().flat_map(|beam| [beam.from.y, beam.to.y + beam.apex]))
            .fold(f32::NEG_INFINITY, f32::max)
    }
}

/// splitmix64: determinístico e sem dependências.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniforme em `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    /// Uniforme em `[-amount, amount]`.
    pub fn sym(&mut self, amount: f32) -> f32 {
        (self.unit() * 2.0 - 1.0) * amount
    }
}

fn horizontal(direction: Vec3) -> Vec3 {
    let flat = Vec3::new(direction.x, 0.0, direction.z);
    flat.normalize_or_zero()
}

/// Cara de fora da cerca num vão: à direita de quem a percorre de `a` para `b`.
fn normal_of(direction: Vec3) -> Vec3 {
    direction.cross(Vec3::Y).normalize_or_zero()
}

fn base_height(point: Vec3, world: Vec3, frame: &FenceFrame, follow: bool, ground: Ground) -> f32 {
    if !follow {
        return point.y;
    }
    let scale = frame.scale.abs().max(1e-4);
    ground(world.x, world.z).map_or(point.y, |height| (height - frame.origin.y) / scale)
}

/// O layout de uma cerca, ou porquê não há cerca.
///
/// `key` é a semente da variação (o nome ou a origem da cerca, depois de
/// hash); `ground` dá a cota do chão em coordenadas do mundo.
#[allow(clippy::too_many_lines)]
pub fn layout(
    spec: &FenceSpec,
    style: &FenceStyle,
    frame: &FenceFrame,
    ground: Ground,
    key: u64,
) -> Result<FenceLayout, LayoutError> {
    let points = parse_points(&spec.points).ok_or(LayoutError::Points)?;
    let closed = spec.closed.unwrap_or(false);
    if points.len() < 2 || (closed && points.len() < 3) {
        return Err(LayoutError::TooShort);
    }
    let follow = spec.follow.as_deref() != Some("points");
    let rotation = Quat::from_rotation_y(frame.yaw);
    let scale = frame.scale.abs().max(1e-4);
    let to_world = |point: Vec3| frame.origin + rotation * (point * scale);
    let count = points.len();
    let gate = style.kind == FenceKind::Gate;

    // — postes ————————————————————————————————————————————————————————
    let mut rng = Rng::new(key ^ (u64::from(style.seed) << 1));
    let make_post = |position: Vec3, along: Vec3, strong: bool, rng: &mut Rng| -> Post {
        let factor = if strong { style.corner_scale.max(1.0) } else { 1.0 };
        Post {
            base: position,
            height: (style.height + rng.sym(0.015)).max(0.2),
            // A secção oscila um pouco: dois postes iguais a fio denunciam a
            // moldura. O yaw roda a peça (aplica-o a malha, pelo `along`).
            width: style.post.width * factor * rng.range(0.96, 1.04),
            depth: style.post.depth * factor * rng.range(0.96, 1.04),
            strong,
            along,
        }
    };

    let segments: Vec<(usize, usize)> = if closed {
        (0..count).map(|index| (index, (index + 1) % count)).collect()
    } else {
        (0..count - 1).map(|index| (index, index + 1)).collect()
    };
    let directions: Vec<Vec3> = segments
        .iter()
        .map(|&(a, b)| horizontal(points[b] - points[a]))
        .collect();

    let vertex_post = |index: usize, rng: &mut Rng| -> Post {
        let point = points[index];
        let base_y = base_height(point, to_world(point), frame, follow, ground);
        let (previous, next) = if closed {
            (
                directions[(index + count - 1) % count],
                directions[index % count],
            )
        } else {
            (
                directions[index.saturating_sub(1)],
                directions[index.min(count - 2)],
            )
        };
        // Canto: a linha muda de direcção no vértice. A secção do poste
        // alinha-se com a bissectriz, para as travessas entrarem de lado.
        let corner = previous.dot(next) < CORNER_DEG.to_radians().cos();
        let end = !closed && (index == 0 || index == count - 1);
        make_post(
            Vec3::new(point.x, base_y, point.z),
            horizontal(previous + next),
            corner || end || gate,
            rng,
        )
    };

    let mut posts: Vec<Post> = Vec::new();
    if gate {
        posts.push(vertex_post(0, &mut rng));
        posts.push(vertex_post(count - 1, &mut rng));
    } else {
        for (index, &(a, b)) in segments.iter().enumerate() {
            let vertex = vertex_post(a, &mut rng);
            posts.push(vertex);
            let (pa, pb) = (points[a], points[b]);
            let flat = Vec3::new(pb.x - pa.x, 0.0, pb.z - pa.z);
            let length = flat.length();
            let pieces = (length / style.spacing.max(0.05)).ceil().max(1.0) as usize;
            for step in 1..pieces {
                let t = step as f32 / pieces as f32;
                let point = pa.lerp(pb, t);
                let base_y = base_height(point, to_world(point), frame, follow, ground);
                posts.push(make_post(
                    Vec3::new(point.x, base_y, point.z),
                    horizontal(flat),
                    false,
                    &mut rng,
                ));
            }
            if !closed && index == segments.len() - 1 {
                posts.push(vertex_post(b, &mut rng));
            }
        }
    }

    let spans: Vec<Span> = if gate {
        vec![Span { a: 0, b: 1 }]
    } else {
        (0..posts.len() - 1)
            .map(|index| Span {
                a: index,
                b: index + 1,
            })
            .chain(closed.then(|| Span {
                a: posts.len() - 1,
                b: 0,
            }))
            .collect()
    };

    let mut layout = FenceLayout {
        posts,
        spans,
        ..Default::default()
    };
    let mut pieces = Pieces {
        layout: &mut layout,
        rng,
        style,
    };
    pieces.posts();
    let rails = style.rail_fractions();
    match style.kind {
        FenceKind::Rail | FenceKind::Crossbuck => pieces.rails(rails.clone(), 0.0),
        FenceKind::Plank => {
            pieces.rails(rails.clone(), 0.0);
            pieces.boards(style.board, style.board_top, style.boards, style.board_gap);
        }
        FenceKind::Picket => {
            pieces.rails(rails.clone(), 0.0);
            pieces.boards(style.board, BoardTop::Point, style.boards, style.board_gap);
        }
        FenceKind::Palisade => {
            pieces.rails(rails, -(style.board.depth + style.rail.depth) * 0.5);
            pieces.boards(style.board, style.board_top, 0, style.board_gap);
        }
        FenceKind::Wattle => {
            pieces.rails(Vec::new(), 0.0);
            pieces.stakes();
            pieces.rods();
        }
        FenceKind::Lattice => {
            pieces.rails(style.rail_fractions(), 0.0);
            pieces.lattice();
        }
        FenceKind::Rope => {
            pieces.rails(Vec::new(), 0.0);
            pieces.ropes();
        }
        FenceKind::Gate => pieces.gate(style.gate_open_deg),
    }
    Ok(layout)
}

/// Acrescenta peças ao layout com a variação comum (tom, desvio da textura).
struct Pieces<'a> {
    layout: &'a mut FenceLayout,
    rng: Rng,
    style: &'a FenceStyle,
}

impl Pieces<'_> {
    fn tint(&mut self) -> Vec3 {
        let brightness = 1.0 + self.rng.sym(self.style.tint_spread);
        let warm = self.rng.sym(0.012);
        Vec3::new(brightness * (1.0 + warm), brightness, brightness * (1.0 - warm))
    }

    fn uv(&mut self) -> Vec2 {
        Vec2::new(self.rng.unit(), self.rng.unit())
    }

    fn push_beam(&mut self, mut beam: Beam) {
        if beam.from.distance_squared(beam.to) < 1e-6 {
            return;
        }
        beam.tint = self.tint();
        beam.uv_offset = self.uv();
        self.layout.beams.push(beam);
    }

    #[allow(clippy::too_many_arguments)]
    fn beam(
        &mut self,
        from: Vec3,
        to: Vec3,
        across: Vec3,
        timber: Timber,
        role: Role,
        slot: Slot,
        ground_y: f32,
    ) {
        self.push_beam(Beam {
            from,
            to,
            across,
            timber,
            end_scale: Vec2::ONE,
            apex: 0.0,
            caps: [true; 2],
            role,
            slot,
            uv_offset: Vec2::ZERO,
            tint: Vec3::ONE,
            ground_y,
        });
    }

    fn tube(&mut self, path: Vec<Vec3>, radius: Vec<f32>, sides: u32, slot: Slot, ground_y: f32) {
        if path.len() < 2 {
            return;
        }
        let (uv_offset, tint) = (self.uv(), self.tint());
        self.layout.tubes.push(Tube {
            path,
            radius,
            sides,
            slot,
            uv_offset,
            tint,
            ground_y,
        });
    }

    fn secondary(&self) -> Slot {
        if self.style.secondary_material.trim().is_empty() {
            Slot::Primary
        } else {
            Slot::Secondary
        }
    }

    /// Os postes em si: do enterrado ao topo, já com o reforço e a secção de
    /// cada um. O topo leva um bisel pequeno (nas paliçadas, a ponta do tronco).
    fn posts(&mut self) {
        let posts = self.layout.posts.clone();
        let tilt = self.style.post_tilt_deg.to_radians().tan();
        for post in posts {
            let pointed = self.style.kind == FenceKind::Palisade;
            // A inclinação empurra o topo para um lado ao acaso (até `post_tilt_deg`).
            let lean = Vec2::new(self.rng.sym(1.0), self.rng.sym(1.0)).normalize_or_zero();
            let lean = Vec3::new(lean.x, 0.0, lean.y) * (post.height * tilt * self.rng.unit());
            self.push_beam(Beam {
                from: post.base - Vec3::Y * self.style.bury,
                to: post.base + Vec3::Y * post.height + lean,
                across: post.along,
                timber: Timber {
                    width: post.width,
                    depth: post.depth,
                    ..self.style.post
                },
                end_scale: if pointed {
                    Vec2::new(0.0, 0.9)
                } else {
                    Vec2::splat(0.86)
                },
                apex: if pointed { 0.0 } else { 0.012 },
                caps: [false, false],
                role: Role::Post,
                slot: Slot::Primary,
                uv_offset: Vec2::ZERO,
                tint: Vec3::ONE,
                ground_y: post.base.y,
            });
        }
    }

    /// As alturas de uma fracção de travessa nos dois postes do vão.
    fn rail_ends(&self, a: &Post, b: &Post, fraction: f32, step: bool) -> (f32, f32) {
        let (ya, yb) = (a.base.y + fraction * a.height, b.base.y + fraction * b.height);
        if step {
            (ya.min(yb), ya.min(yb))
        } else {
            (ya, yb)
        }
    }

    fn slope_steps(&self, a: &Post, b: &Post) -> bool {
        let rise = b.base.y - a.base.y;
        let run = Vec2::new(b.base.x - a.base.x, b.base.z - a.base.z).length();
        self.style.slope_follow == SlopeFollow::Step
            || rise.abs() > run.max(1e-4) * STEP_SLOPE_DEG.to_radians().tan()
    }

    /// Postes dos vãos: as travessas (com as pontas a sair da face), as
    /// diagonais e o remate. `offset` afasta-as da linha da cerca.
    fn rails(&mut self, fractions: Vec<f32>, offset: f32) {
        let spans = self.layout.spans.clone();
        for span in spans {
            let (a, b) = (self.layout.posts[span.a], self.layout.posts[span.b]);
            let flat = Vec3::new(b.base.x - a.base.x, 0.0, b.base.z - a.base.z);
            let direction = horizontal(flat);
            if direction == Vec3::ZERO {
                continue;
            }
            let normal = normal_of(direction);
            let step = self.slope_steps(&a, &b);
            let shift = normal * offset;
            let half = |post: &Post| 0.5 * post.along.dot(direction).abs() * post.width.max(post.depth);
            for fraction in fractions.iter().copied() {
                let (mut ya, mut yb) = self.rail_ends(&a, &b, fraction, step);
                ya += self.rng.sym(0.015);
                yb += self.rng.sym(0.015);
                let from = Vec3::new(a.base.x, ya, a.base.z) - direction * (half(&a) + self.style.tenon) + shift;
                let to = Vec3::new(b.base.x, yb, b.base.z) + direction * (half(&b) + self.style.tenon) + shift;
                self.beam(from, to, Vec3::Y, self.style.rail, Role::Rail, Slot::Primary, a.base.y);
            }
            if matches!(self.style.kind, FenceKind::Crossbuck) {
                self.brace(&a, &b, normal, offset);
            }
        }
    }

    fn brace(&mut self, a: &Post, b: &Post, normal: Vec3, offset: f32) {
        let single = self.style.brace == Brace::Single;
        let (low, high) = (0.1, 0.9);
        let (la, lb) = (a.base.y + low * a.height, b.base.y + low * b.height);
        let (ha, hb) = (a.base.y + high * a.height, b.base.y + high * b.height);
        let start = Vec3::new(a.base.x, 0.0, a.base.z);
        let end = Vec3::new(b.base.x, 0.0, b.base.z);
        let shift = normal * offset;
        let timber = self.style.rail;
        let diagonal = |pieces: &mut Self, from: Vec3, to: Vec3| {
            let axis = (to - from).normalize_or_zero();
            let across = normal.cross(axis);
            pieces.beam(from, to, across, timber, Role::Brace, Slot::Primary, a.base.y);
        };
        diagonal(self, start + Vec3::Y * la + shift, end + Vec3::Y * hb + shift);
        if !single {
            diagonal(self, start + Vec3::Y * ha + shift, end + Vec3::Y * lb + shift);
        }
    }

    /// Tábuas ou troncos verticais por vão. `count` é o número por vão (0 =
    /// caberem pela largura e pelo vão); `nailed` põe-nos à frente das travessas.
    fn boards(&mut self, timber: Timber, top: BoardTop, count: u32, gap: f32) {
        let nailed = matches!(self.style.kind, FenceKind::Plank | FenceKind::Picket);
        let spans = self.layout.spans.clone();
        for span in spans {
            let (a, b) = (self.layout.posts[span.a], self.layout.posts[span.b]);
            let flat = Vec3::new(b.base.x - a.base.x, 0.0, b.base.z - a.base.z);
            let direction = horizontal(flat);
            if direction == Vec3::ZERO {
                continue;
            }
            let normal = normal_of(direction);
            let start = Vec3::new(a.base.x, 0.0, a.base.z) + direction * a.half();
            let end = Vec3::new(b.base.x, 0.0, b.base.z) - direction * b.half();
            let usable = (end - start).length();
            if usable < timber.width {
                continue;
            }
            let fit = ((usable / (timber.width + gap)).floor() as u32).max(1);
            let count = if count > 0 { count } else { fit }.max(1);
            let pitch = usable / count as f32;
            let shift = if nailed {
                normal * (self.style.rail.depth + timber.depth) * 0.5
            } else {
                Vec3::ZERO
            };
            for index in 0..count {
                let t = (index as f32 + 0.5) / count as f32;
                let foot = start.lerp(end, t);
                let ground_y = a.base.y.lerp(b.base.y, t);
                let width = (timber.width * self.rng.range(0.92, 1.08)).min(pitch - gap).max(0.02);
                let mut beam = Beam {
                    from: foot + Vec3::Y * 0.03 + shift,
                    to: foot + Vec3::Y * (0.03 + self.rng.range(
                        self.style.board_length.x.min(self.style.board_length.y),
                        self.style.board_length.x.max(self.style.board_length.y),
                    )) + shift + direction * self.rng.sym(0.012),
                    across: direction,
                    timber: Timber { width, ..timber },
                    end_scale: Vec2::ONE,
                    apex: 0.0,
                    caps: [true; 2],
                    role: Role::Board,
                    slot: Slot::Primary,
                    uv_offset: Vec2::ZERO,
                    tint: Vec3::ONE,
                    ground_y,
                };
                match top {
                    BoardTop::Flat => {}
                    BoardTop::Chamfer => {
                        beam.end_scale = Vec2::new(0.72, 0.5);
                        beam.apex = 0.012;
                    }
                    BoardTop::Point => beam.end_scale = Vec2::new(0.0, 0.9),
                }
                // Até onde a tábua apanha uma travessa (para os pregos).
                let (foot_y, head_y) = (beam.from.y, beam.to.y);
                self.push_beam(beam);
                // Pregos: um par em cada travessa que a tábua apanha.
                if nailed {
                    let face = foot + shift + normal * (timber.depth * 0.5 + 0.001);
                    let step = self.slope_steps(&a, &b);
                    for fraction in self.style.rail_fractions() {
                        let (y, _) = self.rail_ends(&a, &b, fraction, step);
                        if y > foot_y + 0.1 && y < head_y - 0.05 {
                            for side in [-0.3, 0.3] {
                                self.layout.studs.push(Stud {
                                    at: face + Vec3::Y * y + direction * (width * side),
                                    normal,
                                    radius: 0.009,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    /// Estacas das varas entretecidas.
    fn stakes(&mut self) {
        let spans = self.layout.spans.clone();
        for span in spans {
            let (a, b) = (self.layout.posts[span.a], self.layout.posts[span.b]);
            let direction = horizontal(b.base - a.base);
            let start = a.base + direction * a.half();
            let end = b.base - direction * b.half();
            let usable = (end - start).length();
            let count = ((usable / self.style.stake_pitch.max(0.05)).round() as usize).max(1) + 1;
            let timber = self.style.stake;
            for index in 0..count {
                let t = if count == 1 { 0.5 } else { index as f32 / (count - 1) as f32 };
                let foot = start.lerp(end, t);
                let height = self.style.height + 0.12 + self.rng.sym(0.02);
                let mut beam = Beam {
                    from: foot - Vec3::Y * self.style.bury,
                    to: foot + Vec3::Y * height + direction * self.rng.sym(0.01),
                    across: direction,
                    timber,
                    end_scale: Vec2::splat(0.5),
                    apex: 0.03,
                    caps: [false, false],
                    role: Role::Stake,
                    slot: Slot::Primary,
                    uv_offset: Vec2::ZERO,
                    tint: Vec3::ONE,
                    ground_y: foot.y,
                };
                beam.from.y = foot.y - self.style.bury;
                self.push_beam(beam);
            }
        }
        // Os postes da linha também são varas: ficam com a secção da estaca.
        for post in &mut self.layout.beams {
            post.timber = self.style.stake;
            post.end_scale = Vec2::splat(0.55);
            post.apex = 0.03;
        }
    }

    /// As varas, tecidas à frente e atrás das estacas.
    fn rods(&mut self) {
        let spans = self.layout.spans.clone();
        let slot = self.secondary();
        let rod = self.style.rod;
        let stake = self.style.stake;
        let amplitude = (stake.width + rod.width) * 0.5 * 0.92;
        for span in spans {
            let (a, b) = (self.layout.posts[span.a], self.layout.posts[span.b]);
            let direction = horizontal(b.base - a.base);
            if direction == Vec3::ZERO {
                continue;
            }
            let normal = normal_of(direction);
            let start = a.base + direction * a.half();
            let end = b.base - direction * b.half();
            let usable = (end - start).length();
            let stakes = ((usable / self.style.stake_pitch.max(0.05)).round() as usize).max(1);
            let pitch = usable / stakes as f32;
            let samples = (usable / (pitch / 6.0)).ceil().max(8.0) as usize;
            for row in 0..self.style.rod_rows.max(1) {
                let height = 0.1 + row as f32 * rod.width * 1.02;
                let mut path = Vec::with_capacity(samples + 1);
                let mut radius = Vec::with_capacity(samples + 1);
                for sample in 0..=samples {
                    let t = sample as f32 / samples as f32;
                    let along = start.lerp(end, t);
                    let weave = amplitude * (std::f32::consts::PI * (t * stakes as f32 + row as f32)).cos();
                    path.push(along + normal * weave + Vec3::Y * height);
                    radius.push(rod.width * 0.5 * (1.0 - 0.35 * t));
                }
                let ground_y = a.base.y.min(b.base.y);
                self.tube(path, radius, rod.sides.max(3), slot, ground_y);
            }
        }
    }

    /// Ripas diagonais nos dois sentidos, recortadas à moldura do vão.
    fn lattice(&mut self) {
        let spans = self.layout.spans.clone();
        let slat = self.style.slat;
        let (low, high) = rail_bounds(&self.style.rail_fractions());
        let angle = self.style.slat_angle_deg.to_radians();
        for span in spans {
            let (a, b) = (self.layout.posts[span.a], self.layout.posts[span.b]);
            let direction = horizontal(b.base - a.base);
            if direction == Vec3::ZERO {
                continue;
            }
            let normal = normal_of(direction);
            let start = a.base + direction * a.half();
            let end = b.base - direction * b.half();
            let length = (end - start).length();
            let ground_y = (a.base.y + b.base.y) * 0.5;
            let height = (a.height + b.height) * 0.5;
            let bounds_lo = Vec2::new(0.0, low * height);
            let bounds_hi = Vec2::new(length, high * height);
            for (sign, shift) in [(1.0, 1.0), (-1.0, -1.0)] {
                let axis = Vec2::new(angle.cos() * sign, angle.sin());
                let across = direction * (-axis.y * sign) + Vec3::Y * axis.x;
                let side = normal * shift * (slat.depth * 0.5 + 0.004);
                let lo = bounds_lo;
                let hi = bounds_hi;
                let corners = [lo, hi, Vec2::new(lo.x, hi.y), Vec2::new(hi.x, lo.y)];
                let (mut c_min, mut c_max) = (f32::INFINITY, f32::NEG_INFINITY);
                for corner in corners {
                    let c = corner.dot(Vec2::new(-axis.y, axis.x));
                    c_min = c_min.min(c);
                    c_max = c_max.max(c);
                }
                let pitch = self.style.slat_pitch.max(0.05);
                let mut c = c_min + pitch * 0.5;
                while c < c_max {
                    let origin = Vec2::new(-axis.y, axis.x) * c;
                    if let Some((t0, t1)) = clip_line(origin, axis, lo, hi)
                        && t1 - t0 > 0.15
                    {
                        let at = |t: f32| start + direction * (origin + axis * t).x + Vec3::Y * (origin + axis * t).y + side;
                        self.beam(at(t0), at(t1), across, slat, Role::Slat, Slot::Primary, ground_y);
                    }
                    c += pitch;
                }
            }
        }
    }

    /// Cordas em catenária entre as caras de fora dos postes, com um nó em
    /// cada ponta.
    fn ropes(&mut self) {
        let spans = self.layout.spans.clone();
        let slot = self.secondary();
        let rope = self.style.rope;
        let radius = rope.width * 0.5;
        for span in spans {
            let (a, b) = (self.layout.posts[span.a], self.layout.posts[span.b]);
            let direction = horizontal(b.base - a.base);
            if direction == Vec3::ZERO {
                continue;
            }
            let normal = normal_of(direction);
            let start = a.base + direction * a.half();
            let end = b.base - direction * b.half();
            let length = (end - start).length();
            for fraction in self.style.rail_fractions() {
                let (mut ya, mut yb) = self.rail_ends(&a, &b, fraction, false);
                ya += self.rng.sym(0.01);
                yb += self.rng.sym(0.01);
                let from = Vec3::new(start.x, ya, start.z) + normal * (a.depth * 0.5 + radius);
                let to = Vec3::new(end.x, yb, end.z) + normal * (b.depth * 0.5 + radius);
                let sag = self.style.rope_sag.clamp(0.0, 0.4) * length;
                let sample = |t: f32| from.lerp(to, t) - Vec3::Y * (sag * (std::f32::consts::PI * t).sin());
                let samples = 24;
                let path: Vec<Vec3> = (0..=samples).map(|index| sample(index as f32 / samples as f32)).collect();
                self.tube(path, vec![radius; samples + 1], rope.sides.max(3), slot, a.base.y);
                for (ts, mirrored) in [([0.0, 0.02, 0.05, 0.08, 0.1], false), ([0.0, 0.02, 0.05, 0.08, 0.1], true)] {
                    let points: Vec<Vec3> = ts
                        .iter()
                        .map(|&t| sample(if mirrored { 1.0 - t } else { t }))
                        .collect();
                    let bulge = [1.0, 1.9, 2.0, 1.9, 1.0].map(|factor| radius * factor);
                    self.tube(points, bulge.to_vec(), rope.sides.max(3), slot, a.base.y);
                }
            }
        }
    }

    /// A folha do portão: dois montantes, travessas, diagonal, tábuas e as
    /// ferragens; `open_deg` roda a folha no gonzo.
    fn gate(&mut self, open_deg: f32) {
        let Some(span) = self.layout.spans.first().copied() else {
            return;
        };
        let (a, b) = (self.layout.posts[span.a], self.layout.posts[span.b]);
        let direction = horizontal(b.base - a.base);
        if direction == Vec3::ZERO {
            return;
        }
        let normal = normal_of(direction);
        let hinge = a.base + direction * (a.half() + 0.02);
        let latch = b.base - direction * (b.half() + 0.02);
        let floor = a.base.y.max(b.base.y);
        let (bottom, top) = (floor + 0.12, floor + self.style.height - 0.12);
        let ground_y = floor;
        let front = normal * (self.style.board.depth * 0.5 + self.style.rail.depth * 0.5);
        let at = |u: f32, y: f32| Vec3::new(hinge.x, y, hinge.z) + direction * u;

        // Tábuas da folha, de montante a montante.
        let stile = self.style.board.width * 1.25;
        let inner = (latch - hinge).length() - stile * 2.0 - 0.04;
        let board = self.style.board;
        let count = if self.style.boards > 0 {
            self.style.boards
        } else {
            ((inner / (board.width + self.style.board_gap)).floor() as u32).max(1)
        };
        let pitch = inner / count as f32;
        let leaf_start = self.layout.beams.len();
        for index in 0..count {
            let u = stile + 0.02 + pitch * (index as f32 + 0.5);
            self.beam(
                at(u, bottom),
                at(u, top),
                direction,
                Timber {
                    width: board.width.min(pitch - self.style.board_gap).max(0.03),
                    ..board
                },
                Role::Board,
                Slot::Primary,
                ground_y,
            );
        }
        for u in [stile * 0.5, (latch - hinge).length() - stile * 0.5] {
            self.beam(
                at(u, bottom),
                at(u, top),
                direction,
                Timber { width: stile, ..board },
                Role::Board,
                Slot::Primary,
                ground_y,
            );
        }
        let width = (latch - hinge).length();
        let rail = Timber {
            width: self.style.rail.width * 1.3,
            ..self.style.rail
        };
        for y in [bottom + 0.07, top - 0.07] {
            self.beam(
                at(0.0, y) + front,
                at(width, y) + front,
                Vec3::Y,
                rail,
                Role::Rail,
                Slot::Primary,
                ground_y,
            );
        }
        let brace_from = at(0.0, bottom + 0.1) + front;
        let brace_to = at(width, top - 0.1) + front;
        let axis = (brace_to - brace_from).normalize_or_zero();
        self.beam(
            brace_from,
            brace_to,
            normal.cross(axis),
            rail,
            Role::Brace,
            Slot::Primary,
            ground_y,
        );
        let iron = Timber::hewn(0.035, 0.008, 0.0);
        let latch_from = at(width - 0.14, (bottom + top) * 0.5) + front;
        self.beam(
            latch_from,
            latch_from + direction * 0.24,
            Vec3::Y,
            Timber { width: 0.03, ..iron },
            Role::Hardware,
            Slot::Iron,
            ground_y,
        );
        let leaf_end = self.layout.beams.len();
        // As dobadouras ficam com o poste: não rodam com a folha.
        let strap_front = normal * (self.style.board.depth * 0.5 + 0.008);
        for t in [0.18, 0.82] {
            let y = bottom + (top - bottom) * t;
            self.beam(
                at(-0.02, y) + strap_front,
                at(0.42, y) + strap_front,
                Vec3::Y,
                iron,
                Role::Hardware,
                Slot::Iron,
                ground_y,
            );
        }
        let open = -open_deg.to_radians();
        if open != 0.0 {
            let rotation = Quat::from_rotation_y(open);
            for beam in &mut self.layout.beams[leaf_start..leaf_end] {
                for point in [&mut beam.from, &mut beam.to] {
                    *point = hinge + rotation * (*point - hinge);
                }
                beam.across = rotation * beam.across;
            }
        }
    }
}

/// A menor e a maior fracção de travessa.
fn rail_bounds(fractions: &[f32]) -> (f32, f32) {
    let low = fractions.iter().copied().fold(0.1, f32::min);
    let high = fractions.iter().copied().fold(0.9, f32::max);
    (low.min(high), high)
}

/// Onde a recta `origin + t · direction` corta a caixa `[lo, hi]`.
fn clip_line(origin: Vec2, direction: Vec2, lo: Vec2, hi: Vec2) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
    for axis in 0..2 {
        if direction[axis].abs() < 1e-6 {
            if origin[axis] < lo[axis] || origin[axis] > hi[axis] {
                return None;
            }
            continue;
        }
        let ta = (lo[axis] - origin[axis]) / direction[axis];
        let tb = (hi[axis] - origin[axis]) / direction[axis];
        t0 = t0.max(ta.min(tb));
        t1 = t1.min(ta.max(tb));
    }
    (t1 > t0).then_some((t0.max(0.0), t1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::specs::{find, pasture_rail, picket, plank_board, presets, resolve, rope, wattle};

    fn flat(_: f32, _: f32) -> Option<f32> {
        Some(0.0)
    }

    fn spec(points: &str, closed: bool) -> FenceSpec {
        FenceSpec {
            points: points.into(),
            closed: closed.then_some(true),
            ..Default::default()
        }
    }

    fn build(points: &str, style: &FenceStyle, seed: u64) -> FenceLayout {
        let spec = spec(points, false);
        layout(&spec, &resolve(&spec, style), &FenceFrame::default(), &flat, seed).expect("layout")
    }

    /// Uma cerca com uma das pré-definidas do crate (pela chave).
    fn kind(key: &str, points: &str, seed: u64) -> FenceLayout {
        build(points, find(&[], key).expect("predefinição do crate"), seed)
    }

    #[test]
    fn os_voes_dividem_se_por_igual_sem_tocos() {
        let style = pasture_rail();
        for length in [2.9, 3.0, 4.4, 6.1, 9.7] {
            let fence = build(&format!("0 0 {length} 0"), &style, 1);
            let gaps: Vec<f32> = fence
                .spans
                .iter()
                .map(|span| (fence.posts[span.b].base - fence.posts[span.a].base).length())
                .collect();
            assert!(gaps.iter().all(|gap| *gap <= style.spacing + 1e-3), "{length} m: {gaps:?}");
            let smallest = gaps.iter().copied().fold(f32::MAX, f32::min);
            assert!(smallest > style.spacing * 0.49, "{length} m: {gaps:?}");
        }
    }

    #[test]
    fn o_perimetro_fechado_nao_repete_o_poste_da_juncao() {
        let style = pasture_rail();
        let square = spec("0 0 6 0 6 6 0 6", true);
        let fence = layout(&square, &style, &FenceFrame::default(), &flat, 1).unwrap();
        // Um quadrado de 6 m com postes a cada 3 m: 4 cantos e 1 poste por lado.
        let feet: Vec<Vec3> = fence.posts.iter().map(|post| post.base).collect();
        assert_eq!(feet.len(), 8, "{feet:?}");
        assert_eq!(fence.spans.len(), 8, "oito vãos: o último fecha o perímetro");
        let last = fence.spans.last().unwrap();
        assert_eq!((last.a, last.b), (7, 0), "o último vão fecha no primeiro poste");
        assert!(fence.posts.iter().filter(|post| post.strong).count() >= 4, "os cantos são fortes");
        for (index, post) in fence.posts.iter().enumerate() {
            for other in &fence.posts[index + 1..] {
                assert!(post.base.distance(other.base) > 0.1, "dois postes no mesmo sítio");
            }
        }
    }

    #[test]
    fn o_chao_dá_a_cota_de_cada_poste() {
        let style = pasture_rail();
        // Uma rampa de 6° e um degrau de 22° no fim.
        let slope = |_: f32, z: f32| Some(if z > 5.0 { 1.5 } else { z * 0.1 });
        let straight = spec("0 0 0 6", false);
        let fence = layout(&straight, &style, &FenceFrame::default(), &slope, 1).unwrap();
        let feet: Vec<f32> = fence.posts.iter().map(|post| post.base.y).collect();
        assert!((feet[0] - 0.0).abs() < 1e-4);
        assert!((feet[1] - 0.3).abs() < 1e-3, "{feet:?}");
        assert!((feet.last().unwrap() - 1.5).abs() < 1e-4, "{feet:?}");
        // O degrau (22°) põe as travessas em degraus: horizontais, à cota baixa.
        let rails: Vec<&Beam> = fence
            .beams
            .iter()
            .filter(|beam| beam.role == Role::Rail && beam.from.z > 2.5)
            .collect();
        assert_eq!(rails.len(), 2);
        for rail in rails {
            assert!((rail.from.y - rail.to.y).abs() < 0.05, "degraus: {:?}", rail);
        }
        // A rampa (6°) segue o terreno: a travessa sobe com ele.
        let gentle: Vec<&Beam> = fence
            .beams
            .iter()
            .filter(|beam| beam.role == Role::Rail && beam.from.z < 0.1)
            .collect();
        for rail in gentle {
            assert!((rail.to.y - rail.from.y - 0.3).abs() < 0.06, "rampa: {:?}", rail);
        }
        // Sem terreno (interiores) ou com fence-follow="points" fica a cota autorada.
        let none = |_: f32, _: f32| None;
        let indoor = layout(&spec("0 2 0; 0 2 6", false), &style, &FenceFrame::default(), &none, 1).unwrap();
        assert!((indoor.posts[0].base.y - 2.0).abs() < 1e-5);
        let authored = FenceSpec {
            follow: Some("points".into()),
            ..spec("0 2 0; 0 2 6", false)
        };
        let kept = layout(&authored, &style, &FenceFrame::default(), &slope, 1).unwrap();
        assert!(kept.posts.iter().all(|post| (post.base.y - 2.0).abs() < 1e-5));
    }

    #[test]
    fn a_moldura_da_entidade_leva_a_linha_ao_mundo() {
        let style = pasture_rail();
        let frame = FenceFrame {
            origin: Vec3::new(100.0, 40.0, -50.0),
            yaw: std::f32::consts::FRAC_PI_2,
            scale: 2.0,
        };
        // Chão que sobe com o x do mundo: meio metro por metro.
        let slope = |x: f32, _: f32| Some(40.0 + (x - 100.0) * 0.5);
        let line = spec("0 0 3 0", false);
        let turned = layout(&line, &style, &frame, &slope, 1).unwrap();
        // A rotação de 90° leva o x local para o z do mundo: o x do mundo não muda.
        assert!((turned.posts.last().unwrap().base.y - 0.0).abs() < 1e-4);
        let frame = FenceFrame {
            yaw: 0.0,
            ..frame
        };
        let along = layout(&line, &style, &frame, &slope, 1).unwrap();
        // 3 m locais são 6 m de mundo: sobem 3 m de mundo, 1,5 m aos olhos da cerca.
        assert!((along.posts.last().unwrap().base.y - 1.5).abs() < 1e-4);
    }

    #[test]
    fn o_layout_e_deterministico() {
        let style = picket();
        let build_with = |seed: u64| build("0 0 7 0.4", &style, seed);
        assert_eq!(build_with(42), build_with(42));
        assert_ne!(build_with(42), build_with(43));
        assert_ne!(build_with(42).beams[3].uv_offset, build_with(43).beams[3].uv_offset);
    }

    #[test]
    fn a_inclinacao_dos_postes_nunca_passa_o_estilo() {
        let style = pasture_rail();
        let limit = style.post_tilt_deg.to_radians().tan() + 1e-3;
        for seed in 0..40 {
            let fence = build("0 0 9 0", &style, seed);
            let posts: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Post).collect();
            assert_eq!(posts.len(), fence.posts.len(), "cada poste é uma peça");
            for post in posts {
                let height = post.to.y - post.from.y - style.bury;
                let lean = Vec2::new(post.to.x - post.from.x, post.to.z - post.from.z).length();
                assert!(lean <= height * limit, "seed {seed}: {lean} em {height} m");
                // A base fica onde o chão manda: enterrada `bury` abaixo.
                assert!((post.from.y - (post.ground_y - style.bury)).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn as_tabuas_variam_o_comprimento_dentro_do_estilo() {
        let style = plank_board();
        let fence = build("0 0 3 0", &style, 5);
        let boards: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Board).collect();
        assert_eq!(boards.len(), 18, "18 tábuas por vão, como a predefinição pede");
        let lengths: Vec<f32> = boards.iter().map(|board| board.to.y - board.from.y).collect();
        for length in &lengths {
            assert!(*length >= style.board_length.x && *length <= style.board_length.y, "{lengths:?}");
        }
        let min = lengths.iter().copied().fold(f32::MAX, f32::min);
        let max = lengths.iter().copied().fold(f32::MIN, f32::max);
        assert!(max - min > 0.05, "topos desiguais: {lengths:?}");
        // As tábuas pregam-se à frente das travessas (a cara +Z de quem vai de oeste para este).
        assert!(boards.iter().all(|board| board.from.z > 0.03));
        // A sobreposição `fence-boards` manda na contagem.
        let spec = FenceSpec {
            boards: Some(9),
            ..spec("0 0 3 0", false)
        };
        let fence = layout(&spec, &resolve(&spec, &style), &FenceFrame::default(), &flat, 5).unwrap();
        assert_eq!(fence.beams.iter().filter(|beam| beam.role == Role::Board).count(), 9);
        assert!(!fence.studs.is_empty(), "pregos nos cruzamentos com as travessas");
    }

    #[test]
    fn estacas_e_paliçada_acabam_em_ponta() {
        let fence = kind("picket", "0 0 3 0", 1);
        let boards: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Board).collect();
        assert!(boards.len() > 20, "{}", boards.len());
        assert!(boards.iter().all(|board| board.end_scale.x == 0.0), "ponta de estaca");
        assert!(boards.iter().all(|board| board.timber.width < 0.1), "estaca estreita");

        let fence = kind("palisade", "0 0 3 0", 1);
        let logs: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Board).collect();
        assert!(logs.len() >= 12, "{}", logs.len());
        assert!(logs.iter().all(|log| log.slot == Slot::Primary && log.timber.width > 0.1));
        assert!(logs.iter().all(|log| log.end_scale.x == 0.0), "tronco em ponta");
        // As travessas da paliçada ficam atrás dos troncos.
        let rails: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Rail).collect();
        assert_eq!(rails.len(), 4, "duas por vão, dois vãos");
        assert!(rails.iter().all(|rail| rail.from.z < 0.0));
    }

    #[test]
    fn as_varas_tecem_à_frente_e_atras_das_estacas() {
        let style = wattle();
        let fence = build("0 0 2 0", &style, 1);
        assert_eq!(fence.spans.len(), 1, "2 m é um vão só");
        assert!(fence.beams.iter().any(|beam| beam.role == Role::Stake));
        assert_eq!(fence.tubes.len(), style.rod_rows as usize, "uma vara por fila");
        let half = (style.stake.width + style.rod.width) * 0.5 * 0.92;
        for tube in &fence.tubes {
            assert!(tube.path.iter().all(|point| point.z.abs() <= half + 1e-4));
            assert!(tube.path.iter().any(|point| point.z > half * 0.5));
            assert!(tube.path.iter().any(|point| point.z < -half * 0.5), "passa para o outro lado");
            assert_eq!(tube.slot, Slot::Secondary);
            assert!(tube.radius.last() < tube.radius.first(), "afina até à ponta");
        }
        // As duas filas vizinhas estão em lados opostos à mesma estaca.
        let (first, second) = (&fence.tubes[0].path, &fence.tubes[1].path);
        assert!(first[0].z * second[0].z < 0.0);
        // As filas empilham-se de baixo para cima.
        assert!(second[0].y > first[0].y);
    }

    #[test]
    fn a_corda_tem_flecha_e_nos() {
        let style = rope();
        let fence = build("0 0 2 0", &style, 1);
        let usable = 2.0 - style.post.width.max(style.post.depth);
        let ropes: Vec<&Tube> = fence.tubes.iter().filter(|tube| tube.path.len() > 8).collect();
        assert_eq!(ropes.len(), 2, "duas cordas por vão");
        for tube in &ropes {
            let (start, end) = (tube.path[0], *tube.path.last().unwrap());
            let middle = tube.path[tube.path.len() / 2];
            let sag = (start.y + end.y) * 0.5 - middle.y;
            let expected = style.rope_sag * usable;
            assert!((sag - expected).abs() < 0.01, "flecha {sag} vs {expected}");
            // A corda pende e volta: nenhum ponto desce mais do que o meio.
            let lowest = tube.path.iter().map(|point| point.y).fold(f32::MAX, f32::min);
            assert!((lowest - middle.y).abs() < 0.01);
        }
        assert!(fence.tubes.iter().filter(|tube| tube.path.len() == 5).count() == 4, "um nó em cada ponta");
        assert!(fence.tubes.iter().all(|tube| tube.slot == Slot::Secondary));
    }

    #[test]
    fn a_treliça_cruza_ripas_dos_dois_lados() {
        let fence = kind("lattice", "0 0 3 0", 1);
        let slats: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Slat).collect();
        assert!(slats.len() >= 10, "{}", slats.len());
        assert!(slats.iter().any(|slat| slat.from.z > 0.0) && slats.iter().any(|slat| slat.from.z < 0.0));
        for slat in &slats {
            let run = slat.to.x - slat.from.x;
            let rise = slat.to.y - slat.from.y;
            let angle = (rise / run).atan().abs().to_degrees();
            assert!((angle - 45.0).abs() < 0.5, "{angle}");
            assert!(slat.from.x >= -1e-3 && slat.to.x <= 3.0 + 1e-3, "recortada ao vão");
        }
        // As duas famílias não se cruzam no mesmo plano: uma está à frente da outra.
        let (front, back): (Vec<&Beam>, Vec<&Beam>) = slats.iter().copied().partition(|slat| slat.from.z > 0.0);
        assert!(!front.is_empty() && !back.is_empty());
    }

    #[test]
    fn as_cruzetas_cruzam_o_vao() {
        let fence = kind("crossbuck", "0 0 3 0", 1);
        let braces: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Brace).collect();
        assert_eq!(braces.len(), 2);
        assert!(braces[0].from.y < braces[0].to.y && braces[1].from.y > braces[1].to.y);
        assert!(fence.beams.iter().filter(|beam| beam.role == Role::Rail).count() == 2);
    }

    #[test]
    fn o_numero_de_travessas_pedidas_manda() {
        // Três numa predefinição de duas alturas: três por vão, a do meio à meia altura
        // (encontrado ao vivo: `fence-rails=3` dava duas).
        let spec = FenceSpec { points: "0 0 6 0".into(), rails: Some(3), ..Default::default() };
        let fence = layout(&spec, &resolve(&spec, &pasture_rail()), &FenceFrame::default(), &flat, 7).unwrap();
        let rails: Vec<&Beam> = fence.beams.iter().filter(|beam| beam.role == Role::Rail).collect();
        assert_eq!(fence.spans.len(), 2);
        assert_eq!(rails.len(), 6);
        let mut heights: Vec<f32> = rails.iter().map(|rail| (rail.from.y + rail.to.y) * 0.5).collect();
        heights.sort_by(f32::total_cmp);
        for level in [0.38, 0.58, 0.78] {
            assert!(
                heights.iter().copied().filter(|y| (y - level * 1.2).abs() < 0.08).count() >= 2,
                "nível {level}: {heights:?}"
            );
        }
    }

    #[test]
    fn o_portao_rosse_a_folha_no_gonzo() {
        let style = find(&[], "gate").unwrap();
        let closed = build("0 0 2.6 0", style, 1);
        assert_eq!(closed.posts.len(), 2);
        assert!(closed.posts.iter().all(|post| post.strong));
        assert!(closed.beams.iter().any(|beam| beam.slot == Slot::Iron), "ferragens");
        assert!(closed.beams.iter().any(|beam| beam.role == Role::Brace), "travessa diagonal");
        // A folha fica dentro do vão, fechada.
        assert!(closed.beams.iter().filter(|beam| beam.role == Role::Board).all(|beam| beam.from.x > 0.0 && beam.from.x < 2.6));

        let mut open_style = style.clone();
        open_style.gate_open_deg = 60.0;
        let line = spec("0 0 2.6 0", false);
        let open = layout(&line, &open_style, &FenceFrame::default(), &flat, 1).unwrap();
        assert!(open.beams.iter().any(|beam| beam.to.z > 0.5), "a folha saiu da linha da cerca");
        // As dobadouras (junto ao gonzo) ficam onde estavam.
        let straps = |fence: &FenceLayout| {
            fence
                .beams
                .iter()
                .filter(|beam| beam.role == Role::Hardware && beam.from.x < 0.1)
                .map(|beam| (beam.from, beam.to))
                .collect::<Vec<_>>()
        };
        assert_eq!(straps(&closed).len(), 2, "duas dobadouras");
        assert_eq!(straps(&closed), straps(&open));
    }

    #[test]
    fn todas_as_predefinicoes_fazem_geometria() {
        for style in presets() {
            let fence = build("0 0 2.4 0 2.4 2.4", &style, 7);
            assert!(!fence.beams.is_empty() || !fence.tubes.is_empty(), "{}", style.key);
            assert!(fence.max_height().is_finite() && fence.max_height() > 0.5, "{}", style.key);
            for beam in &fence.beams {
                assert!(beam.from.is_finite() && beam.to.is_finite() && beam.across.is_finite(), "{}", style.key);
                assert!(beam.timber.width > 0.0 && beam.timber.depth > 0.0, "{}", style.key);
                assert!(beam.uv_offset.x >= 0.0 && beam.uv_offset.x < 1.0, "{}", style.key);
            }
        }
    }

    #[test]
    fn uma_linha_impossivel_diz_porque() {
        let style = pasture_rail();
        let try_layout = |points: &str, closed: bool| {
            layout(&spec(points, closed), &style, &FenceFrame::default(), &flat, 1).unwrap_err()
        };
        assert_eq!(try_layout("0 0 3", false), LayoutError::Points);
        assert_eq!(try_layout("0 0", false), LayoutError::TooShort, "um ponto só");
        assert_eq!(try_layout("0 0 3 0", true), LayoutError::TooShort, "fechada precisa de três");
        assert_eq!(
            try_layout("0 0 3", false).to_string(),
            "fence-points não é uma lista de pontos"
        );
    }
}
