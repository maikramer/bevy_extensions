//! Hidráulica de rio por secção molhada.
//!
//! O carve abre um canal real (assimétrico nas curvas, alargado por margens,
//! estradas e confluências); a lâmina e as queries de água leem a secção
//! MOLHADA dele — onde a superfície toca o terreno de cada lado — em vez da
//! largura nominal da spec. Desta secção sai a corrente:
//!
//! * **declive**: a velocidade de base segue a curva de declive afinada para
//!   os barcos (`1,15 + 14·S`, 0,9…3,8 m/s; troços navegáveis ≤ ~2,3 m/s).
//!   Manning puro com os declives reais do mundo saturava todos os rios;
//! * **continuidade** num troço sem quedas: o caudal é o mesmo, portanto o
//!   estreito acelera e o poço abranda (`× mediana(A)/A`, 0,65…1,6);
//! * **perfil lateral** por faixa vertical, `v(y) = v̄·(h(y)/h̄)^(2/3)` — lento
//!   na margem e no baixio, rápido no talvegue (que o carve já pôs na margem
//!   exterior das curvas);
//! * **remanso**: a jusante de um alargamento brusco de um lado, a corrente
//!   descola dessa margem e volta para trás junto dela (redemoinho).
//!
//! Tudo puro e determinístico; o mesmo [`section_flow`] serve a malha (vértice)
//! e o domínio (barco, peixes, rasterizador do solver), por isso a corrente
//! desenhada e a sentida são a mesma.

use bevy::math::Vec2;

use crate::body::CascadeInfo;

/// Velocidade média mínima / máxima de uma secção (m/s). O teto fica abaixo
/// do limiar de cachoeira do shader (5,4 m/s).
pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 4.4;
/// Modulação da continuidade (`mediana(A)/A`): limites do estreito / poço.
const CONTINUITY_MIN: f32 = 0.65;
const CONTINUITY_MAX: f32 = 1.6;
/// Gravidade para o número de Froude.
const G: f32 = 9.81;

/// Geometria de uma secção molhada (amostrada sobre o terreno carvado).
#[derive(Debug, Clone, Copy, PartialEq, Default, bevy::prelude::Reflect)]
pub struct SectionGeom {
    /// Área molhada (m²).
    pub area: f32,
    /// Largura molhada (m) = esquerda + direita.
    pub width: f32,
    /// Perímetro molhado (m).
    pub perimeter: f32,
    /// Profundidade máxima da secção (m).
    pub max_depth: f32,
}

/// Hidráulica resolvida de uma estação.
#[derive(Debug, Clone, Copy, PartialEq, Default, bevy::prelude::Reflect)]
pub struct StationHydro {
    /// Largura molhada (m).
    pub width: f32,
    /// Profundidade hidráulica `A / W` (m).
    pub hyd_depth: f32,
    /// Velocidade média da secção (m/s).
    pub speed: f32,
    /// Número de Froude da secção (`v / sqrt(g·h̄)`).
    pub froude: f32,
    /// Intensidade de remanso junto à margem esquerda / direita (0..1).
    pub eddy_left: f32,
    pub eddy_right: f32,
}

impl StationHydro {
    pub fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Self {
            width: l(a.width, b.width),
            hyd_depth: l(a.hyd_depth, b.hyd_depth),
            speed: l(a.speed, b.speed),
            froude: l(a.froude, b.froude),
            eddy_left: l(a.eddy_left, b.eddy_left),
            eddy_right: l(a.eddy_right, b.eddy_right),
        }
    }
}

/// Secção molhada de um rio, por estação. Vazia = contrato antigo (largura
/// nominal simétrica `half_width`, corrente pelo declive).
#[derive(Debug, Clone, PartialEq, Default, bevy::prelude::Reflect)]
pub struct RiverSection {
    /// Meia-largura molhada do lado `−perp` (esquerda da tangente), m.
    pub wet_left: Vec<f32>,
    /// Meia-largura molhada do lado `+perp`, m.
    pub wet_right: Vec<f32>,
    /// Bit 0 = esquerda aberta, bit 1 = direita aberta: a lâmina não tocou o
    /// terreno dentro do teto (encosta a descer, foz) — fica a nominal.
    pub open: Vec<u8>,
    /// Hidráulica por estação.
    pub hydro: Vec<StationHydro>,
}

impl RiverSection {
    pub fn is_empty(&self) -> bool {
        self.wet_left.is_empty()
    }
    /// Secção válida para `n` estações.
    pub fn matches(&self, n: usize) -> bool {
        self.wet_left.len() == n
            && self.wet_right.len() == n
            && self.open.len() == n
            && self.hydro.len() == n
    }
}

/// Lado da secção: `lateral < 0` é a esquerda (`−perp`).
#[inline]
pub fn side_is_left(lateral: f32) -> bool {
    lateral < 0.0
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    values[values.len() / 2]
}

fn box_smooth(values: &mut [f32], half: usize) {
    if values.len() < 3 || half == 0 {
        return;
    }
    let src = values.to_vec();
    let n = src.len();
    for (i, v) in values.iter_mut().enumerate() {
        let a = i.saturating_sub(half);
        let b = (i + half).min(n - 1);
        *v = src[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
    }
}

/// Resolve a hidráulica de cada estação a partir da geometria molhada.
///
/// `stations`, `surface_y`, `geoms`, `wet_left` e `wet_right` têm o mesmo
/// comprimento. As quedas (`cascades`, `[lip, base]`) partem o rio em troços
/// de caudal próprio — dentro de uma queda manda só o declive local.
pub fn solve_hydro(
    stations: &[Vec2],
    surface_y: &[f32],
    geoms: &[SectionGeom],
    wet_left: &[f32],
    wet_right: &[f32],
    cascades: &[CascadeInfo],
) -> Vec<StationHydro> {
    let n = stations.len();
    if n < 2
        || surface_y.len() != n
        || geoms.len() != n
        || wet_left.len() != n
        || wet_right.len() != n
    {
        return Vec::new();
    }
    // Declive suavizado (±2 estações).
    let slope: Vec<f32> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(2);
            let b = (i + 2).min(n - 1);
            let run: f32 = (a..b).map(|k| stations[k].distance(stations[k + 1])).sum();
            ((surface_y[a] - surface_y[b]) / run.max(0.5)).clamp(0.0, 0.35)
        })
        .collect();

    let in_cascade = |i: usize| cascades.iter().any(|c| i >= c.lip && i <= c.base);

    // Base pelo declive (a mesma curva do contrato antigo do domínio).
    let base: Vec<f32> = slope.iter().map(|&s| (1.15 + 14.0 * s).clamp(0.9, 3.8)).collect();

    // Continuidade por troço sem quedas: Q constante ⇒ v ∝ 1/A.
    let mut speed = base.clone();
    let mut start = 0usize;
    while start < n {
        if in_cascade(start) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end + 1 < n && !in_cascade(end + 1) {
            end += 1;
        }
        let mut areas: Vec<f32> = (start..=end)
            .map(|i| geoms[i].area)
            .filter(|&a| a > 0.02)
            .collect();
        let a_med = median(&mut areas);
        if a_med > 0.0 {
            for i in start..=end {
                let a = geoms[i].area.max(0.02);
                speed[i] = base[i] * (a_med / a).clamp(CONTINUITY_MIN, CONTINUITY_MAX);
            }
        }
        start = end + 1;
    }
    for v in &mut speed {
        *v = v.clamp(MIN_SPEED, MAX_SPEED);
    }
    box_smooth(&mut speed, 1);

    // Alargamento por lado → remanso a jusante (decai em ~4 estações).
    let expansion = |w: &[f32], i: usize| -> f32 {
        let a = i.saturating_sub(2);
        let b = (i + 2).min(n - 1);
        if a == b {
            return 0.0;
        }
        let run: f32 = (a..b).map(|k| stations[k].distance(stations[k + 1])).sum();
        (w[b] - w[a]) / run.max(0.5)
    };
    let eddy_of = |w: &[f32]| -> Vec<f32> {
        // Larguras suavizadas (±2): o ruído de texel da margem não é um
        // alargamento.
        let mut ws = w.to_vec();
        box_smooth(&mut ws, 2);
        let raw: Vec<f32> = (0..n).map(|i| smoothstep(0.15, 0.50, expansion(&ws, i))).collect();
        (0..n)
            .map(|i| {
                let mut e = 0.0f32;
                for (k, j) in (i.saturating_sub(4)..=i).rev().enumerate() {
                    e = e.max(raw[j] * 0.78f32.powi(k as i32));
                }
                if in_cascade(i) { 0.0 } else { e.min(1.0) }
            })
            .collect()
    };
    let eddy_l = eddy_of(wet_left);
    let eddy_r = eddy_of(wet_right);

    (0..n)
        .map(|i| {
            let g = &geoms[i];
            let width = g.width.max(0.1);
            let hyd_depth = (g.area / width).max(0.05);
            let v = speed[i];
            StationHydro {
                width,
                hyd_depth,
                speed: v,
                froude: v / (G * hyd_depth).sqrt(),
                eddy_left: eddy_l[i],
                eddy_right: eddy_r[i],
            }
        })
        .collect()
}

/// Amostra da corrente num ponto da secção.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowSample {
    /// Velocidade de superfície (m/s, XZ).
    pub flow: Vec2,
    /// Froude local (`|flow| / sqrt(g·h)`).
    pub froude: f32,
    /// Remanso local (0..1).
    pub eddy: f32,
    /// Distância à margem molhada do lado do ponto (m, ≥ 0).
    pub bank_dist: f32,
}

/// Corrente no ponto `lateral` (m, sinal = lado; `< 0` = esquerda/`−perp`)
/// de uma secção com hidráulica `h`, meia-largura molhada `half_side` desse
/// lado e profundidade local `local_depth`. `dir` é a tangente a jusante.
pub fn section_flow(
    dir: Vec2,
    h: &StationHydro,
    lateral: f32,
    half_side: f32,
    local_depth: f32,
) -> FlowSample {
    let half_side = half_side.max(0.1);
    let u = (lateral.abs() / half_side).clamp(0.0, 1.0);
    let bank_dist = (half_side - lateral.abs()).max(0.0);
    let h_bar = h.hyd_depth.max(0.05);
    let ratio = (local_depth.max(0.0) / h_bar).clamp(0.0, 1.8);
    // Faixa vertical de Manning + camada limite da margem (o leito raso já
    // abranda, mas uma margem a pique com 1 m de água também arrasta).
    let wall = smoothstep(0.0, 0.9, bank_dist).sqrt();
    let profile = (ratio.powf(2.0 / 3.0) * (0.25 + 0.75 * wall)).max(0.06);
    let mut flow = dir * (h.speed * profile);

    let eddy_side = if side_is_left(lateral) { h.eddy_left } else { h.eddy_right };
    let band = smoothstep(0.40, 0.78, u) * (1.0 - smoothstep(0.96, 1.0, u));
    let eddy = (eddy_side * band).clamp(0.0, 1.0);
    if eddy > 0.0 {
        let perp = Vec2::new(-dir.y, dir.x);
        let to_center = -perp * lateral.signum();
        // A corrente descola da margem: perde o avanço, inverte junto dela e
        // gira para o centro.
        flow = flow * (1.0 - 1.7 * eddy) + to_center * (eddy * h.speed * 0.28);
    }
    let speed = flow.length();
    FlowSample {
        flow,
        froude: speed / (G * local_depth.max(0.05)).sqrt(),
        eddy,
        bank_dist,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight(n: usize, spacing: f32, drop_per_m: f32) -> (Vec<Vec2>, Vec<f32>) {
        let st: Vec<Vec2> = (0..n).map(|i| Vec2::new(i as f32 * spacing, 0.0)).collect();
        let y: Vec<f32> = (0..n).map(|i| 10.0 - i as f32 * spacing * drop_per_m).collect();
        (st, y)
    }

    fn rect(width: f32, depth: f32) -> SectionGeom {
        SectionGeom {
            area: width * depth,
            width,
            perimeter: width + 2.0 * depth,
            max_depth: depth,
        }
    }

    #[test]
    fn narrows_accelerate_and_pools_slow_down() {
        let n = 30;
        let (st, y) = straight(n, 3.0, 0.004);
        let mut geoms = vec![rect(8.0, 1.0); n];
        for g in &mut geoms[12..16] {
            *g = rect(4.0, 1.0); // estreito
        }
        for g in &mut geoms[22..26] {
            *g = rect(14.0, 1.8); // poço
        }
        let wl: Vec<f32> = geoms.iter().map(|g| g.width * 0.5).collect();
        let h = solve_hydro(&st, &y, &geoms, &wl, &wl, &[]);
        let reach = h[6].speed;
        assert!(h[14].speed > reach * 1.2, "estreito {} vs {}", h[14].speed, reach);
        assert!(h[24].speed < reach * 0.8, "poço {} vs {}", h[24].speed, reach);
        assert!(h.iter().all(|s| s.speed >= MIN_SPEED && s.speed <= MAX_SPEED));
    }

    #[test]
    fn steeper_reach_is_faster_and_supercritical() {
        let n = 20;
        let (st, y_flat) = straight(n, 3.0, 0.001);
        let (_, y_steep) = straight(n, 3.0, 0.08);
        let geoms = vec![rect(6.0, 0.4); n];
        let w = vec![3.0; n];
        let flat = solve_hydro(&st, &y_flat, &geoms, &w, &w, &[]);
        let steep = solve_hydro(&st, &y_steep, &geoms, &w, &w, &[]);
        assert!(steep[10].speed > flat[10].speed * 1.8);
        assert!(steep[10].froude > 1.0, "froude {}", steep[10].froude);
        assert!(flat[10].froude < 0.65, "froude {}", flat[10].froude);
    }

    #[test]
    fn expansion_on_one_side_makes_an_eddy_on_that_side_only() {
        let n = 30;
        let (st, y) = straight(n, 3.0, 0.004);
        let mut wl = vec![3.0f32; n];
        let wr = vec![3.0f32; n];
        for (i, w) in wl.iter_mut().enumerate().skip(12) {
            *w = (3.0 + (i - 12) as f32 * 1.5).min(9.0);
        }
        let geoms: Vec<SectionGeom> = (0..n).map(|i| rect(wl[i] + wr[i], 1.0)).collect();
        let h = solve_hydro(&st, &y, &geoms, &wl, &wr, &[]);
        let max_l = h.iter().map(|s| s.eddy_left).fold(0.0, f32::max);
        let max_r = h.iter().map(|s| s.eddy_right).fold(0.0, f32::max);
        assert!(max_l > 0.5, "remanso esquerdo {max_l}");
        assert!(max_r < 0.05, "remanso direito {max_r}");
    }

    #[test]
    fn lateral_profile_is_fast_on_thalweg_slow_at_banks_and_reverses_in_eddy() {
        let h = StationHydro {
            width: 8.0,
            hyd_depth: 1.0,
            speed: 2.0,
            froude: 0.6,
            eddy_left: 0.0,
            eddy_right: 0.9,
        };
        let dir = Vec2::X;
        let centre = section_flow(dir, &h, 0.0, 4.0, 1.4);
        let bank = section_flow(dir, &h, -3.8, 4.0, 0.1);
        assert!(centre.flow.x > 2.0);
        assert!(bank.flow.x < centre.flow.x * 0.3);
        assert!((bank.bank_dist - 0.2).abs() < 1e-4);
        let eddy = section_flow(dir, &h, 2.6, 4.0, 0.6);
        assert!(eddy.eddy > 0.5);
        assert!(eddy.flow.x < 0.0, "remanso inverte junto à margem: {:?}", eddy.flow);
        assert!(eddy.flow.y < 0.0, "e gira para o centro: {:?}", eddy.flow);
    }

    #[test]
    fn cascades_split_the_discharge() {
        let n = 30;
        let (st, mut y) = straight(n, 3.0, 0.003);
        for v in &mut y[15..] {
            *v -= 4.0;
        }
        let geoms = vec![rect(6.0, 1.0); n];
        let w = vec![3.0; n];
        let cascades = [CascadeInfo {
            lip: 14,
            base: 15,
            drop: 4.0,
            waterfall: true,
            wall: false,
            top_y: y[14],
            bot_y: y[15],
        }];
        let h = solve_hydro(&st, &y, &geoms, &w, &w, &cascades);
        assert!(h[14].speed > h[5].speed);
        assert_eq!(h[14].eddy_left, 0.0);
    }
}
