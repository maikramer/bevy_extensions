//! Authored primitives → `bevy_gradmesh` geometry.

use bevy::prelude::*;
use bevy_ext_scene::primitives::Shape;
use bevy_gradmesh::{Shape as GradShape, ShapeDetail};

/// Converts the authored primitive into `bevy_gradmesh` geometry.
///
/// `Plane` returns `None` on purpose: the crate has no plane and the game
/// keeps textured decals on `StandardMaterial`.
pub fn gradmesh_shape(shape: &Shape, detail: ShapeDetail) -> Option<GradShape> {
    Some(match shape {
        Shape::Cuboid { half_size } => GradShape::cuboid(Vec3::from(*half_size) * 2.0, 0.0, detail),
        Shape::RoundedCuboid {
            half_size,
            roundness,
        } => GradShape::cuboid(
            Vec3::from(*half_size) * 2.0,
            roundness.clamp(0.0, 1.0),
            detail,
        ),
        Shape::Sphere { radius } => GradShape::sphere(*radius, detail),
        Shape::Cylinder {
            half_height,
            radius,
        } => GradShape::cylinder(*radius, *half_height * 2.0, detail),
        Shape::Plane { .. } => return None,
        Shape::Capsule {
            radius,
            half_height,
        } => GradShape::capsule(
            *radius,
            // `half_height` is the cylindrical section (Rapier/Capsule3d);
            // gradmesh wants the total height, caps included.
            *half_height * 2.0 + *radius * 2.0,
            detail,
        ),
        Shape::Cone {
            half_height,
            radius,
            roundness,
        } => GradShape::rounded_cone(
            *radius,
            *half_height * 2.0,
            roundness.clamp(0.0, 1.0),
            detail,
        ),
        Shape::Pyramid {
            sides,
            radius,
            half_height,
            roundness,
        } => GradShape::rounded_pyramid(
            (*sides).max(3),
            *radius,
            *half_height * 2.0,
            roundness.clamp(0.0, 1.0),
            detail,
        ),
        Shape::Torus {
            radius,
            thickness,
            start_deg,
            span_deg,
        } => GradShape::torus(
            *radius,
            *thickness,
            start_deg.to_radians(),
            span_deg.to_radians(),
            detail,
        ),
        Shape::Lathe { profile, segments } => GradShape::lathe(
            profile.iter().map(|[r, y]| Vec2::new(*r, *y)).collect(),
            (*segments).max(3),
            detail,
        ),
        Shape::Sweep {
            profile,
            path,
            closed,
            caps,
        } => GradShape::sweep(
            profile.iter().map(|[x, y]| Vec2::new(*x, *y)).collect(),
            path.iter().map(|p| Vec3::from(*p)).collect(),
            bevy_gradmesh::SweepOptions::default()
                .with_closed(*closed)
                .with_caps(*caps)
                .with_detail(detail),
        ),
        Shape::Extruded {
            outer,
            holes,
            depth,
            roundness,
        } => GradShape::extruded_polygon_with_holes(
            outer.iter().map(|[x, y]| Vec2::new(*x, *y)).collect(),
            holes
                .iter()
                .map(|hole| hole.iter().map(|[x, y]| Vec2::new(*x, *y)).collect())
                .collect(),
            *depth,
            roundness.clamp(0.0, 1.0),
            detail,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_keeps_the_total_height() {
        let shape = Shape::Capsule {
            radius: 0.5,
            half_height: 1.0,
        };
        let grad = gradmesh_shape(&shape, ShapeDetail::Low).expect("capsule builds");
        let size = grad.bounds().size();
        assert!((size.y - 3.0).abs() < 1e-3, "altura veio {}", size.y);
    }

    #[test]
    fn plane_has_no_gradmesh_equivalent() {
        assert!(
            gradmesh_shape(
                &Shape::Plane {
                    half_size: [1.0, 1.0]
                },
                ShapeDetail::Low
            )
            .is_none()
        );
    }

    #[test]
    fn procedural_shapes_build_geometry() {
        let lathe = Shape::Lathe {
            profile: vec![[0.5, 0.0], [0.4, 1.0], [0.3, 2.0]],
            segments: 16,
        };
        let grad = gradmesh_shape(&lathe, ShapeDetail::Low).expect("lathe builds");
        let size = grad.bounds().size();
        assert!((size.y - 2.0).abs() < 1e-3);
    }
}
