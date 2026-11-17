//! Bake das máscaras de densidade e heightmaps a partir do [`GrassTerrain`].
//!
//! Duas passagens, como no Vael original:
//!
//! 1. **estática** (`static_cell`, 4 m por omissão): cobertura e regras de
//!    sítio do jogo numa grelha grossa do mundo, guardada entre rebakes;
//! 2. **dinâmica** (`cell`, 2 m): alturas atuais, declive e altitude em cada
//!    texel, amostrando a grelha estática por bioma.
//!
//! A passagem dinâmica corre a cada edição viva do terreno; a estática só
//! quando o provider/layout muda. O resultado são imagens `R32Float`
//! (heightmap) e `Rg8Unorm` (densidade/altura) que o `bevy_grass` lê.

use bevy::log::info;
use bevy::math::{UVec2, Vec2};
use bevy::prelude::Resource;

use crate::terrain::{FieldPlan, GrassTerrain};

/// Parâmetros do bake. O jogo pode afinar sem tocar no crate.
#[derive(Resource, Debug, Clone, Copy)]
pub struct GrassBakeConfig {
    /// Metro por texel da heightmap/máscara final.
    pub cell: f32,
    /// Metro por texel da grelha estática (regras de sítio + bioma).
    pub static_cell: f32,
    /// Declive máximo (graus) que ainda segura relva.
    pub max_slope_deg: f32,
    /// Faixa suave antes do corte do declive (graus).
    pub slope_soft_deg: f32,
    /// Altitude (m) onde a relva começa a rarear.
    pub altitude_fade_start: f32,
    /// Altitude (m) onde a relva desaparece.
    pub altitude_fade_end: f32,
    /// Segundos de silêncio depois de uma edição viva antes de refazer.
    pub edit_debounce: f32,
}

impl Default for GrassBakeConfig {
    fn default() -> Self {
        Self {
            cell: 2.0,
            static_cell: 4.0,
            max_slope_deg: 38.0,
            slope_soft_deg: 14.0,
            altitude_fade_start: 118.0,
            altitude_fade_end: 168.0,
            edit_debounce: 0.6,
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Densidade e bioma numa grelha grossa e estática do mundo.
pub struct StaticMaps {
    dim: u32,
    cell: f32,
    /// Densidade `0..255` (0 = bloqueado ou sem cobertura).
    density: Vec<u8>,
    /// Bioma por texel.
    biome: Vec<u16>,
}

impl StaticMaps {
    /// `(bioma, densidade 0..255)` no ponto do mundo, ou `None` fora da grelha.
    #[must_use]
    pub fn sample(&self, world: Vec2) -> Option<(u16, u8)> {
        let half = self.dim as f32 * self.cell * 0.5;
        let fx = ((world.x + half) / self.cell).floor();
        let fz = ((world.y + half) / self.cell).floor();
        if fx < 0.0 || fz < 0.0 || fx >= self.dim as f32 || fz >= self.dim as f32 {
            return None;
        }
        let idx = fz as usize * self.dim as usize + fx as usize;
        Some((self.biome[idx], self.density[idx]))
    }
}

/// Um campo pronto a virar `bevy_grass::GrassField`.
pub struct FieldData {
    pub plan: FieldPlan,
    pub texels: UVec2,
    /// Alturas absolutas (Y do mundo), uma por texel.
    pub heights: Vec<f32>,
    /// Máscara RG interleaved (R = densidade, G = altura).
    pub mask: Vec<u8>,
}

/// Primeira passagem: cobertura + regras de sítio que não mudam com o
/// terreno. Roda uma vez por provider/layout.
pub fn bake_static(terrain: &dyn GrassTerrain, config: &GrassBakeConfig) -> StaticMaps {
    let world_size = terrain.world_size().max(1.0);
    let dim = (world_size / config.static_cell.max(0.5)).ceil().max(1.0) as u32;
    let dim_usize = dim as usize;
    let cell = world_size / dim as f32;
    let min = Vec2::splat(-world_size * 0.5);
    let total = dim_usize * dim_usize;
    let mut density = vec![0u8; total];
    let mut biome = vec![0u16; total];
    let mut blocked = 0u32;
    let mut empty = 0u32;
    let mut ok = 0u32;

    let bake_rows = |start_iz: u32, d_slice: &mut [u8], b_slice: &mut [u16]| -> (u32, u32, u32) {
        let mut l_blocked = 0u32;
        let mut l_empty = 0u32;
        let mut l_ok = 0u32;
        let rows = d_slice.len() / dim_usize;
        for row in 0..rows {
            let iz = start_iz + row as u32;
            let py = min.y + (iz as f32 + 0.5) * cell;
            let row_off = row * dim_usize;
            for ix in 0..dim_usize {
                let px = min.x + (ix as f32 + 0.5) * cell;
                let idx = row_off + ix;
                let ground = terrain.height(px, py);
                let site = terrain.site(px, py, ground);
                b_slice[idx] = site.biome.0;
                if site.blocked {
                    l_blocked += 1;
                    continue;
                }
                if site.coverage <= 0.0 {
                    l_empty += 1;
                    continue;
                }
                d_slice[idx] = (site.coverage * 255.0).round().clamp(0.0, 255.0) as u8;
                l_ok += 1;
            }
        }
        (l_blocked, l_empty, l_ok)
    };

    let workers = if total >= 4096 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(dim_usize)
    } else {
        1
    };
    if workers <= 1 {
        let (b, e, o) = bake_rows(0, &mut density, &mut biome);
        blocked = b;
        empty = e;
        ok = o;
    } else {
        let rows_per_worker = dim_usize.div_ceil(workers);
        let chunk_elems = rows_per_worker * dim_usize;
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for (chunk_idx, (d_chunk, b_chunk)) in density
                .chunks_mut(chunk_elems)
                .zip(biome.chunks_mut(chunk_elems))
                .enumerate()
            {
                let start_iz = (chunk_idx * rows_per_worker) as u32;
                handles.push(scope.spawn(move || bake_rows(start_iz, d_chunk, b_chunk)));
            }
            for handle in handles {
                let (b, e, o) = handle
                    .join()
                    .expect("bevy_ext_grass: bake estático entrou em pânico");
                blocked += b;
                empty += e;
                ok += o;
            }
        });
    }

    info!(
        "bevy_ext_grass: máscara estática {ok}/{} texels a {:.1} m — bloqueados {blocked}, sem cobertura {empty}",
        dim * dim,
        config.static_cell
    );
    StaticMaps {
        dim,
        cell,
        density,
        biome,
    }
}

/// Segunda passagem: alturas atuais + declive/altitude, amostrando a grelha
/// estática. Roda a cada edição viva.
pub fn bake_dynamic(
    terrain: &dyn GrassTerrain,
    config: &GrassBakeConfig,
    plans: &[FieldPlan],
    statics: &StaticMaps,
) -> Vec<FieldData> {
    // Um campo por thread: os campos são independentes (só leem o terreno e a
    // grelha estática) e em série custavam ~0,55 s cada no arranque.
    if plans.len() <= 1 {
        return plans
            .iter()
            .map(|plan| bake_field(terrain, config, plan, statics))
            .collect();
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = plans
            .iter()
            .map(|plan| scope.spawn(move || bake_field(terrain, config, plan, statics)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("bevy_ext_grass: bake de campo entrou em pânico")
            })
            .collect()
    })
}

/// Bake de um campo (alturas + máscara final).
fn bake_field(
    terrain: &dyn GrassTerrain,
    config: &GrassBakeConfig,
    plan: &FieldPlan,
    statics: &StaticMaps,
) -> FieldData {
    {
        let cell_size = config.cell.max(0.25);
        let texels = plan.texels(cell_size);
        let w = texels.x as usize;
        let h = texels.y as usize;
        let cell = plan.cell(texels);
        let (sin, cos) = plan.yaw.sin_cos();
        let local0 = 0.5 * cell - plan.size * 0.5;
        let origin = Vec2::new(
            plan.center.x + local0.x * cos + local0.y * sin,
            plan.center.y - local0.x * sin + local0.y * cos,
        );
        let step_x = Vec2::new(cell.x * cos, -cell.x * sin);
        let step_z = Vec2::new(cell.y * sin, cell.y * cos);

        let mut heights = vec![0.0f32; w * h];
        for iz in 0..h {
            let row_base = origin + step_z * iz as f32;
            let row_off = iz * w;
            for ix in 0..w {
                let world = row_base + step_x * ix as f32;
                heights[row_off + ix] = terrain.height(world.x, world.y);
            }
        }
        let mut mask = vec![0u8; w * h * 2];
        let mut nonzero = 0u32;
        let mut sum = 0f64;
        let mut hmin = f32::INFINITY;
        let mut hmax = f32::NEG_INFINITY;
        for iz in 0..h {
            let row_base = origin + step_z * iz as f32;
            let row_off = iz * w;
            for ix in 0..w {
                let idx = row_off + ix;
                let world = row_base + step_x * ix as f32;
                let Some((static_biome, static_density)) = statics.sample(world) else {
                    continue;
                };
                if static_biome != plan.biome.0 || static_density == 0 {
                    continue;
                }
                let static_density = f32::from(static_density) / 255.0;
                let y = heights[idx];
                hmin = hmin.min(y);
                hmax = hmax.max(y);
                let x0 = ix.saturating_sub(1);
                let x1 = (ix + 1).min(w - 1);
                let z0 = iz.saturating_sub(1);
                let z1 = (iz + 1).min(h - 1);
                let dx = (heights[iz * w + x1] - heights[iz * w + x0])
                    / (cell.x * (x1 - x0).max(1) as f32);
                let dz = (heights[z1 * w + ix] - heights[z0 * w + ix])
                    / (cell.y * (z1 - z0).max(1) as f32);
                let slope_deg = dx.hypot(dz).atan().to_degrees();
                let slope_fade = 1.0
                    - smoothstep(
                        config.max_slope_deg - config.slope_soft_deg,
                        config.max_slope_deg,
                        slope_deg,
                    );
                let altitude_fade =
                    1.0 - smoothstep(config.altitude_fade_start, config.altitude_fade_end, y);
                let density = static_density * slope_fade * altitude_fade;
                let byte = (density * 255.0).round().clamp(0.0, 255.0) as u8;
                mask[idx * 2] = byte;
                mask[idx * 2 + 1] = 255;
                if byte > 0 {
                    nonzero += 1;
                    sum += f64::from(byte);
                }
            }
        }
        info!(
            "bevy_ext_grass: campo {} tem {} texels, altura {:.1}..{:.1} m, densidade média {:.3} ({} finais)",
            plan.biome.0,
            w * h,
            if hmin.is_finite() { hmin } else { 0.0 },
            if hmax.is_finite() { hmax } else { 0.0 },
            if nonzero == 0 {
                0.0
            } else {
                sum / f64::from(nonzero) / 255.0
            },
            nonzero
        );
        if std::env::var("GRASS_DEBUG").as_deref() == Ok("1") {
            dump_debug(plan.biome.0, texels, &heights, &mask);
        }
        FieldData {
            plan: *plan,
            texels,
            heights,
            mask,
        }
    }
}

/// Despeja máscara final e heightmap normalizada em PGM (`GRASS_DEBUG=1`).
fn dump_debug(biome: u16, texels: UVec2, heights: &[f32], mask: &[u8]) {
    let write = |suffix: &str, data: &[u8]| {
        let mut bytes = format!("P5\n{} {}\n255\n", texels.x, texels.y).into_bytes();
        bytes.extend_from_slice(data);
        let _ = std::fs::write(
            format!("/tmp/bevy_ext_grass_biome{biome}_{suffix}.pgm"),
            bytes,
        );
    };
    let red: Vec<u8> = mask.chunks_exact(2).map(|pair| pair[0]).collect();
    write("mask", &red);
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &v in heights {
        if v.is_finite() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    let span = (hi - lo).max(1e-3);
    let grey: Vec<u8> = heights
        .iter()
        .map(|&v| {
            if v.is_finite() {
                (((v - lo) / span) * 255.0).clamp(0.0, 255.0) as u8
            } else {
                0
            }
        })
        .collect();
    write("height", &grey);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{GrassBiomeId, GrassSite};

    struct Flat {
        world: f32,
        blocked: bool,
        coverage: f32,
    }

    impl Flat {
        fn new() -> Self {
            Self {
                world: 512.0,
                blocked: false,
                coverage: 1.0,
            }
        }
    }

    impl GrassTerrain for Flat {
        fn world_size(&self) -> f32 {
            self.world
        }
        fn revision(&self) -> u64 {
            0
        }
        fn height(&self, _x: f32, _z: f32) -> f32 {
            0.0
        }
        fn site(&self, _x: f32, _z: f32, _ground: f32) -> GrassSite {
            if self.blocked {
                GrassSite::BLOCKED
            } else {
                GrassSite::grass(self.coverage, GrassBiomeId(0))
            }
        }
    }

    #[test]
    fn flat_world_grows_grass() {
        let terrain = Flat::new();
        let config = GrassBakeConfig::default();
        let plan = FieldPlan {
            biome: GrassBiomeId(0),
            center: Vec2::ZERO,
            yaw: 0.0,
            size: Vec2::splat(terrain.world),
        };
        let statics = bake_static(&terrain, &config);
        let fields = bake_dynamic(&terrain, &config, &[plan], &statics);
        assert_eq!(fields.len(), 1);
        let total = fields[0].mask.len() / 2;
        let nonzero = fields[0]
            .mask
            .chunks_exact(2)
            .filter(|pair| pair[0] > 0)
            .count();
        assert!(
            nonzero > total / 5,
            "máscara quase vazia num mundo liso: {nonzero}/{total}"
        );
    }

    #[test]
    fn blocked_terrain_has_no_grass() {
        let terrain = Flat {
            blocked: true,
            ..Flat::new()
        };
        let config = GrassBakeConfig::default();
        let plan = FieldPlan {
            biome: GrassBiomeId(0),
            center: Vec2::ZERO,
            yaw: 0.0,
            size: Vec2::splat(terrain.world),
        };
        let statics = bake_static(&terrain, &config);
        let fields = bake_dynamic(&terrain, &config, &[plan], &statics);
        assert!(fields[0].mask.iter().all(|b| *b == 0));
    }

    #[test]
    fn biome_mismatch_has_no_grass() {
        let terrain = Flat::new();
        let config = GrassBakeConfig::default();
        let plan = FieldPlan {
            biome: GrassBiomeId(7),
            center: Vec2::ZERO,
            yaw: 0.0,
            size: Vec2::splat(terrain.world),
        };
        let statics = bake_static(&terrain, &config);
        let fields = bake_dynamic(&terrain, &config, &[plan], &statics);
        assert!(fields[0].mask.iter().all(|b| *b == 0));
    }
}
