//! Mesh generation with a baked per-object vertical gradient.
//!
//! The gradient material's own `Style::shaded` gradient is computed from a
//! per-material `world_center` uniform, which only works for objects near the
//! world origin (Vael props sit at y≈40–140 m). Baking a neutral vertical
//! factor into vertex colours gives each object the same "darker at the
//! bottom" read while keeping one shared material per style: the gradmesh
//! shader multiplies the style colour by the vertex colour.

use bevy::mesh::{Mesh, VertexAttributeValues};
use bevy_ext_scene::primitives::Shape;
use bevy_gradmesh::ShapeDetail;

use crate::shape::gradmesh_shape;

/// How dark the bottom of an object is (linear multiplier).
pub const VERTEX_GRADIENT_DARK: f32 = 0.74;

/// Builds the procedural mesh for a primitive and bakes the vertical gradient.
///
/// Returns `None` for [`Shape::Plane`] (no gradmesh equivalent; the game keeps
/// it on `StandardMaterial`).
pub fn build_mesh(shape: &Shape, detail: ShapeDetail) -> Option<Mesh> {
    let mut mesh = gradmesh_shape(shape, detail)?.build();
    bake_vertical_gradient(&mut mesh, VERTEX_GRADIENT_DARK);
    Some(mesh)
}

/// Multiplies the mesh colour by `dark` at its lowest vertex, ramping to 1.0
/// at the highest one (smoothstep in between).
///
/// The attribute is `ATTRIBUTE_COLOR` (`Float32x4`), which both the gradmesh
/// shader and Bevy's `StandardMaterial` multiply into the base colour. The
/// factor is colour-independent, so meshes stay deduplicable by shape.
pub fn bake_vertical_gradient(mesh: &mut Mesh, dark: f32) {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return;
    };
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for position in positions {
        min_y = min_y.min(position[1]);
        max_y = max_y.max(position[1]);
    }
    let span = (max_y - min_y).max(1e-4);
    let dark = dark.clamp(0.0, 1.0);
    let colors: Vec<[f32; 4]> = positions
        .iter()
        .map(|position| {
            let t = ((position[1] - min_y) / span).clamp(0.0, 1.0);
            // Smoothstep: keeps the mid-body flat and the ends readable.
            let t = t * t * (3.0 - 2.0 * t);
            let value = dark + (1.0 - dark) * t;
            [value, value, value, 1.0]
        })
        .collect();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        VertexAttributeValues::Float32x4(colors),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    #[test]
    fn gradient_goes_from_dark_to_white() {
        use bevy::asset::RenderAssetUsages;
        use bevy::mesh::PrimitiveTopology;

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, -1.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        );
        bake_vertical_gradient(&mut mesh, 0.5);
        let Some(VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("colour attribute inserted");
        };
        assert!((colors[0][0] - 0.5).abs() < 1e-6, "bottom = dark");
        assert!((colors[1][0] - 0.75).abs() < 1e-6, "middle = smoothstep");
        assert!((colors[2][0] - 1.0).abs() < 1e-6, "top = white");
    }
}
