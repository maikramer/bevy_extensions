//! Espelho oceânico a cota fixa. O terreno define a praia e a batimetria.

use bevy::math::Vec2;
use bevy_ext_water::body::{WaterBody, WaterKind};

use crate::{brush::BrushGrid, water::WaterMeshData};

#[derive(Clone, Debug)]
pub struct OceanSpec {
    pub min: Vec2,
    pub max: Vec2,
    pub level: f32,
    pub color: [f32; 3],
    pub opacity: f32,
}

impl OceanSpec {
    pub fn body(&self) -> WaterBody {
        WaterBody {
            kind: WaterKind::Ocean { min: self.min, max: self.max },
            at: (self.min + self.max) * 0.5,
            radius: 0.0,
            carve_radius: 0.0,
            hull: 0.0,
            water_y: self.level,
            mirror_reach: 1.0,
            shape: Default::default(),
            stations: Vec::new(),
            surface_y: Vec::new(),
            water_width: 0.0,
            half_width: Vec::new(),
            depths: Vec::new(),
            cascades: Vec::new(),
            section: Default::default(),
        }
    }

    /// Até à borda do terreno, a malha amostra o fundo. Fora dele, reduz
    /// a densidade dos vértices e prolonga a mesma lâmina mar adentro.
    pub(crate) fn mesh(&self, grid: &BrushGrid) -> WaterMeshData {
        let half = grid.world_size() * 0.5;
        let xs = axis_samples(self.min.x, self.max.x, half);
        let zs = axis_samples(self.min.y, self.max.y, half);
        let mut data = WaterMeshData::default();
        for &z in &zs {
            for &x in &xs {
                let depth = if x.abs() <= half && z.abs() <= half {
                    (self.level - grid.sample(x, z)).clamp(0.02, 40.0)
                } else {
                    35.0
                };
                data.mesh.positions.push([x, self.level, z]);
                data.mesh.normals.push([0.0, 1.0, 0.0]);
                data.mesh.uvs.push([self.opacity, depth]);
                data.mesh.colors.push([self.color[0], self.color[1], self.color[2], 1.0]);
                data.uv_b.push([0.0, 0.0]);
            }
        }
        let row = xs.len() as u32;
        for z in 0..zs.len() as u32 - 1 {
            for x in 0..row - 1 {
                let a = z * row + x;
                let b = a + 1;
                let c = a + row;
                let d = c + 1;
                data.mesh.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        data
    }
}

/// Uma superfície opaca sob o mar profundo dá profundidade ao céu e às nuvens
/// mesmo onde já não há terreno. Fica abaixo das ondas e fora das praias,
/// que continuam a usar a profundidade do fundo verdadeiro.
pub(crate) fn deep_water_underlay(data: &WaterMeshData) -> crate::mesh::ChunkMeshData {
    let mut mesh = data.mesh.clone();
    mesh.indices = data
        .mesh
        .indices
        .chunks_exact(3)
        .filter(|triangle| triangle.iter().all(|&i| mesh.uvs[i as usize][1] >= 5.0))
        .flatten()
        .copied()
        .collect();
    for position in &mut mesh.positions {
        position[1] -= 0.30;
    }
    mesh
}

fn axis_samples(min: f32, max: f32, half: f32) -> Vec<f32> {
    let mut axis = vec![min];
    for (lo, hi, spacing) in [
        (min, max.min(-half), 320.0),
        (min.max(-half), max.min(half), 14.0),
        (min.max(half), max, 320.0),
    ] {
        if hi <= lo {
            continue;
        }
        let n = ((hi - lo) / spacing).ceil().max(1.0) as u32;
        for i in 1..=n {
            axis.push(lo + (hi - lo) * (i as f32 / n as f32));
        }
    }
    axis
}
