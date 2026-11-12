//! Chunk mesh buffers and the height/slope tint shared by the terrain and
//! voxel meshers.

/// Vertex/index buffers for one terrain chunk mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkMeshData {
    /// Vertex positions, world-origin relative to the chunk entity
    /// (x/z relative to the chunk center, y absolute meters).
    pub positions: Vec<[f32; 3]>,
    /// Smooth normals.
    pub normals: Vec<[f32; 3]>,
    /// UVs (world-space tiled, see the voxel mesh builder).
    pub uvs: Vec<[f32; 2]>,
    /// RGBA vertex colors from the height/slope tint.
    pub colors: Vec<[f32; 4]>,
    /// Triangle indices.
    pub indices: Vec<u32>,
}

/// Height/slope banding parameters for [`tint_vertex_color`] (the render-side
/// subset of the host's terrain tint).
#[derive(Debug, Clone, PartialEq)]
pub struct TintParams {
    pub base_color: [f32; 4],
    pub color_low: [f32; 4],
    pub color_mid: [f32; 4],
    pub color_high: [f32; 4],
    pub color_rock: [f32; 4],
    pub snow_height: f32,
    pub slope_threshold: f32,
    pub slope_softness: f32,
    /// `0` disables the banding entirely: every vertex gets `base_color`, so
    /// the surface is the material texture times the authored base colour and
    /// nothing else.
    pub height_blend_strength: f32,
}

impl Default for TintParams {
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            color_low: [1.0; 4],
            color_mid: [1.0; 4],
            color_high: [1.0; 4],
            color_rock: [1.0; 4],
            snow_height: 1.0,
            slope_threshold: 1.0,
            slope_softness: 0.05,
            height_blend_strength: 0.0,
        }
    }
}

/// Neutral the rock tint is mixed against: white leaves the diffuse texture
/// untouched on flat ground and at `height-blend-strength: 0`.
const NEUTRAL_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// `mix(white, band, height_blend_strength) * base_color`. The surface also
/// carries a diffuse texture, and the shader multiplies the two; the mix makes
/// `height-blend-strength` mean "how far towards the authored band colour",
/// with `0` leaving the texture times `base-color`.
pub fn tint_vertex_color(y: f32, normal_y: f32, max_height: f32, tint: &TintParams) -> [f32; 4] {
    // Altitude não pinta. `y` / `max_height` ficam na assinatura porque o
    // mesher voxel e o heightfield chamam os dois do mesmo modo.
    let _ = (y, max_height);
    let strength = tint.height_blend_strength.clamp(0.0, 1.0);
    if strength <= 0.0 {
        return tint.base_color;
    }
    // Slope: 0 flat, 1 vertical. Rocha só na parede — o chão plano fica
    // a textura, seja ela pintada ou a paleta do bioma.
    let slope = 1.0 - normal_y.clamp(0.0, 1.0);
    let rock_band = smoothstep(
        tint.slope_threshold - tint.slope_softness,
        tint.slope_threshold + tint.slope_softness,
        slope,
    );
    let color = mix4(NEUTRAL_COLOR, tint.color_rock, rock_band * strength);
    [
        color[0] * tint.base_color[0],
        color[1] * tint.base_color[1],
        color[2] * tint.base_color[2],
        color[3] * tint.base_color[3],
    ]
}

/// GLSL-style `smoothstep(edge0, edge1, x)` with a guard for degenerate edges
/// (`edge1 <= edge0` collapses to a hard step instead of dividing by zero).
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear blend of two RGBA colors: `a` at `t = 0`, `b` at `t = 1`.
fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tint_ignores_altitude() {
        let tint = TintParams {
            base_color: [1.0; 4],
            color_low: [0.04, 0.09, 0.02, 1.0],
            color_mid: [0.5, 0.5, 0.5, 1.0],
            color_high: [0.9, 0.9, 1.0, 1.0],
            color_rock: [0.2, 0.18, 0.15, 1.0],
            snow_height: 0.92,
            slope_threshold: 0.93,
            slope_softness: 0.05,
            height_blend_strength: 1.0,
        };
        let valley = tint_vertex_color(10.0, 1.0, 200.0, &tint);
        let peak = tint_vertex_color(199.0, 1.0, 200.0, &tint);
        assert_eq!(valley, peak);
    }
}
