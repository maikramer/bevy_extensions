//! Corpos de água (lago/rio): geometria de consulta, contorno orgânico e
//! amostra hidrodinâmica. Extraído do runtime do terreno: esta crate não
//! conhece heightfield, carve nem cena — só a lâmina e as suas propriedades.

use bevy::math::Vec2;

/// Amplitude MÍNIMA do alongamento — até o lago mais redondo do mundo tem
/// um oval subtil (o utilizador viu "círculos" nos lagos antigos).
const LAKE_STRETCH_MIN: f32 = 0.10;
/// Amplitude MÁXIMA do alongamento — o harmónico 2 dirigido
/// (`stretch·cos(2(θ−axis))`) que dá os lagos ovais/alongados.
pub const LAKE_STRETCH_MAX: f32 = 0.20;
/// Amplitudes MÍNIMAS dos harmónicos lineares (k = 1, 3, 5, 7): k=1
/// desloca a bacia (baía assimétrica), 3/5 dão lóbulos e baías, 7 dá o
/// recorte fino da linha de água. Os mínimos mantêm TODOS os lagos fora do
/// círculo perfeito — o sorteio por lago vive em `[min, max]`.
const LAKE_HARMONIC_MIN: [f32; 4] = [0.03, 0.05, 0.03, 0.02];
/// Amplitudes MÁXIMAS dos harmónicos lineares (k = 1, 3, 5, 7).
pub const LAKE_HARMONIC_MAX: [f32; 4] = [0.06, 0.10, 0.05, 0.04];
/// Peak of the organic contour relative to the design radius
/// (`1 + LAKE_STRETCH_MAX + Σ LAKE_HARMONIC_MAX = 1.45`). The carve AABB
/// must cover `radius · CONTOUR_PEAK · CARVE_MARGIN` — capping it at plain
/// `radius · CARVE_MARGIN` clipped the bowl of large lakes and left the
/// water mirror over dry land on the harmonic lobes.
pub const CONTOUR_PEAK: f32 = 1.0
    + LAKE_STRETCH_MAX
    + LAKE_HARMONIC_MAX[0]
    + LAKE_HARMONIC_MAX[1]
    + LAKE_HARMONIC_MAX[2]
    + LAKE_HARMONIC_MAX[3];

/// SplitMix64 do contrato de `bevy_ext_core` (mesma sequência, para as formas
/// dos lagos serem idênticas às do runtime do terreno).
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn distance_to_path(points: &[Vec2], p: Vec2) -> f32 {
    nearest_on_path(points, p)
        .map(|hit| hit.point.distance(p))
        .unwrap_or(f32::INFINITY)
}

/// Where a query point projects onto a polyline.
///
/// `segment` is the index of the segment's **start** station and `t` the
/// normalized position along it (`0` at `segment`, `1` at `segment + 1`).
///
/// Carvers keep their design as per-station arrays (river water surface, road
/// grade, half-widths). Reading those with `array[segment]` alone makes the
/// design *piecewise constant*: two neighbouring texels that project onto
/// different segments read values that differ by a whole station step, so the
/// carve writes a cliff at every segment boundary — the vertical fins that
/// used to line every river bank. [`station_lerp`] exists so callers blend
/// with `t` instead and the design stays continuous across the whole path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathHit {
    /// Closest point on the polyline.
    pub point: Vec2,
    /// Index of the segment's start station.
    pub segment: usize,
    /// Normalized position along that segment, `0..=1`.
    pub t: f32,
}

impl PathHit {
    /// Fractional station coordinate (`segment + t`), clamped to the last
    /// station so it indexes any per-station array of the same path.
    pub fn station(&self, stations: usize) -> f32 {
        let last = stations.saturating_sub(1) as f32;
        (self.segment as f32 + self.t).clamp(0.0, last.max(0.0))
    }
}

/// Samples a per-station array at the fractional position of `hit`, linearly
/// interpolating between the two neighbouring stations.
///
/// This is the continuous counterpart of `values[hit.segment]`; use it for
/// every quantity that varies along a path (design height, half-width,
/// falloff) so the carve never steps at a segment boundary.
pub fn station_lerp(values: &[f32], hit: &PathHit) -> f32 {
    match values.len() {
        0 => 0.0,
        1 => values[0],
        n => {
            let s = hit.station(n);
            let i = (s.floor() as usize).min(n - 1);
            let j = (i + 1).min(n - 1);
            let frac = s - i as f32;
            values[i] + (values[j] - values[i]) * frac
        }
    }
}

/// Closest point on the polyline, with the segment index and the normalized
/// position along it. `None` for paths with fewer than 2 points.
pub fn nearest_on_path(points: &[Vec2], p: Vec2) -> Option<PathHit> {
    if points.len() < 2 {
        return points.first().map(|q| PathHit {
            point: *q,
            segment: 0,
            t: 0.0,
        });
    }
    let mut best_d = f32::INFINITY;
    let mut best = PathHit {
        point: Vec2::ZERO,
        segment: 0,
        t: 0.0,
    };
    for (i, window) in points.windows(2).enumerate() {
        let (a, b) = (window[0], window[1]);
        let ab = b - a;
        let t = if ab.length_squared() <= 1e-12 {
            0.0
        } else {
            ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
        };
        let q = a + ab * t;
        let d = q.distance_squared(p);
        if d < best_d {
            best_d = d;
            best = PathHit {
                point: q,
                segment: i,
                t,
            };
        }
    }
    Some(best)
}

/// Autoria da forma de um lago (`<Lake seed stretch axis lobes>`) — os
/// quatro knobs opcionais sobre o contorno orgânico. Tudo `None` = o
/// comportamento histórico (uniformes sorteados pelo hash da posição), pelo
/// que mundos existentes ficam bit-exact.
///
/// * `seed` — troca a FONTE dos uniformes para o SplitMix64 local: mesma
///   seed, mesma forma, em qualquer posição.
/// * `stretch` — amplitude do alongamento (0 = redondo, clamp 0..0.45).
/// * `axis` — direção do alongamento, em GRAUS (o hash sorteava-a).
/// * `lobes` — multiplicador das amplitudes harmónicas (0 = sem baías,
///   1 = o natural sorteado, até 1.6; acima reescala o contorno para
///   continuar dentro do envelope [`CONTOUR_PEAK`], ver
///   [`LakeShape::fit_to_contour_envelope`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LakeAuthoring {
    pub seed: Option<u64>,
    pub stretch: Option<f32>,
    pub axis_deg: Option<f32>,
    pub lobes: Option<f32>,
}

/// Uma queda contígua do perfil de um rio — cascata ou cachoeira.
///
/// É o registro partilhado por todos os consumidores: o mesh (face
/// vertical dos segmentos `lip..base`, cortina alargada se cachoeira), a
/// névoa/spray (na base, escalados por [`Self::drop`]), o áudio
/// (posição/força) e a anotação de parede (rio × cliff, feita no bootstrap
/// quando a banda correspondente existe).
#[derive(Debug, Clone, Copy, PartialEq, bevy::prelude::Reflect)]
pub struct CascadeInfo {
    /// Estação LIP — onde a queda começa (borda de montante).
    pub lip: usize,
    /// Primeira estação DEPOIS da queda (fim da face; caldeirão aqui).
    pub base: usize,
    /// Queda acumulada da face (m) — `top_y − bot_y`.
    pub drop: f32,
    /// Tier: cachoeira (queda ≥ `waterfall_min_drop`) vs cascata simples.
    pub waterfall: bool,
    /// A queda despenca numa PAREDE voxel (rio × cliff): `bot_y` vem do
    /// pé da banda (não do perfil do rio) e o mesh desenha UM quadro do
    /// brow ao toe em vez de faces por segmento.
    pub wall: bool,
    /// Topo da face — a lâmina no lip (no brow, se parede).
    pub top_y: f32,
    /// Fundo da face — a lâmina na base (o pé da banda, se parede; submerso
    /// no caldeirão quando o pool desce abaixo do pé).
    pub bot_y: f32,
}

/// Kind of a registered [`WaterBody`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, bevy::prelude::Reflect)]
pub enum WaterKind {
    Lake,
    River,
}

/// Query registry entry produced by every water carve.
#[derive(Debug, Clone, PartialEq, bevy::prelude::Reflect)]
pub struct WaterBody {
    pub kind: WaterKind,
    /// Anchor point (lake center; river centroid).
    pub at: Vec2,
    /// Lake: design water radius. River: `0.0`.
    pub radius: f32,
    /// Avoid-zone radius (lake: carve radius; river: half carve width).
    pub carve_radius: f32,
    /// Raio O(1) de rejeição das queries de rio: distância máxima das
    /// estações ao `at` MAIS a maior meia-largura. O ponto mais próximo do
    /// caminho vive num segmento entre estações (casco convexo — fica tão
    /// perto do centro como as estações), pelo que `|p − at| > hull`
    /// garante ponto seco, e o +meia-largura estende a garantia à lâmina
    /// (e a `blend_surface_y` soma a margem fora). Lagos não o usam (as
    /// queries deles já são O(1)): `0.0`.
    pub hull: f32,
    /// Lake mirror height (rivers: mean surface height).
    pub water_y: f32,
    /// Lake: reach do ESPELHO sobre o contorno nominal — o MESMO fator
    /// [`lake_mirror_reach`] que o mesh do espelho aplica ao anel exterior;
    /// [`WaterBody::distance_to_waterline`] mede até
    /// `contorno × mirror_reach` (a linha de água real, não o contorno
    /// nominal — a banda de areia/lama do splat fica ancorada ao bordo da
    /// água). Rio / corpo à mão: `1.0` (contrato antigo).
    pub mirror_reach: f32,
    /// Lake: a forma orgânica do contorno (cache do `LakeShape::new(at)` —
    /// as queries não voltam a fazer hash da posição). Rio / corpo à mão:
    /// [`LakeShape::default`] (círculo, nunca consultado).
    pub shape: LakeShape,
    /// River stations in world XZ (empty for lakes).
    pub stations: Vec<Vec2>,
    /// River surface height per station (empty for lakes).
    pub surface_y: Vec<f32>,
    /// River full water width (lakes: `0.0`).
    pub water_width: f32,
    /// Meia-largura EFETIVA da água POR estação (poços largos, rápidos
    /// estreitos, já escalada pelo reach da linha de água no carve — a
    /// ribbon e as queries de água partilham esta métrica; ver
    /// [`river_water_mesh`]). Vazio = largura constante
    /// (`water_width * 0.5` em toda a linha) — é o contrato que os
    /// consumidores antigos assumem.
    pub half_width: Vec<f32>,
    /// Rio: profundidade EFETIVA do canal POR estação — poços fundos,
    /// rápidos rasos, caldeirão de cascata/nascente; é este vetor que o
    /// carve usa no perfil do canal e que alimenta o reach da linha de
    /// água guardado em `half_width`. Vazio = profundidade constante
    /// (contrato antigo: o consumidor usa a nominal da spec — ver
    /// [`WaterBody::depth_at`]).
    pub depths: Vec<f32>,
    /// Quedas do perfil (queda `surface[i] − surface[i+1] > CASCADE_DROP`
    /// contígua = UMA entrada). Vazio = rio sem quedas. Ver
    /// [`CascadeInfo`] para o contrato por consumidor.
    pub cascades: Vec<CascadeInfo>,
}

impl WaterBody {
    /// Meia-largura da água numa estação (com fallback do contrato antigo).
    pub fn half_width_at(&self, i: usize) -> f32 {
        self.half_width
            .get(i)
            .copied()
            .unwrap_or(self.water_width * 0.5)
    }
    /// Profundidade EFETIVA do canal numa estação. `fallback` é a
    /// profundidade NOMINAL da spec — o contrato antigo dos corpos sem
    /// `depths` (construídos à mão ou por versões anteriores do carve).
    pub fn depth_at(&self, i: usize, fallback: f32) -> f32 {
        self.depths.get(i).copied().unwrap_or(fallback)
    }
    /// Meia-largura EFETIVA interpolada no ponto do caminho (`hit`) — a
    /// linha de água real (as larguras guardadas já incluem o reach do
    /// carve). Fallback do contrato antigo quando o vetor está vazio.
    /// Meia-largura efetiva interpolada no ponto do caminho (usada pelo
    /// carve e pelos testes do host).
    pub fn half_width_lerp(&self, hit: &PathHit) -> f32 {
        if self.half_width.is_empty() {
            self.water_width * 0.5
        } else {
            station_lerp(&self.half_width, hit)
        }
    }
    /// Contorno orgânico do lago na direção `delta` (a partir do centro) —
    /// a métrica radial partilhada pelas queries de espelho.
    fn contour_at_delta(&self, delta: Vec2) -> f32 {
        self.shape.contour(self.radius, delta.y.atan2(delta.x))
    }

    #[inline]
    fn lake_radial_bounds(&self, margin: f32) -> (f32, f32) {
        let (min_frac, max_frac) = self.shape.radius_frac_bounds();
        let scale = self.radius * self.mirror_reach;
        ((scale * min_frac + margin).max(0.0), (scale * max_frac + margin).max(0.0))
    }

    /// Point is inside the carve zone (spawner `avoid-water`).
    pub fn contains(&self, p: Vec2) -> bool {
        match self.kind {
            WaterKind::Lake => (p - self.at).length_squared() <= self.carve_radius * self.carve_radius,
            WaterKind::River => {
                !self.stations.is_empty()
                    // Rejeição O(1) pelo casco antes do scan O(estações): um
                    // ponto a 200 m de um rio de 2 km não paga 2 000 segmentos
                    // — e quem paga são a relva (por célula de lattice) e os
                    // scripts (por frame). `hull <= 0` (corpo construído à
                    // mão que não computou o casco) desliga o rejeite em vez
                    // de negar tudo.
                    && (self.hull <= 0.0
                        || (p - self.at).length_squared()
                            <= (self.hull + self.carve_radius).powi(2))
                    && distance_to_path(&self.stations, p) <= self.carve_radius
            }
        }
    }

    /// Point is inside the carve zone plus `margin` (spawner `near-water`).
    pub fn is_near(&self, p: Vec2, margin: f32) -> bool {
        match self.kind {
            WaterKind::Lake => {
                let r = (self.carve_radius + margin).max(0.0);
                (p - self.at).length_squared() <= r * r
            }
            WaterKind::River => {
                !self.stations.is_empty()
                    && (self.hull <= 0.0
                        || (p - self.at).length_squared()
                            <= (self.hull + self.carve_radius + margin).powi(2))
                    && distance_to_path(&self.stations, p) <= self.carve_radius + margin
            }
        }
    }

    /// Water surface height at `p` when the point is over the water ribbon
    /// (rivers: nearest station surface; lakes: flat mirror when inside the
    /// design radius). `None` when the point is on dry land.
    pub fn surface_y_at(&self, p: Vec2) -> Option<f32> {
        match self.kind {
            WaterKind::Lake => {
                // Linha de água REAL (contorno × reach): um lóbulo de água
                // no contorno orgânico tem de responder "água" mesmo fora
                // do raio nominal — e um recorte tem de responder "seco"
                // mesmo dentro dele (assentamento `in-water` à lâmina).
                let delta = p - self.at;
                let dist_sq = delta.length_squared();
                let (min_r, max_r) = self.lake_radial_bounds(0.0);
                if dist_sq > max_r * max_r {
                    return None;
                }
                if dist_sq <= min_r * min_r {
                    return Some(self.water_y);
                }
                let contour = self.contour_at_delta(delta) * self.mirror_reach;
                (dist_sq.sqrt() <= contour).then_some(self.water_y)
            }
            WaterKind::River => {
                // Mesma rejeição O(1) do casco (`contains`): além do hull não
                // há lâmina — o teste fino só corre perto do rio. `hull <= 0`
                // (corpo à mão) desliga o rejeite.
                if self.hull > 0.0 && (p - self.at).length_squared() > self.hull.powi(2) {
                    return None;
                }
                let hit = nearest_on_path(&self.stations, p)?;
                let d_sq = hit.point.distance_squared(p);
                let hw = self.half_width_lerp(&hit);
                // Meia-largura EFETIVA da estação — a ribbon acaba em
                // `half · reach` (guardado no carve); usar a nominal punha a
                // lâmina/queries sobre a margem seca.
                (d_sq <= hw * hw).then(|| {
                    if self.surface_y.is_empty() {
                        self.water_y
                    } else {
                        station_lerp(&self.surface_y, &hit)
                    }
                })
            }
        }
    }

    /// Horizontal distance from `p` to the waterline (meters; **negative**
    /// inside the water mirror). This is the geometry line — the shoreline a
    /// renderer or spawner cares about — not the carve radius: over a lake
    /// slope the carve reaches past the mirror, and painting sand to the carve
    /// radius would put beaches on dry hillsides. No lago a linha é o
    /// contorno ×[`WaterBody::mirror_reach`] — o bordo REAL do espelho (a
    /// mesma métrica do mesh): medir ao contorno nominal punha a praia de
    /// areia/lama do splat para lá (fundo) ou para dentro (raso) da água.
    pub fn distance_to_waterline(&self, p: Vec2) -> f32 {
        match self.kind {
            WaterKind::Lake => {
                let delta = p - self.at;
                let theta = delta.y.atan2(delta.x);
                let contour = self.shape.contour(self.radius, theta) * self.mirror_reach;
                delta.length() - contour
            }
            WaterKind::River => match nearest_on_path(&self.stations, p) {
                Some(hit) => hit.point.distance(p) - self.half_width_lerp(&hit),
                None => f32::INFINITY,
            },
        }
    }

    /// Water surface height above `p` for ground-color work (the splat map):
    /// like [`WaterBody::surface_y_at`] but with a **margin** so a texel just
    /// outside the mirror still resolves a surface while it lies inside the
    /// waterline-plus-margin band. `None` far from any water.
    pub fn blend_surface_y(&self, p: Vec2, margin: f32) -> Option<f32> {
        match self.kind {
            WaterKind::Lake => {
                // Mesma métrica de [`WaterBody::surface_y_at`] + margem.
                let delta = p - self.at;
                let dist_sq = delta.length_squared();
                let (min_r, max_r) = self.lake_radial_bounds(margin);
                if dist_sq > max_r * max_r {
                    return None;
                }
                if dist_sq <= min_r * min_r {
                    return Some(self.water_y);
                }
                let contour = self.contour_at_delta(delta) * self.mirror_reach;
                (dist_sq.sqrt() <= contour + margin).then_some(self.water_y)
            }
            WaterKind::River => {
                // Mesma rejeição O(1) do casco, +margem (`blend_surface_y`);
                // `hull <= 0` (corpo à mão) desliga o rejeite.
                if self.hull > 0.0 && (p - self.at).length_squared() > (self.hull + margin).powi(2)
                {
                    return None;
                }
                let hit = nearest_on_path(&self.stations, p)?;
                let d_sq = hit.point.distance_squared(p);
                let hw_m = self.half_width_lerp(&hit) + margin;
                // Meia-largura EFETIVA + margem (mesma métrica de
                // [`WaterBody::surface_y_at`]).
                (d_sq <= hw_m * hw_m).then(|| {
                    if self.surface_y.is_empty() {
                        self.water_y
                    } else {
                        station_lerp(&self.surface_y, &hit)
                    }
                })
            }
        }
    }

    /// Spatial hydrodynamic sample at world XZ `p` (`None` outside the water mirror
    /// or over dry emerges inside `bed_y`).
    pub fn sample_domain(
        &self,
        p: Vec2,
        bed_y: Option<f32>,
        default_depth: f32,
    ) -> Option<WaterDomainSample> {
        self.sample_domain_with_bed(p, || bed_y, default_depth)
    }

    /// Variant of [`Self::sample_domain`] that evaluates `bed_y_fn` lazily — only
    /// when `p` is actually inside the body contour/ribbon — avoiding expensive
    /// heightfield sampling on dry texels.
    pub fn sample_domain_with_bed<F>(
        &self,
        p: Vec2,
        bed_y_fn: F,
        default_depth: f32,
    ) -> Option<WaterDomainSample>
    where
        F: FnOnce() -> Option<f32>,
    {
        match self.kind {
            WaterKind::Lake => {
                let delta = p - self.at;
                let dist_sq = delta.length_squared();
                let (min_r, max_r) = self.lake_radial_bounds(0.0);
                if dist_sq > max_r * max_r {
                    return None;
                }
                let mut contour_cache: Option<f32> = None;
                let radial_dist = if dist_sq <= min_r * min_r {
                    None
                } else {
                    let radial_dist = dist_sq.sqrt();
                    let contour = self.contour_at_delta(delta) * self.mirror_reach;
                    if radial_dist > contour {
                        return None;
                    }
                    contour_cache = Some(contour);
                    Some(radial_dist)
                };
                let bed_y = bed_y_fn();
                if let Some(by) = bed_y {
                    if by >= self.water_y - 0.01 {
                        return None;
                    }
                    return Some(WaterDomainSample {
                        surface_y: self.water_y,
                        depth: (self.water_y - by).clamp(0.02, 40.0),
                        flow: Vec2::ZERO,
                        plunge: 0.0,
                    });
                }
                let radial_dist = radial_dist.unwrap_or_else(|| dist_sq.sqrt());
                let contour = contour_cache
                    .unwrap_or_else(|| self.contour_at_delta(delta) * self.mirror_reach);
                let rim_t = (1.0 - (radial_dist / contour.max(0.1)).powi(2)).clamp(0.05, 1.0);
                Some(WaterDomainSample {
                    surface_y: self.water_y,
                    depth: (default_depth.max(0.4) * rim_t).max(0.05),
                    flow: Vec2::ZERO,
                    plunge: 0.0,
                })
            }
            WaterKind::River => {
                if self.hull > 0.0 && (p - self.at).length_squared() > self.hull.powi(2) {
                    return None;
                }
                let hit = nearest_on_path(&self.stations, p)?;
                self.sample_river_hit(p, hit, bed_y_fn, default_depth, None)
            }
        }
    }

    fn sample_river_hit<F>(
        &self,
        p: Vec2,
        hit: PathHit,
        bed_y_fn: F,
        default_depth: f32,
        cascade_bounds: Option<(Vec2, Vec2)>,
    ) -> Option<WaterDomainSample>
    where
        F: FnOnce() -> Option<f32>,
    {
        let hw = self.half_width_lerp(&hit);
        let dist_sq = hit.point.distance_squared(p);
        if dist_sq > hw * hw {
            return None;
        }
        let surf_y = if self.surface_y.is_empty() {
            self.water_y
        } else {
            station_lerp(&self.surface_y, &hit)
        };
        let bed_y = bed_y_fn();
        if let Some(by) = bed_y {
            if by >= surf_y + 0.08 {
                return None;
            }
        }
        let dist = dist_sq.sqrt();
        let channel_depth = if self.depths.is_empty() {
            default_depth
        } else {
            station_lerp(&self.depths, &hit)
        };
        let bank_u = (dist / hw.max(0.1)).clamp(0.0, 1.0);
        let analytic_depth = (channel_depth * (1.0 - 0.65 * bank_u * bank_u)).max(0.05);
        let depth = bed_y
            .map(|by| (surf_y - by).max(analytic_depth * 0.35).clamp(0.02, 40.0))
            .unwrap_or(analytic_depth);

        let n = self.stations.len();
        let seg_a = hit.segment.min(n.saturating_sub(2));
        let seg_b = (seg_a + 1).min(n.saturating_sub(1));
        let tangent = self.stations[seg_b] - self.stations[seg_a];
        let seg_len = tangent.length().max(0.05);
        let dir = tangent / seg_len;
        let slope = if self.surface_y.len() > seg_b {
            ((self.surface_y[seg_a] - self.surface_y[seg_b]) / seg_len).max(0.0)
        } else {
            0.0
        };
        let speed = (1.15_f32 + slope * 14.0).clamp(0.9, 3.8);
        let profile = 0.45 + 0.55 * (1.0 - bank_u * bank_u);
        let flow = dir * (speed * profile);

        let mut plunge = 0.0_f32;
        if !self.cascades.is_empty() {
            let check_cascades = cascade_bounds.is_none_or(|(cmin, cmax)| {
                p.x >= cmin.x && p.x <= cmax.x && p.y >= cmin.y && p.y <= cmax.y
            });
            if check_cascades {
                for c in &self.cascades {
                    if let Some(&base_pos) = self.stations.get(c.base) {
                        let pr = (self.half_width_at(c.base) * 1.45).max(2.5);
                        let pd_sq = p.distance_squared(base_pos);
                        if pd_sq < pr * pr {
                            let pd = pd_sq.sqrt();
                            let fall_scale = (c.drop / 4.0).clamp(0.35, 1.0);
                            plunge = plunge.max((1.0 - pd / pr) * fall_scale);
                        }
                    }
                }
            }
        }

        Some(WaterDomainSample {
            surface_y: surf_y,
            depth,
            flow,
            plunge,
        })
    }
}

const PATH_BLOCK_SEGS: usize = 8;
const SUPER_BLOCK_CHILDREN: usize = 8;

#[derive(Debug, Clone)]
struct PathBlock {
    start: usize,
    end: usize,
    min: Vec2,
    max: Vec2,
}

#[derive(Debug, Clone)]
struct SuperPathBlock {
    block_start: usize,
    block_end: usize,
    min: Vec2,
    max: Vec2,
}

#[inline]
fn dist_sq_to_aabb(p: Vec2, min: Vec2, max: Vec2) -> f32 {
    let clamped = p.clamp(min, max);
    (p - clamped).length_squared()
}

/// Hierarchical AABB index over a polyline's segments for fast bounded and exact
/// `nearest_on_path` queries without modifying [`WaterBody`] fields or reflection.
#[derive(Debug, Clone)]
pub struct PathSpatialIndex {
    min: Vec2,
    max: Vec2,
    blocks: Vec<PathBlock>,
    super_blocks: Vec<SuperPathBlock>,
}

impl PathSpatialIndex {
    pub fn build(points: &[Vec2]) -> Option<Self> {
        if points.len() < 2 {
            return None;
        }
        let seg_count = points.len() - 1;
        let num_blocks = seg_count.div_ceil(PATH_BLOCK_SEGS);
        let mut blocks = Vec::with_capacity(num_blocks);
        let mut g_min = Vec2::splat(f32::INFINITY);
        let mut g_max = Vec2::splat(f32::NEG_INFINITY);

        for start in (0..seg_count).step_by(PATH_BLOCK_SEGS) {
            let end = (start + PATH_BLOCK_SEGS).min(seg_count);
            let mut b_min = points[start];
            let mut b_max = points[start];
            for &pt in &points[start + 1..=end] {
                b_min = b_min.min(pt);
                b_max = b_max.max(pt);
            }
            g_min = g_min.min(b_min);
            g_max = g_max.max(b_max);
            blocks.push(PathBlock {
                start,
                end,
                min: b_min,
                max: b_max,
            });
        }

        let mut super_blocks = Vec::new();
        if blocks.len() > SUPER_BLOCK_CHILDREN {
            for block_start in (0..blocks.len()).step_by(SUPER_BLOCK_CHILDREN) {
                let block_end = (block_start + SUPER_BLOCK_CHILDREN).min(blocks.len());
                let mut s_min = Vec2::splat(f32::INFINITY);
                let mut s_max = Vec2::splat(f32::NEG_INFINITY);
                for b in &blocks[block_start..block_end] {
                    s_min = s_min.min(b.min);
                    s_max = s_max.max(b.max);
                }
                super_blocks.push(SuperPathBlock {
                    block_start,
                    block_end,
                    min: s_min,
                    max: s_max,
                });
            }
        }

        Some(Self {
            min: g_min,
            max: g_max,
            blocks,
            super_blocks,
        })
    }

    pub fn bounds(&self) -> (Vec2, Vec2) {
        (self.min, self.max)
    }

    /// Exact closest point on `points`, equivalent to [`nearest_on_path`] when the
    /// result lies within `max_dist` (and returns `None` if farther than `max_dist`).
    pub fn nearest_within(&self, points: &[Vec2], p: Vec2, max_dist: f32) -> Option<PathHit> {
        let mut best_d = max_dist * max_dist;
        if dist_sq_to_aabb(p, self.min, self.max) > best_d {
            return None;
        }
        let mut best: Option<PathHit> = None;

        if !self.super_blocks.is_empty() {
            for sb in &self.super_blocks {
                if dist_sq_to_aabb(p, sb.min, sb.max) > best_d {
                    continue;
                }
                for b in &self.blocks[sb.block_start..sb.block_end] {
                    if dist_sq_to_aabb(p, b.min, b.max) > best_d {
                        continue;
                    }
                    scan_segment_range(points, b.start, b.end, p, &mut best_d, &mut best);
                }
            }
        } else {
            for b in &self.blocks {
                if dist_sq_to_aabb(p, b.min, b.max) > best_d {
                    continue;
                }
                scan_segment_range(points, b.start, b.end, p, &mut best_d, &mut best);
            }
        }

        best
    }

    /// Exact closest point anywhere on `points`, bit-equivalent to [`nearest_on_path`].
    pub fn nearest(&self, points: &[Vec2], p: Vec2) -> Option<PathHit> {
        if points.len() < 2 {
            return nearest_on_path(points, p);
        }
        // Pick an initial upper bound from the block whose AABB is closest to `p`
        // so branch-and-bound prunes all distant blocks immediately.
        let mut best_d = f32::INFINITY;
        let mut best: Option<PathHit> = None;
        let seed_block = self
            .blocks
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                dist_sq_to_aabb(p, a.min, a.max)
                    .partial_cmp(&dist_sq_to_aabb(p, b.min, b.max))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        let sb0 = &self.blocks[seed_block];
        scan_segment_range(points, sb0.start, sb0.end, p, &mut best_d, &mut best);

        if !self.super_blocks.is_empty() {
            for sb in &self.super_blocks {
                if dist_sq_to_aabb(p, sb.min, sb.max) > best_d {
                    continue;
                }
                for (bi, b) in self.blocks[sb.block_start..sb.block_end].iter().enumerate() {
                    if sb.block_start + bi == seed_block {
                        continue;
                    }
                    if dist_sq_to_aabb(p, b.min, b.max) > best_d {
                        continue;
                    }
                    scan_segment_range(points, b.start, b.end, p, &mut best_d, &mut best);
                }
            }
        } else {
            for (bi, b) in self.blocks.iter().enumerate() {
                if bi == seed_block || dist_sq_to_aabb(p, b.min, b.max) > best_d {
                    continue;
                }
                scan_segment_range(points, b.start, b.end, p, &mut best_d, &mut best);
            }
        }

        best
    }
}

#[inline]
fn scan_segment_range(
    points: &[Vec2],
    start: usize,
    end: usize,
    p: Vec2,
    best_d: &mut f32,
    best: &mut Option<PathHit>,
) {
    for i in start..end {
        let a = points[i];
        let b = points[i + 1];
        let seg_min = a.min(b);
        let seg_max = a.max(b);
        if dist_sq_to_aabb(p, seg_min, seg_max) > *best_d {
            continue;
        }
        let ab = b - a;
        let len_sq = ab.length_squared();
        let t = if len_sq <= 1e-12 {
            0.0
        } else {
            ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
        };
        let q = a + ab * t;
        let d = q.distance_squared(p);
        // Preserve strict `<` tie-break order only when equal distance occurs on
        // the block scan vs seed scan; matches linear `nearest_on_path` on ties by
        // only tightening on `<`, except we treat exact ties cleanly.
        if d < *best_d || (d == *best_d && best.is_none_or(|prev| i < prev.segment)) {
            *best_d = d;
            *best = Some(PathHit {
                point: q,
                segment: i,
                t,
            });
        }
    }
}

/// Precomputed spatial accelerator for a [`WaterBody`] (used by domain rasterization,
/// contact detection, buoyancy, and ground splatting).
#[derive(Debug, Clone)]
pub struct WaterBodyIndex {
    bounds_min: Vec2,
    bounds_max: Vec2,
    max_hw: f32,
    path: Option<PathSpatialIndex>,
    cascade_bounds: Option<(Vec2, Vec2)>,
}

impl WaterBodyIndex {
    pub fn for_body(body: &WaterBody) -> Self {
        match body.kind {
            WaterKind::Lake => {
                let (_, max_frac) = body.shape.radius_frac_bounds();
                let reach = body
                    .carve_radius
                    .max(body.radius * max_frac * body.mirror_reach);
                Self {
                    bounds_min: body.at - Vec2::splat(reach),
                    bounds_max: body.at + Vec2::splat(reach),
                    max_hw: reach,
                    path: None,
                    cascade_bounds: None,
                }
            }
            WaterKind::River => {
                let max_hw = body
                    .half_width
                    .iter()
                    .copied()
                    .fold(body.water_width * 0.5, f32::max);
                let expand = max_hw.max(body.carve_radius);
                let path = PathSpatialIndex::build(&body.stations);
                let (pmin, pmax) = path
                    .as_ref()
                    .map(|idx| idx.bounds())
                    .unwrap_or_else(|| {
                        body.stations
                            .first()
                            .copied()
                            .map(|p| (p, p))
                            .unwrap_or((body.at, body.at))
                    });
                let cascade_bounds = if body.cascades.is_empty() {
                    None
                } else {
                    let mut cmin = Vec2::splat(f32::INFINITY);
                    let mut cmax = Vec2::splat(f32::NEG_INFINITY);
                    let mut any = false;
                    for c in &body.cascades {
                        if let Some(&base_pos) = body.stations.get(c.base) {
                            let pr = (body.half_width_at(c.base) * 1.45).max(2.5);
                            cmin = cmin.min(base_pos - Vec2::splat(pr));
                            cmax = cmax.max(base_pos + Vec2::splat(pr));
                            any = true;
                        }
                    }
                    any.then_some((cmin, cmax))
                };
                Self {
                    bounds_min: pmin - Vec2::splat(expand),
                    bounds_max: pmax + Vec2::splat(expand),
                    max_hw,
                    path,
                    cascade_bounds,
                }
            }
        }
    }

    pub fn build_vec(bodies: &[WaterBody]) -> Vec<Self> {
        bodies.iter().map(Self::for_body).collect()
    }

    #[inline]
    pub fn bounds(&self) -> (Vec2, Vec2) {
        (self.bounds_min, self.bounds_max)
    }

    #[inline]
    pub fn overlaps_rect(&self, min: Vec2, max: Vec2, pad: f32) -> bool {
        self.bounds_max.x + pad >= min.x
            && self.bounds_min.x - pad <= max.x
            && self.bounds_max.y + pad >= min.y
            && self.bounds_min.y - pad <= max.y
    }

    /// Appends river segment indices that can possibly satisfy `d <= hw` for any point
    /// in `[cell_min, cell_max]` (preserving exact `nearest_on_path` semantics).
    pub fn collect_river_segments_in_rect(
        &self,
        body: &WaterBody,
        cell_min: Vec2,
        cell_max: Vec2,
        out_segs: &mut Vec<usize>,
    ) {
        if body.kind != WaterKind::River || body.stations.len() < 2 {
            return;
        }
        let expand = self.max_hw + 1e-3;
        let qmin = cell_min - Vec2::splat(expand);
        let qmax = cell_max + Vec2::splat(expand);
        if self.bounds_max.x < qmin.x
            || self.bounds_min.x > qmax.x
            || self.bounds_max.y < qmin.y
            || self.bounds_min.y > qmax.y
        {
            return;
        }
        let Some(path) = &self.path else {
            out_segs.extend(0..body.stations.len() - 1);
            return;
        };
        let push_block = |b: &PathBlock, out: &mut Vec<usize>| {
            if b.max.x < qmin.x || b.min.x > qmax.x || b.max.y < qmin.y || b.min.y > qmax.y {
                return;
            }
            for i in b.start..b.end {
                let s0 = body.stations[i];
                let s1 = body.stations[i + 1];
                let smin = s0.min(s1);
                let smax = s0.max(s1);
                if smax.x >= qmin.x && smin.x <= qmax.x && smax.y >= qmin.y && smin.y <= qmax.y {
                    out.push(i);
                }
            }
        };
        if !path.super_blocks.is_empty() {
            for sb in &path.super_blocks {
                if sb.max.x < qmin.x || sb.min.x > qmax.x || sb.max.y < qmin.y || sb.min.y > qmax.y {
                    continue;
                }
                for b in &path.blocks[sb.block_start..sb.block_end] {
                    push_block(b, out_segs);
                }
            }
        } else {
            for b in &path.blocks {
                push_block(b, out_segs);
            }
        }
    }

    pub fn sample_domain_river_segs<F>(
        &self,
        body: &WaterBody,
        p: Vec2,
        segs: &[usize],
        bed_y_fn: F,
        default_depth: f32,
    ) -> Option<WaterDomainSample>
    where
        F: FnOnce() -> Option<f32>,
    {
        if segs.is_empty() {
            return None;
        }
        let mut best_d = (self.max_hw + 1e-4).powi(2);
        let mut best: Option<PathHit> = None;
        for &i in segs {
            let a = body.stations[i];
            let b = body.stations[i + 1];
            let ab = b - a;
            let len_sq = ab.length_squared();
            let t = if len_sq <= 1e-12 {
                0.0
            } else {
                ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
            };
            let q = a + ab * t;
            let d = q.distance_squared(p);
            if d < best_d || (d == best_d && best.is_none_or(|prev| i < prev.segment)) {
                best_d = d;
                best = Some(PathHit {
                    point: q,
                    segment: i,
                    t,
                });
            }
        }
        let hit = best?;
        body.sample_river_hit(p, hit, bed_y_fn, default_depth, self.cascade_bounds)
    }

    pub fn surface_y_at(&self, body: &WaterBody, p: Vec2) -> Option<f32> {
        if p.x < self.bounds_min.x
            || p.x > self.bounds_max.x
            || p.y < self.bounds_min.y
            || p.y > self.bounds_max.y
        {
            return None;
        }
        match body.kind {
            WaterKind::Lake => body.surface_y_at(p),
            WaterKind::River => {
                let hit = match &self.path {
                    Some(idx) => idx.nearest_within(&body.stations, p, self.max_hw + 1e-4)?,
                    None => nearest_on_path(&body.stations, p)?,
                };
                let d_sq = hit.point.distance_squared(p);
                let hw = body.half_width_lerp(&hit);
                (d_sq <= hw * hw).then(|| {
                    if body.surface_y.is_empty() {
                        body.water_y
                    } else {
                        station_lerp(&body.surface_y, &hit)
                    }
                })
            }
        }
    }

    pub fn blend_surface_y(&self, body: &WaterBody, p: Vec2, margin: f32) -> Option<f32> {
        if p.x < self.bounds_min.x - margin
            || p.x > self.bounds_max.x + margin
            || p.y < self.bounds_min.y - margin
            || p.y > self.bounds_max.y + margin
        {
            return None;
        }
        match body.kind {
            WaterKind::Lake => body.blend_surface_y(p, margin),
            WaterKind::River => {
                let hit = match &self.path {
                    Some(idx) => {
                        idx.nearest_within(&body.stations, p, self.max_hw + margin + 1e-4)?
                    }
                    None => nearest_on_path(&body.stations, p)?,
                };
                let d_sq = hit.point.distance_squared(p);
                let hw_m = body.half_width_lerp(&hit) + margin;
                (d_sq <= hw_m * hw_m).then(|| {
                    if body.surface_y.is_empty() {
                        body.water_y
                    } else {
                        station_lerp(&body.surface_y, &hit)
                    }
                })
            }
        }
    }

    pub fn distance_to_waterline(&self, body: &WaterBody, p: Vec2) -> f32 {
        match body.kind {
            WaterKind::Lake => body.distance_to_waterline(p),
            WaterKind::River => match &self.path {
                Some(idx) => match idx.nearest(&body.stations, p) {
                    Some(hit) => hit.point.distance(p) - body.half_width_lerp(&hit),
                    None => f32::INFINITY,
                },
                None => body.distance_to_waterline(p),
            },
        }
    }

    pub fn sample_domain_with_bed<F>(
        &self,
        body: &WaterBody,
        p: Vec2,
        bed_y_fn: F,
        default_depth: f32,
    ) -> Option<WaterDomainSample>
    where
        F: FnOnce() -> Option<f32>,
    {
        if p.x < self.bounds_min.x
            || p.x > self.bounds_max.x
            || p.y < self.bounds_min.y
            || p.y > self.bounds_max.y
        {
            return None;
        }
        match body.kind {
            WaterKind::Lake => body.sample_domain_with_bed(p, bed_y_fn, default_depth),
            WaterKind::River => {
                let hit = match &self.path {
                    Some(idx) => idx.nearest_within(&body.stations, p, self.max_hw + 1e-4)?,
                    None => nearest_on_path(&body.stations, p)?,
                };
                body.sample_river_hit(p, hit, bed_y_fn, default_depth, self.cascade_bounds)
            }
        }
    }
}

/// Hydrodynamic water domain sample returned by [`WaterBody::sample_domain`].
#[derive(Debug, Clone, Copy, Default)]
pub struct WaterDomainSample {
    pub surface_y: f32,

    pub depth: f32,
    pub flow: Vec2,
    pub plunge: f32,
}

/// Personalidade orgânica de um lago: alongamento dirigido + harmónicos
/// lineares (k = 1, 3, 5, 7) com amplitude e fase sorteadas por hash da
/// posição — a mesma posição devolve sempre a MESMA forma ("mesma seed,
/// mesmo mundo"), mas lagos diferentes ficam diferentes: uns quase redondos,
/// outros ovais com baías e lóbulos. O contorno é RADIAL (star-shaped) de
/// propósito: rim, mesh do espelho, pedras de margem, parede gorge, espuma e
/// o audit amostram todos `r(θ)` — um contorno não-estrela partia-os.
#[derive(Debug, Clone, Copy, PartialEq, bevy::prelude::Reflect)]
pub struct LakeShape {
    /// Direção do alongamento (radianos). Público para auditoria/testes do
    /// host (o carve compara formas e eixos).
    pub axis: f32,
    /// Amplitude do alongamento — `stretch·cos(2(θ − axis))`.
    pub stretch: f32,
    /// Harmónicos lineares: `(k, amplitude, fase)`.
    pub harmonics: [(f32, f32, f32); 4],
}

impl LakeShape {
    /// Contorno de um lago ancorado em `at` — determinístico por posição.
    /// Hash da família `sin·43758` do repo, um salt POR parâmetro: as fases
    /// antigas derivavam todas de UM `base` e as silhuetas repetiam-se
    /// entre lagos.
    pub fn new(at: Vec2) -> Self {
        Self::from_authoring(at, &LakeAuthoring::default())
    }

    /// Contorno com autoria opcional ([`LakeAuthoring`]). Sem autoria é o
    /// caminho histórico do [`Self::new`] — bit-exact em mundos existentes.
    /// Com `seed`, os uniformes saem do SplitMix64 da engine
    /// (`splitmix64` local, topo 24 bits → f32 [0,1)): mesma seed,
    /// mesma forma, em qualquer canto do mundo.
    pub fn from_authoring(at: Vec2, a: &LakeAuthoring) -> Self {
        let h = |salt: f32| -> f32 {
            let s = at.x * 12.989_8_f32 + at.y * 78.233_f32;
            ((s * salt + salt * 91.7).sin() * 43_758.55_f32)
                .fract()
                .abs()
        };
        let g = |salt: u64| -> f32 {
            let mut state = a.seed.unwrap_or(0) ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            ((splitmix64(&mut state) >> 40) as f32) / 16_777_216.0
        };
        // Um uniforme por parâmetro, da fonte que a autoria escolher.
        let pick =
            |fsalt: f32, usalt: u64| -> f32 { if a.seed.is_some() { g(usalt) } else { h(fsalt) } };
        let mut harmonics = std::array::from_fn(|i| {
            let amp = LAKE_HARMONIC_MIN[i]
                + (LAKE_HARMONIC_MAX[i] - LAKE_HARMONIC_MIN[i])
                    * pick(2.3 + i as f32 * 1.37, 11 + i as u64);
            let phase = pick(3.7 + i as f32 * 1.73, 21 + i as u64) * std::f32::consts::TAU;
            ([1.0_f32, 3.0, 5.0, 7.0][i], amp, phase)
        });
        let mut axis = pick(0.77, 1) * std::f32::consts::TAU;
        let mut stretch = LAKE_STRETCH_MIN + (LAKE_STRETCH_MAX - LAKE_STRETCH_MIN) * pick(1.31, 2);
        if let Some(deg) = a.axis_deg {
            axis = deg.to_radians();
        }
        if let Some(s) = a.stretch {
            stretch = s.clamp(0.0, 0.45);
        }
        if let Some(lobes) = a.lobes {
            let m = lobes.clamp(0.0, 1.6);
            for h_ in &mut harmonics {
                h_.1 *= m;
            }
        }
        let mut shape = Self {
            axis,
            stretch,
            harmonics,
        };
        shape.fit_to_contour_envelope();
        shape
    }

    /// Reescala os desvios do círculo para o contorno caber no envelope
    /// `[1 − CONTOUR_PEAK, CONTOUR_PEAK]` — o MESMO pior caso que o carve
    /// (`carve_r`), o espelho (`lake_mirror_reach`) e o audit assumem.
    /// Autorias agressivas (`stretch` alto × `lobes` alto) são comprimidas
    /// proporcionalmente, nunca rejeitadas: o lago fica menos exuberante em
    /// vez de partir os consumidores que contam com o envelope.
    fn fit_to_contour_envelope(&mut self) {
        let (mut max, mut min) = (f32::NEG_INFINITY, f32::INFINITY);
        for i in 0..128 {
            let r = self.radius_frac(i as f32 / 128.0 * std::f32::consts::TAU);
            max = max.max(r);
            min = min.min(r);
        }
        let mut scale = 1.0_f32;
        if max > CONTOUR_PEAK {
            scale = scale.min((CONTOUR_PEAK - 1.0) / (max - 1.0));
        }
        if min < 1.0 - CONTOUR_PEAK {
            scale = scale.min((1.0 - CONTOUR_PEAK) / (min - 1.0));
        }
        if scale < 1.0 {
            // 0.995 = margem para o máximo verdadeiro cair ENTRE as amostras
            // da sondagem (subavaliação ≲0,1% do desvio).
            let scale = scale * 0.995;
            self.stretch *= scale;
            for h in &mut self.harmonics {
                h.1 *= scale;
            }
        }
    }

    /// Fração do raio nominal em `theta` — o contorno orgânico em si,
    /// dentro de `[1 − pico, CONTOUR_PEAK]` (ver
    /// `test_contour_peak_covers_the_harmonics`).
    pub fn radius_frac(&self, theta: f32) -> f32 {
        let mut r = 1.0 + self.stretch * (2.0 * (theta - self.axis)).cos();
        for &(k, a, p) in &self.harmonics {
            r += a * (k * theta + p).sin();
        }
        r
    }

    /// Raio do contorno em `theta` para um lago de raio nominal `radius`.
    pub fn contour(&self, radius: f32, theta: f32) -> f32 {
        radius * self.radius_frac(theta)
    }

    /// Conservative `(min_frac, max_frac)` radial envelope for `radius_frac`,
    /// avoiding trigonometric calls outside the inner/outer annuli.
    #[inline]
    pub fn radius_frac_bounds(&self) -> (f32, f32) {
        let dev = self.stretch.abs()
            + self.harmonics[0].1.abs()
            + self.harmonics[1].1.abs()
            + self.harmonics[2].1.abs()
            + self.harmonics[3].1.abs();
        ((1.0 - dev).max(0.0), 1.0 + dev)
    }
}

impl Default for LakeShape {
    /// Corpo circular — rios e corpos construídos à mão não usam a forma.
    fn default() -> Self {
        Self {
            axis: 0.0,
            stretch: 0.0,
            harmonics: [
                (1.0, 0.0, 0.0),
                (3.0, 0.0, 0.0),
                (5.0, 0.0, 0.0),
                (7.0, 0.0, 0.0),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_winding_river() -> WaterBody {
        let mut stations = Vec::with_capacity(400);
        let mut surface_y = Vec::with_capacity(400);
        let mut half_width = Vec::with_capacity(400);
        let mut depths = Vec::with_capacity(400);
        for i in 0..400 {
            let s = i as f32 * 3.0;
            let x = s - 600.0;
            let z = (s * 0.015).sin() * 80.0;
            stations.push(Vec2::new(x, z));
            surface_y.push(12.0 - s * 0.004);
            half_width.push(6.0 + 3.0 * (s * 0.03).cos().abs());
            depths.push(1.8 + (s * 0.02).sin().abs());
        }
        WaterBody {
            kind: WaterKind::River,
            at: Vec2::ZERO,
            radius: 0.0,
            carve_radius: 12.0,
            hull: 650.0,
            water_y: 10.0,
            mirror_reach: 1.0,
            shape: LakeShape::default(),
            stations,
            surface_y,
            water_width: 14.0,
            half_width,
            depths,
            cascades: vec![CascadeInfo {
                lip: 120,
                base: 123,
                drop: 3.0,
                waterfall: true,
                wall: false,
                top_y: 11.5,
                bot_y: 8.5,
            }],
        }
    }

    fn sample_lake() -> WaterBody {
        let at = Vec2::new(42.0, -18.0);
        WaterBody {
            kind: WaterKind::Lake,
            at,
            radius: 34.0,
            carve_radius: 52.0,
            hull: 0.0,
            water_y: 8.5,
            mirror_reach: 1.08,
            shape: LakeShape::new(at),
            stations: Vec::new(),
            surface_y: Vec::new(),
            water_width: 0.0,
            half_width: Vec::new(),
            depths: Vec::new(),
            cascades: Vec::new(),
        }
    }

    #[test]
    fn water_body_index_matches_linear_queries_exactly() {
        let river = sample_winding_river();
        let lake = sample_lake();
        let r_idx = WaterBodyIndex::for_body(&river);
        let l_idx = WaterBodyIndex::for_body(&lake);

        for iz in -30..=30 {
            for ix in -60..=60 {
                let p = Vec2::new(ix as f32 * 9.5, iz as f32 * 4.5);
                assert_eq!(
                    r_idx.surface_y_at(&river, p),
                    river.surface_y_at(p),
                    "river surface mismatch at {p:?}"
                );
                assert_eq!(
                    r_idx.blend_surface_y(&river, p, 3.0),
                    river.blend_surface_y(p, 3.0),
                    "river blend mismatch at {p:?}"
                );
                let d_lin = river.distance_to_waterline(p);
                let d_idx = r_idx.distance_to_waterline(&river, p);
                assert!(
                    (d_lin - d_idx).abs() < 1e-4,
                    "river waterline dist mismatch at {p:?}: {d_lin} vs {d_idx}"
                );
                let s_lin = river.sample_domain(p, Some(9.2), 1.5);
                let s_idx = r_idx.sample_domain_with_bed(&river, p, || Some(9.2), 1.5);
                match (s_lin, s_idx) {
                    (None, None) => {}
                    (Some(a), Some(b)) => {
                        assert!((a.surface_y - b.surface_y).abs() < 1e-5);
                        assert!((a.depth - b.depth).abs() < 1e-5);
                        assert!((a.flow - b.flow).length() < 1e-5);
                        assert!((a.plunge - b.plunge).abs() < 1e-5);
                    }
                    _ => panic!("river domain presence mismatch at {p:?}"),
                }

                let lp = lake.at + p * 0.2;
                assert_eq!(l_idx.surface_y_at(&lake, lp), lake.surface_y_at(lp));
                assert_eq!(
                    l_idx.blend_surface_y(&lake, lp, 2.5),
                    lake.blend_surface_y(lp, 2.5)
                );
                let ls_lin = lake.sample_domain(lp, Some(6.0), 1.5);
                let ls_idx = l_idx.sample_domain_with_bed(&lake, lp, || Some(6.0), 1.5);
                assert_eq!(ls_lin.is_some(), ls_idx.is_some());
                if let (Some(a), Some(b)) = (ls_lin, ls_idx) {
                    assert!((a.depth - b.depth).abs() < 1e-5);
                }
            }
        }
    }
}
