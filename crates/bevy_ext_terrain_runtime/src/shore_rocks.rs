//! Pedras de margem automáticas — `rocks="1"` no `<Lake>`/`<River>`.
//!
//! Caminha a linha de água de cada corpo (contorno harmónico do lago ou as
//! estações do rio) e devolve CANDIDATOS XZ determinísticos na banda de
//! margem: a maioria em seco (a faixa que a maré do splat pinta de areia),
//! uma fração deliberadamente dentro da água rasa — o clássico "pedras a
//! sair da água". As posições alimentam o pipeline de spawner como
//! `fixed_candidates` (os `<StaticSpawner>` do host), que aplica os
//! gates normais (ocupação partilhada, estradas, declive, align-to-terrain)
//! e a escolha de template/escala/yaw pelo RNG — mesma seed, mesmo mundo.
//!
//! Puro: sem ECS, sem assets — unit-testável headless (a entrega via
//! `collect_spawn_groups` é a única parte viva).

use bevy::math::Vec2;

use super::water::{LakeShape, LakeSpec, waterline_reach};

/// GLBs de pedra do pool, em ordem de "pretensão" (boulder > musgo > seixo).
/// Um é escolhido por instância pelo RNG do grupo de spawner.
pub const ROCK_TEMPLATES: [&str; 4] = [
    "/assets/meshes/props/rock_boulder_lod0.glb",
    "/assets/meshes/props/rock_mossy_lod0.glb",
    "/assets/meshes/swamp/moss_rock_lod0.glb",
    "/assets/meshes/farm/flint_rock_lod0.glb",
];

/// Ladders de LOD por template (mesmo índice de [`ROCK_TEMPLATES`]) —
/// `(lod1, lod2)`, thresholds próximos aos dos mundos do pool (50/120 m).
pub const ROCK_LODS: [(Option<&str>, Option<&str>); 4] = [
    (
        Some("/assets/meshes/props/rock_boulder_lod1.glb"),
        Some("/assets/meshes/props/rock_boulder_lod2.glb"),
    ),
    (
        Some("/assets/meshes/props/rock_mossy_lod1.glb"),
        Some("/assets/meshes/props/rock_mossy_lod2.glb"),
    ),
    (
        Some("/assets/meshes/swamp/moss_rock_lod1.glb"),
        Some("/assets/meshes/swamp/moss_rock_lod2.glb"),
    ),
    (
        Some("/assets/meshes/farm/flint_rock_lod1.glb"),
        Some("/assets/meshes/farm/flint_rock_lod2.glb"),
    ),
];

/// Parâmetros parseados (`rocks-density` / `rocks-scale-max`, seed derivada
/// da posição do corpo).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShoreRocksSpec {
    /// Pedras por metro de linha de água (default 0.12 — ~1 pedra a cada
    /// 8 m; lagos pequenos ficam com 4–6, rios ganham fileiras ralas).
    pub density: f32,
    /// Escala máxima do GLB (mínimo fixo em 0.55 — nunca migalhas).
    pub scale_max: f32,
}

impl Default for ShoreRocksSpec {
    fn default() -> Self {
        Self {
            density: 0.12,
            scale_max: 1.4,
        }
    }
}

/// Teto por corpo — uma margem nunca vira um muro de pedra.
const MAX_PER_BODY: usize = 160;
/// Recuo/avanço radial em relação à linha de água (m): ~30% nascem dentro
/// da água rasa, o resto espalha-se até aqui em seco.
const DRY_SPREAD: f32 = 3.5;
const WET_DEPTH: f32 = 1.2;

/// SplitMix64 partilhado ([`bevy_ext_core::rng::Rng`]) semeado com XOR fixo — os
/// outputs têm de continuar idênticos aos do `Rng` local que vivia aqui.
use super::water::station_curvatures;
use bevy_ext_core::rng::Rng;

/// Metadata for a single clustered shoreline boulder/cobble placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShoreRockPlacement {
    pub pos: Vec2,
    /// Relative radius multiplier (`0.42..=1.15`): primary cluster anchors are
    /// larger than companion satellites.
    pub scale_factor: f32,
    pub yaw: f32,
    /// Positive on river outer cutbanks (`0` for lakes / straight stretches).
    pub outer_bias: f32,
    /// True when placed inside or right along the shallow wet edge.
    pub half_submerged: bool,
}

/// Seed derivada da posição do corpo (estável entre runs, distinta por
/// corpo sobreposto).
fn body_seed(at: Vec2, index: usize) -> u64 {
    (at.x.to_bits() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ ((at.y.to_bits() as u64).rotate_left(17))
        ^ (index as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
}

/// Sal fixo que o `Rng` local antigo aplicava no `new` — preservado para o
/// seq shuffler das pedras não mudar.
const SHORE_SEED_SALT: u64 = 0x5EED_5EED_5EED_5EED;

/// Emits natural clustered boulder + satellite cobble placements around a lake's
/// organic waterline. Primary anchors cluster near coves and promontories while
/// keeping every point inside the `[-WET_DEPTH, +DRY_SPREAD]` shore band.
pub fn lake_rock_clusters(spec: &LakeSpec, index: usize) -> Vec<ShoreRockPlacement> {
    let at = spec.at;
    if spec.radius <= 0.0 {
        return Vec::new();
    }
    let shape = LakeShape::from_authoring(at, &spec.shape);
    let reach = super::water::lake_mirror_reach(spec.depth, spec.water_offset);
    if reach <= 0.0 {
        return Vec::new();
    }
    let samples = 96;
    let mut perimeter = 0.0;
    let mut prev: Option<Vec2> = None;
    for i in 0..=samples {
        let theta = i as f32 / samples as f32 * std::f32::consts::TAU;
        let r = shape.contour(spec.radius, theta) * reach;
        let p = at + Vec2::new(theta.cos(), theta.sin()) * r;
        if let Some(q) = prev {
            perimeter += p.distance(q);
        }
        prev = Some(p);
    }
    let step = 1.0 / spec.rocks_spec.density.max(0.01);
    let target_count = ((perimeter / step) as usize).min(MAX_PER_BODY);
    if target_count == 0 {
        return Vec::new();
    }
    let seed = spec
        .shape
        .seed
        .map(|s| s ^ (index as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .unwrap_or_else(|| body_seed(at, index));
    let mut rng = Rng::new(seed ^ SHORE_SEED_SALT);
    let cluster_budget = target_count.div_ceil(2).max(1);
    let mut out = Vec::with_capacity(target_count);
    for c in 0..cluster_budget {
        if out.len() >= target_count {
            break;
        }
        let base_theta = c as f32 / cluster_budget as f32 * std::f32::consts::TAU
            + rng.range(-0.42, 0.42) / cluster_budget as f32 * std::f32::consts::TAU;
        let wl_anchor = shape.contour(spec.radius, base_theta) * reach;
        let radial_anchor = rng.range(-WET_DEPTH * 0.85, DRY_SPREAD * 0.85);
        let anchor =
            at + Vec2::new(base_theta.cos(), base_theta.sin()) * (wl_anchor + radial_anchor);
        out.push(ShoreRockPlacement {
            pos: anchor,
            scale_factor: rng.range(0.75, 1.15),
            yaw: rng.range(0.0, std::f32::consts::TAU),
            outer_bias: 0.0,
            half_submerged: radial_anchor < 0.25,
        });
        let cluster_size = if rng.range(0.0, 1.0) > 0.35 { 2 } else { 1 };
        let sat_radius_scale = spec.radius.clamp(6.0, 24.0).recip();
        for _ in 0..cluster_size {
            if out.len() >= target_count {
                break;
            }
            let dtheta = rng.range(-2.2, 2.2) * sat_radius_scale;
            let stheta = base_theta + dtheta;
            let swl = shape.contour(spec.radius, stheta) * reach;
            let srad = (radial_anchor + rng.range(-1.3, 1.4)).clamp(-WET_DEPTH, DRY_SPREAD);
            out.push(ShoreRockPlacement {
                pos: at + Vec2::new(stheta.cos(), stheta.sin()) * (swl + srad),
                scale_factor: rng.range(0.42, 0.72),
                yaw: rng.range(0.0, std::f32::consts::TAU),
                outer_bias: 0.0,
                half_submerged: srad < 0.25,
            });
        }
    }
    out
}

/// Candidatos ao longo do contorno orgânico de um lago em clusters naturais
/// com pedras principais + satélites dentro da banda de margem.
pub fn lake_candidates(spec: &LakeSpec, index: usize) -> Vec<Vec2> {
    lake_rock_clusters(spec, index)
        .into_iter()
        .map(|p| p.pos)
        .collect()
}

/// Emits natural clustered rock/cobble placements along river banks, biased toward
/// outer eroded cutbank toes (where talus collects) and shallow riffle shoals.
pub fn river_rock_clusters(
    stations: &[Vec2],
    width: f32,
    depth: f32,
    water_offset: f32,
    bank_width: f32,
    rocks: &ShoreRocksSpec,
    index: usize,
) -> Vec<ShoreRockPlacement> {
    if stations.len() < 2 || width <= 0.0 {
        return Vec::new();
    }
    let t_wl = waterline_reach(depth, water_offset).clamp(0.4, 1.0);
    let waterline = width * 0.5 * t_wl;
    let outer_edge = width * 0.5 + bank_width.max(0.0) + 2.0;
    let lat_min = (waterline * 0.85 - WET_DEPTH * 0.88).max(waterline * 0.65);
    let lat_max = outer_edge.max(lat_min + 1.0);
    let mut length = 0.0;
    for pair in stations.windows(2) {
        length += pair[0].distance(pair[1]);
    }
    let step = 1.0 / rocks.density.max(0.01);
    let target_count = ((length / step) as usize).min(MAX_PER_BODY);
    if target_count == 0 {
        return Vec::new();
    }
    let curvatures = station_curvatures(stations);
    let cluster_budget = target_count.div_ceil(2).max(1);
    let walk_step = length / cluster_budget as f32;
    let mut rng = Rng::new(body_seed(stations[0], index) ^ SHORE_SEED_SALT);
    let mut out = Vec::with_capacity(target_count);
    let mut target = walk_step * 0.5;
    let mut walked = 0.0;
    for (seg_idx, pair) in stations.windows(2).enumerate() {
        let seg = pair[0].distance(pair[1]);
        if seg < 1e-4 {
            continue;
        }
        let dir = (pair[1] - pair[0]) / seg;
        let perp = Vec2::new(-dir.y, dir.x);
        let curv = curvatures[seg_idx];
        while target <= walked + seg && out.len() < target_count {
            let along = target - walked;
            let base = pair[0] + dir * along;
            // On meanders (`curv != 0`), bias ~75% toward the outer cutbank toe
            // (`side = -curv.signum()`); on straight reaches alternate randomly.
            let outer_side = if curv >= 0.0 { -1.0 } else { 1.0 };
            let curv_mag = curv.abs();
            let side = if curv_mag > 0.12 {
                if rng.range(0.0, 1.0) < 0.78 {
                    outer_side
                } else {
                    -outer_side
                }
            } else if rng.next_u64() & 1 == 0 {
                1.0
            } else {
                -1.0
            };
            let is_outer = curv_mag * (side * outer_side);
            // Cutbank boulders hug near the water-toe interface; point-bar
            // cobbles spread slightly out across shallow sand/gravel bars.
            let lat_center = if is_outer > 0.15 {
                waterline * 0.92 + rng.range(-WET_DEPTH * 0.75, 1.8)
            } else {
                waterline * 0.85 + rng.range(-WET_DEPTH * 0.75, (lat_max - waterline * 0.85) * 0.8)
            };
            let anchor_lat = lat_center.clamp(lat_min, lat_max);
            let slide = rng.range(-1.8, 1.8);
            let anchor = base + perp * (side * anchor_lat) + dir * slide;
            out.push(ShoreRockPlacement {
                pos: anchor,
                scale_factor: rng.range(0.76, 1.15) * (1.0 + 0.15 * is_outer.max(0.0)),
                yaw: rng.range(0.0, std::f32::consts::TAU),
                outer_bias: is_outer,
                half_submerged: anchor_lat < waterline + 0.35,
            });
            let sat_n = if is_outer > 0.2 || rng.range(0.0, 1.0) > 0.42 {
                2
            } else {
                1
            };
            for _ in 0..sat_n {
                if out.len() >= target_count {
                    break;
                }
                let slat = (anchor_lat + rng.range(-1.2, 1.4)).clamp(lat_min, lat_max);
                let sslip = slide + rng.range(-2.2, 2.2);
                out.push(ShoreRockPlacement {
                    pos: base + perp * (side * slat) + dir * sslip,
                    scale_factor: rng.range(0.40, 0.72),
                    yaw: rng.range(0.0, std::f32::consts::TAU),
                    outer_bias: is_outer,
                    half_submerged: slat < waterline + 0.35,
                });
            }
            target += walk_step;
        }
        walked += seg;
    }
    out
}

/// Candidatos em clusters ao longo das margens de um rio (acúmulo em curvas
/// externas e baixios).
pub fn river_candidates(
    stations: &[Vec2],
    width: f32,
    depth: f32,
    water_offset: f32,
    bank_width: f32,
    rocks: &ShoreRocksSpec,
    index: usize,
) -> Vec<Vec2> {
    river_rock_clusters(stations, width, depth, water_offset, bank_width, rocks, index)
        .into_iter()
        .map(|p| p.pos)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lake_candidates_ring_the_shore() {
        let rocks = ShoreRocksSpec::default();
        let spec = LakeSpec {
            at: Vec2::new(-40.0, 20.0),
            radius: 12.0,
            depth: 3.0,
            ..LakeSpec::default()
        };
        let a = lake_candidates(&spec, 0);
        let b = lake_candidates(&spec, 0);
        assert_eq!(a, b, "same body → same candidates");
        assert!(!a.is_empty(), "a 12 m lake has shore rocks");
        // Todos os candidatos ficam perto do contorno (banda húmida+seca).
        for p in &a {
            let d = p.distance(spec.at);
            assert!(
                d > 6.0 && d < 12.0 * 1.25 + DRY_SPREAD + 1.0,
                "candidate {p} off the shore band (d={d})"
            );
        }
        // Densidade maior → mais pedras.
        let dense = LakeSpec {
            rocks_spec: ShoreRocksSpec {
                density: 0.4,
                ..rocks
            },
            ..spec.clone()
        };
        let more = lake_candidates(&dense, 0);
        assert!(more.len() > a.len(), "density drives count");
    }

    /// A autoria da forma (`seed` + `lobes`) muda o contorno que as pedras
    /// seguem — e a mesma seed reproduz exatamente o mesmo anel.
    #[test]
    fn test_lake_candidates_follow_the_authored_shape() {
        let authored = LakeSpec {
            at: Vec2::new(-40.0, 20.0),
            radius: 12.0,
            depth: 3.0,
            shape: super::super::water::LakeAuthoring {
                seed: Some(77),
                lobes: Some(1.5),
                ..Default::default()
            },
            ..LakeSpec::default()
        };
        let a = lake_candidates(&authored, 0);
        let b = lake_candidates(&authored, 0);
        assert_eq!(a, b, "authored seed is reproducible");
        let natural = lake_candidates(
            &LakeSpec {
                at: authored.at,
                radius: authored.radius,
                depth: authored.depth,
                ..LakeSpec::default()
            },
            0,
        );
        assert_ne!(a, natural, "authored shape moves the rock ring");
    }

    #[test]
    fn test_river_candidates_line_the_banks() {
        let rocks = ShoreRocksSpec::default();
        let stations = vec![Vec2::new(0.0, -30.0), Vec2::new(0.0, 30.0)];
        let cands = river_candidates(&stations, 6.0, 2.0, 0.4, 2.0, &rocks, 0);
        assert!(!cands.is_empty());
        // Nenhum candidato no centro do canal: |x| > waterline·0.85 − WET.
        let waterline = 3.0 * waterline_reach(2.0, 0.4).clamp(0.4, 1.0);
        for p in &cands {
            assert!(
                p.x.abs() > (waterline * 0.85 - WET_DEPTH - 0.5).max(0.0),
                "candidate {p} in mid-channel"
            );
        }
    }

    #[test]
    fn test_degenerate_bodies_give_no_candidates() {
        let rocks = ShoreRocksSpec::default();
        assert!(
            lake_candidates(
                &LakeSpec {
                    at: Vec2::ZERO,
                    radius: 0.0,
                    depth: 2.0,
                    rocks_spec: rocks,
                    ..LakeSpec::default()
                },
                0
            )
            .is_empty()
        );
        assert!(river_candidates(&[], 6.0, 2.0, 0.4, 2.0, &rocks, 0).is_empty());
    }

    #[test]
    fn test_river_rock_clusters_bias_outer_bend() {
        let rocks = ShoreRocksSpec {
            density: 0.35,
            scale_max: 1.6,
        };
        let stations: Vec<Vec2> = (0..=20)
            .map(|i| {
                let t = (i as f32 / 20.0 - 0.5) * 2.0;
                Vec2::new(t * 40.0, (1.0 - t * t) * 24.0)
            })
            .collect();
        let clusters = river_rock_clusters(&stations, 8.0, 2.0, 0.5, 4.0, &rocks, 0);
        assert!(!clusters.is_empty());
        let outer_count = clusters.iter().filter(|c| c.outer_bias > 0.1).count();
        let inner_count = clusters.iter().filter(|c| c.outer_bias < -0.1).count();
        assert!(
            outer_count > inner_count,
            "outer cutbank clusters must outnumber inner point bars: outer={outer_count} vs inner={inner_count}"
        );
        assert!(clusters.iter().any(|c| c.half_submerged));
        assert!(clusters.iter().any(|c| c.scale_factor < 0.75));
        assert!(clusters.iter().any(|c| c.scale_factor > 0.85));
    }
}
