//! Leitor do sidecar `.ahgt.paint` (VPAINT01) e a paleta canónica das
//! camadas, para previews que não podem correr o splat do runtime.
//!
//! Formato (little-endian):
//!
//! ```text
//! 0   8  magic "VPAINT01"
//! 8   4  world_size: f32
//! 12  4  side: u32 (células por lado)
//! 16  -  side² pares (slot: u8, weight: u8), linha a linha
//! ```
//!
//! A grelha é uma grelha de amostras: `step = world_size / (side - 1)` e a
//! primeira amostra fica em `-world_size/2`, tal como o heightmap e o
//! `PaintGrid` do `Viber/src/terrain/paint.rs`. `weight == 0` significa "sem
//! paint" (o slot é irrelevante); o runtime mistura
//! `(1 - w) * procedural + w * layer`, por isso o preview usa a mesma
//! fórmula com as cores `flat` de `Viber/src/terrain/layer_material.rs`.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// Magic do ficheiro de paint.
pub const PAINT_MAGIC: &[u8; 8] = b"VPAINT01";
/// Nº de camadas canónicas do splat (`DEFAULT_LAYERS` no splat do Viber).
pub const LAYER_COUNT: usize = 13;
/// Tamanho mínimo/máximo aceite para `side` (4096 é o caso do mundo do Vael).
const MIN_SIDE: usize = 2;
const MAX_SIDE: usize = 8192;
const HEADER_LEN: usize = 16;

/// Cores `flat` lineares por slot, copiadas de `SLOT_STYLES` em
/// `Viber/src/terrain/layer_material.rs` (o albedo médio já tingido).
pub const SLOT_FLAT: [[f32; 3]; LAYER_COUNT] = [
    [0.1620, 0.3050, 0.0482], // 0 grass
    [0.3231, 0.5647, 0.1221], // 1 vale_grass
    [0.1441, 0.0802, 0.0452], // 2 dirt
    [0.2961, 0.1946, 0.0802], // 3 dirt_trail
    [0.0976, 0.1441, 0.0545], // 4 forest_floor
    [0.4020, 0.3231, 0.2159], // 5 gravel
    [0.2623, 0.2542, 0.2307], // 6 mountain_stone
    [0.6867, 0.4452, 0.2086], // 7 sand
    [0.6308, 0.4125, 0.1878], // 8 desert_sand
    [0.8879, 0.9216, 0.9734], // 9 snow_peak
    [0.0612, 0.0762, 0.0343], // 10 swamp_mud
    [0.3140, 0.2384, 0.1620], // 11 dirt_road
    [0.2262, 0.2476, 0.2701], // 12 pebbles
];

/// Grelha de paint descodificada. Guarda os pares crus (2 bytes/célula); no
/// mundo do Vael são 33 MB, por isso o chamador deve libertá-la depois de
/// construir o mesh.
#[derive(Debug, Clone)]
pub struct PaintGrid {
    world_size: f32,
    side: usize,
    cells: Vec<u8>,
}

impl PaintGrid {
    /// Lê e descodifica `path`. O ficheiro é auto-contido (sem `.meta`).
    pub fn from_path(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    /// Descodifica um blob VPAINT01 em memória.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            bail!("VPAINT: truncated header");
        }
        if &bytes[0..8] != PAINT_MAGIC {
            bail!("VPAINT: bad magic");
        }
        let world_size = f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let side = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
        if !world_size.is_finite() || world_size <= 0.0 {
            bail!("VPAINT: worldSize {world_size} outside (0, inf)");
        }
        if !(MIN_SIDE..=MAX_SIDE).contains(&side) {
            bail!("VPAINT: side {side} outside [{MIN_SIDE}, {MAX_SIDE}]");
        }
        let expected = side
            .checked_mul(side)
            .and_then(|cells| cells.checked_mul(2))
            .ok_or_else(|| anyhow::anyhow!("VPAINT: {side}² overflows usize"))?;
        if bytes.len() != HEADER_LEN + expected {
            bail!(
                "VPAINT: {} bytes for a {side}² grid (expected {})",
                bytes.len(),
                HEADER_LEN + expected
            );
        }
        Ok(Self {
            world_size,
            side,
            cells: bytes[HEADER_LEN..].to_vec(),
        })
    }

    /// Lado em metros do quadrado coberto.
    pub fn world_size(&self) -> f32 {
        self.world_size
    }

    /// Nº de amostras por lado.
    pub fn side(&self) -> usize {
        self.side
    }

    /// `Some((slot, weight))` quando a célula tem paint; `weight` em `(0, 1]`.
    /// Fora dos limites devolve `None`, como o `PaintGrid::sample` do runtime.
    pub fn sample(&self, x: f32, z: f32) -> Option<(u8, f32)> {
        let half = self.world_size * 0.5;
        if x < -half || x > half || z < -half || z > half {
            return None;
        }
        let step = self.world_size / (self.side - 1) as f32;
        let i = (((x + half) / step).round() as i64).clamp(0, self.side as i64 - 1) as usize;
        let j = (((z + half) / step).round() as i64).clamp(0, self.side as i64 - 1) as usize;
        let index = 2 * (j * self.side + i);
        let slot = self.cells[index];
        let weight = self.cells[index + 1];
        (weight > 0 && (slot as usize) < LAYER_COUNT)
            .then_some((slot, weight as f32 / u8::MAX as f32))
    }

    /// Mistura a cor base com a camada pintada: `mix(base, layer, w * strength)`.
    pub fn tint(&self, x: f32, z: f32, base: [f32; 3], strength: f32) -> [f32; 3] {
        let Some((slot, weight)) = self.sample(x, z) else {
            return base;
        };
        let layer = SLOT_FLAT[slot as usize];
        let k = (weight * strength).clamp(0.0, 1.0);
        [
            base[0] + (layer[0] - base[0]) * k,
            base[1] + (layer[1] - base[1]) * k,
            base[2] + (layer[2] - base[2]) * k,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(world_size: f32, side: usize, cells: &[(u8, u8)]) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + cells.len() * 2);
        out.extend_from_slice(PAINT_MAGIC);
        out.extend_from_slice(&world_size.to_le_bytes());
        out.extend_from_slice(&(side as u32).to_le_bytes());
        for (slot, weight) in cells {
            out.push(*slot);
            out.push(*weight);
        }
        out
    }

    #[test]
    fn decodes_and_samples_the_sample_grid() {
        // 3×3, step = 10/2 = 5: centros -5, 0, +5.
        let mut cells = vec![(0, 0); 9];
        cells[4] = (9, 255); // centro = snow
        cells[0] = (11, 128); // canto (-5,-5) = dirt_road
        let grid = PaintGrid::from_bytes(&encode(10.0, 3, &cells)).expect("decode");

        assert_eq!(grid.sample(0.0, 0.0), Some((9, 1.0)));
        let (slot, weight) = grid.sample(-5.0, -5.0).expect("corner");
        assert_eq!(slot, 11);
        assert!((weight - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(grid.sample(5.0, 5.0), None);
        // Fora da grelha nunca pinta.
        assert_eq!(grid.sample(5.1, 0.0), None);
    }

    #[test]
    fn weight_zero_and_out_of_range_slots_mean_no_paint() {
        let cells = [
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (99, 255),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
        ];
        let grid = PaintGrid::from_bytes(&encode(10.0, 3, &cells)).expect("decode");
        assert_eq!(grid.sample(0.0, 0.0), None);
    }

    #[test]
    fn tint_blends_towards_the_layer() {
        let cells = [
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (9, 255),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
        ];
        let grid = PaintGrid::from_bytes(&encode(10.0, 3, &cells)).expect("decode");
        let base = [0.5, 0.5, 0.5];
        let out = grid.tint(0.0, 0.0, base, 1.0);
        assert!((out[0] - SLOT_FLAT[9][0]).abs() < 1e-6);
        assert!((out[1] - SLOT_FLAT[9][1]).abs() < 1e-6);
        assert!((out[2] - SLOT_FLAT[9][2]).abs() < 1e-6);
        // Fora da grelha mantém a base.
        assert_eq!(grid.tint(100.0, 0.0, base, 1.0), base);
    }

    #[test]
    fn rejects_bad_magic_and_truncated_payloads() {
        let mut bytes = encode(10.0, 3, &[(0, 0); 9]);
        bytes[0] = b'X';
        assert!(PaintGrid::from_bytes(&bytes).is_err());

        let bytes = encode(10.0, 3, &[(0, 0); 8]);
        assert!(PaintGrid::from_bytes(&bytes).is_err());
    }
}
