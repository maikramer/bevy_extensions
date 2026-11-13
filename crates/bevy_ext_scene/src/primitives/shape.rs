//! Authored primitive shapes (the world DSL contract).
//!
//! The render conversion lives in `bevy_ext_gradmesh`; this module is
//! render-free so the game parser, the editor and the headless tools can all
//! hold the same value type.

use bevy::prelude::*;

/// One authored primitive. Mirrors the tags the world DSL accepts.
///
/// Kept as a plain value type (no reflect/ECS) so both the engine spawn and
/// the editor preview can hold it. `Plane` has no procedural mesh (the game
/// keeps it on `StandardMaterial` for textured decals); the other variants map
/// 1:1 to a `bevy_gradmesh` shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// `Box`/`Cuboid` — half extents per axis.
    Cuboid {
        half_size: [f32; 3],
    },
    /// `Box round="0.1"` — a cuboid with rounded edges (roundness 0..=1).
    RoundedCuboid {
        half_size: [f32; 3],
        roundness: f32,
    },
    Sphere {
        radius: f32,
    },
    Cylinder {
        half_height: f32,
        radius: f32,
    },
    /// `Plane3d` — half size on the XZ plane (standard-material only).
    Plane {
        half_size: [f32; 2],
    },
    Capsule {
        radius: f32,
        half_height: f32,
    },
    /// `Cone` — `half-height` is the half of the full height.
    Cone {
        half_height: f32,
        radius: f32,
        roundness: f32,
    },
    /// `Pyramid` — `sides` edges (4 = a hipped roof).
    Pyramid {
        sides: u32,
        radius: f32,
        half_height: f32,
        roundness: f32,
    },
    /// `Torus` — ring of `radius` with tube `thickness`; angles in degrees.
    Torus {
        radius: f32,
        thickness: f32,
        start_deg: f32,
        span_deg: f32,
    },
    /// `Lathe` — `(radius, height)` profile revolved around Y.
    Lathe {
        profile: Vec<[f32; 2]>,
        segments: u32,
    },
    /// `Sweep` — a 2D profile swept along a 3D path.
    Sweep {
        profile: Vec<[f32; 2]>,
        path: Vec<[f32; 3]>,
        closed: bool,
        caps: bool,
    },
    /// `ExtrudedPolygon` — an XY polygon (with optional holes) extruded on Z.
    Extruded {
        outer: Vec<[f32; 2]>,
        holes: Vec<Vec<[f32; 2]>>,
        depth: f32,
        roundness: f32,
    },
}

impl Shape {
    /// Axis-aligned local bounds `(center, half_extents)`.
    ///
    /// Approximate for swept/lathed shapes (the preview and the fallback
    /// collider only need a conservative box).
    pub fn local_bounds(&self) -> (Vec3, Vec3) {
        match self {
            Shape::Cuboid { half_size } | Shape::RoundedCuboid { half_size, .. } => {
                (Vec3::ZERO, Vec3::from(*half_size))
            }
            Shape::Sphere { radius } => (Vec3::ZERO, Vec3::splat(*radius)),
            Shape::Cylinder {
                half_height,
                radius,
            } => (Vec3::ZERO, Vec3::new(*radius, *half_height, *radius)),
            Shape::Plane { half_size } => (Vec3::ZERO, Vec3::new(half_size[0], 0.01, half_size[1])),
            Shape::Capsule {
                radius,
                half_height,
            } => (
                Vec3::ZERO,
                Vec3::new(*radius, *half_height + *radius, *radius),
            ),
            Shape::Cone {
                half_height,
                radius,
                ..
            } => (Vec3::ZERO, Vec3::new(*radius, *half_height, *radius)),
            Shape::Pyramid {
                radius,
                half_height,
                ..
            } => (Vec3::ZERO, Vec3::new(*radius, *half_height, *radius)),
            Shape::Torus {
                radius, thickness, ..
            } => (
                Vec3::ZERO,
                Vec3::new(*radius + *thickness, *thickness, *radius + *thickness),
            ),
            Shape::Lathe { profile, .. } => {
                let mut max_r: f32 = 0.0;
                let mut min_y = f32::INFINITY;
                let mut max_y = f32::NEG_INFINITY;
                for [r, y] in profile {
                    max_r = max_r.max(r.abs());
                    min_y = min_y.min(*y);
                    max_y = max_y.max(*y);
                }
                if !min_y.is_finite() || !max_y.is_finite() {
                    return (Vec3::ZERO, Vec3::ZERO);
                }
                (
                    Vec3::new(0.0, (min_y + max_y) * 0.5, 0.0),
                    Vec3::new(max_r, (max_y - min_y) * 0.5, max_r),
                )
            }
            Shape::Sweep { profile, path, .. } => {
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for point in path {
                    let p = Vec3::from(*point);
                    min = min.min(p);
                    max = max.max(p);
                }
                if !min.is_finite() || !max.is_finite() {
                    return (Vec3::ZERO, Vec3::ZERO);
                }
                let radius = profile
                    .iter()
                    .map(|[x, y]| x.abs().max(y.abs()))
                    .fold(0.0_f32, f32::max);
                let center = (min + max) * 0.5;
                let half = (max - min) * 0.5 + Vec3::splat(radius);
                (center, half)
            }
            Shape::Extruded { outer, depth, .. } => {
                let mut min = Vec2::splat(f32::INFINITY);
                let mut max = Vec2::splat(f32::NEG_INFINITY);
                for [x, y] in outer {
                    let p = Vec2::new(*x, *y);
                    min = min.min(p);
                    max = max.max(p);
                }
                if !min.is_finite() || !max.is_finite() {
                    return (Vec3::ZERO, Vec3::ZERO);
                }
                let center = Vec3::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5, 0.0);
                let half = Vec3::new(
                    (max.x - min.x) * 0.5,
                    (max.y - min.y) * 0.5,
                    depth.abs() * 0.5,
                );
                (center, half)
            }
        }
    }
}

/// Bounds in the shape's local space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeBounds {
    pub center: Vec3,
    pub half: Vec3,
}

/// Local bounds of a primitive (see [`Shape::local_bounds`]).
pub fn bounds(shape: &Shape) -> ShapeBounds {
    let (center, half) = shape.local_bounds();
    ShapeBounds { center, half }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_match_the_authored_dimensions() {
        let (center, half) = Shape::Cuboid {
            half_size: [2.0, 0.5, 1.0],
        }
        .local_bounds();
        assert_eq!(center, Vec3::ZERO);
        assert_eq!(half, Vec3::new(2.0, 0.5, 1.0));

        let (center, half) = Shape::Lathe {
            profile: vec![[1.0, 0.0], [0.5, 2.0]],
            segments: 16,
        }
        .local_bounds();
        assert_eq!(center, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(half, Vec3::new(1.0, 1.0, 1.0));
    }

    #[test]
    fn sweep_bounds_cover_the_path_and_profile() {
        let (center, half) = Shape::Sweep {
            profile: vec![[-0.5, 0.0], [0.5, 0.0], [0.5, 0.2], [-0.5, 0.2]],
            path: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 4.0]],
            closed: false,
            caps: true,
        }
        .local_bounds();
        assert_eq!(center, Vec3::new(0.0, 0.0, 2.0));
        assert!((half.z - 2.5).abs() < 1e-6, "half.z veio {}", half.z);
    }
}
