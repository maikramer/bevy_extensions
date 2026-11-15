//! Exact Avian colliders for the procedural primitives.
//!
//! The simple shapes use the analytic Avian primitives; pyramids, lathes,
//! sweeps, extruded polygons and tori bake a trimesh from the SAME mesh the
//! game renders. `Plane` returns `None` (the engine/game keeps its thin plane
//! collider). Enabled by the `physics` feature; the editor preview does not
//! need physics.

use avian3d::prelude::{Collider, TrimeshFlags};
use bevy::mesh::{Indices, Mesh, VertexAttributeValues};
use bevy::prelude::*;
use bevy_ext_scene::primitives::{Shape, bounds};
use bevy_gradmesh::ShapeDetail;

use crate::mesh::build_mesh;

/// Builds the exact collider for a primitive, or `None` for `Plane`.
///
/// `scale` is applied to the local geometry (the production spawn passes
/// `Vec3::ONE`; the engine also applies the entity transform).
pub fn collider(shape: &Shape, scale: Vec3) -> Option<Collider> {
    let scale = Vec3::new(scale.x.abs(), scale.y.abs(), scale.z.abs());
    Some(match shape {
        Shape::Plane { .. } => return None,
        // O `cuboid` do Avian leva extensões COMPLETAS (o Rapier levava meias).
        Shape::Cuboid { half_size } | Shape::RoundedCuboid { half_size, .. } => Collider::cuboid(
            (half_size[0] * scale.x * 2.0).max(1e-3),
            (half_size[1] * scale.y * 2.0).max(1e-3),
            (half_size[2] * scale.z * 2.0).max(1e-3),
        ),
        Shape::Sphere { radius } => Collider::sphere((radius * scale.max_element()).max(1e-3)),
        Shape::Cylinder {
            half_height,
            radius,
        } => Collider::cylinder(
            (radius * scale.x.max(scale.z)).max(1e-3),
            (half_height * scale.y * 2.0).max(1e-3),
        ),
        Shape::Capsule {
            radius,
            half_height,
        } => Collider::capsule(
            (radius * scale.x.max(scale.z)).max(1e-3),
            (half_height * scale.y * 2.0).max(1e-3),
        ),
        Shape::Cone {
            half_height,
            radius,
            ..
        } => Collider::cone(
            (radius * scale.x.max(scale.z)).max(1e-3),
            (half_height * scale.y * 2.0).max(1e-3),
        ),
        Shape::Pyramid { .. }
        | Shape::Lathe { .. }
        | Shape::Sweep { .. }
        | Shape::Extruded { .. }
        | Shape::Torus { .. } => procedural_collider(shape, scale),
    })
}

fn procedural_collider(shape: &Shape, scale: Vec3) -> Collider {
    let fallback = || {
        let half = bounds(shape).half;
        Collider::cuboid(
            (half.x * scale.x * 2.0).max(1e-3),
            (half.y * scale.y * 2.0).max(1e-3),
            (half.z * scale.z * 2.0).max(1e-3),
        )
    };
    let Some(mut mesh) = build_mesh(shape, ShapeDetail::Medium) else {
        return fallback();
    };
    if scale != Vec3::ONE {
        scale_mesh_positions(&mut mesh, scale);
    }
    let Some((vertices, indices)) = mesh_vertices_indices(&mesh) else {
        return fallback();
    };
    Collider::try_trimesh_with_config(
        vertices,
        indices,
        TrimeshFlags::FIX_INTERNAL_EDGES | TrimeshFlags::DELETE_DEGENERATE_TRIANGLES,
    )
    .unwrap_or_else(|_| fallback())
}

fn scale_mesh_positions(mesh: &mut Mesh, scale: Vec3) {
    if let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        for position in positions {
            position[0] *= scale.x;
            position[1] *= scale.y;
            position[2] *= scale.z;
        }
    }
}

fn mesh_vertices_indices(mesh: &Mesh) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };
    let vertices: Vec<Vec3> = positions.iter().map(|p| Vec3::from(*p)).collect();
    let indices: Vec<[u32; 3]> = match mesh.indices()? {
        Indices::U16(values) => values
            .chunks_exact(3)
            .map(|chunk| [chunk[0] as u32, chunk[1] as u32, chunk[2] as u32])
            .collect(),
        Indices::U32(values) => values
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect(),
    };
    (!indices.is_empty()).then_some((vertices, indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analytic_shapes_keep_their_collider() {
        let cuboid = Shape::Cuboid {
            half_size: [1.0, 0.5, 2.0],
        };
        let cuboid_collider = collider(&cuboid, Vec3::ONE).expect("cuboid collider");
        assert!(cuboid_collider.shape_scaled().as_cuboid().is_some());

        let cone = Shape::Cone {
            half_height: 1.0,
            radius: 0.6,
            roundness: 0.0,
        };
        assert!(collider(&cone, Vec3::ONE).is_some());
    }

    #[test]
    fn procedural_shapes_bake_a_trimesh() {
        let lathe = Shape::Lathe {
            profile: vec![[0.6, 0.0], [0.5, 1.0], [0.4, 2.0]],
            segments: 16,
        };
        let trimesh = collider(&lathe, Vec3::ONE).expect("lathe collider");
        assert!(
            trimesh.shape_scaled().as_trimesh().is_some(),
            "lathe → exact trimesh"
        );
    }

    #[test]
    fn plane_has_no_procedural_collider() {
        assert!(
            collider(
                &Shape::Plane {
                    half_size: [1.0, 1.0]
                },
                Vec3::ONE
            )
            .is_none()
        );
    }
}
