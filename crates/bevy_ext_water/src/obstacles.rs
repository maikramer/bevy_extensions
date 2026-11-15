//! Obstáculos 2D do escoamento (rochas, pilares, pontes): SDF, deflexão da
//! corrente e halo/esteira de espuma.
//!
//! A CONVERSÃO de colliders 3D para estes primitivos é do host (Rapier,
//! avian, voxel ou outra coisa): esta crate só recebe [`WaterObstacle`]s já
//! cortados no plano da lâmina pelo chamador. Assim a água não conhece
//! nenhum motor de física concreto.

use std::hash::{Hash, Hasher};

use bevy::prelude::*;

/// Vertical slice below local `water_y` included as current-blocking obstacles
/// (captures submerged boulders close enough to deflect surface currents).
pub const OBSTACLE_SLICE_BELOW_M: f32 = 0.60;

/// Vertical slice above local `water_y` included as obstacles (kept below
/// overhead bridge decks and arches so vessels/currents pass underneath).
pub const OBSTACLE_SLICE_ABOVE_M: f32 = 0.55;

/// Signed-distance halo width (m) encoded into `obstacle_tex.r`.
pub const OBSTACLE_HALO_M: f32 = 4.0;

/// Minimum effective obstacle half-size (m); smaller pebbles are ignored.
pub const MIN_OBSTACLE_RADIUS_M: f32 = 0.14;

/// Upper bound used to normalize effective obstacle size & wake scaling.
pub const MAX_OBSTACLE_RADIUS_M: f32 = 16.0;

/// 2D horizontal obstacle representation at water level.
#[derive(Debug, Clone)]
pub enum WaterObstacleKind {
    Circle {
        center: Vec2,
        radius: f32,
    },
    Capsule {
        a: Vec2,
        b: Vec2,
        radius: f32,
    },
    OrientedBox {
        center: Vec2,
        right: Vec2,
        forward: Vec2,
        half_extents: Vec2,
    },
    SliceContour {
        segments: Vec<[Vec2; 2]>,
        closed: bool,
        wall_half_thick: f32,
    },
}

/// Extracted obstacle footprint in the water plane with bounding data for
/// fast spatial culling during rasterization and gameplay current queries.
#[derive(Debug, Clone)]
pub struct WaterObstacle {
    pub kind: WaterObstacleKind,
    pub aabb_min: Vec2,
    pub aabb_max: Vec2,
    pub center: Vec2,
    pub radius: f32,
}

/// Evaluation of an obstacle signed distance field at one XZ coordinate.
#[derive(Debug, Clone, Copy)]
pub struct ObstacleSdfSample {
    /// Signed distance in meters (`< 0` inside obstacle, `> 0` outside).
    pub signed_dist: f32,
    /// Unit outward normal pointing away from the obstacle core in XZ.
    pub normal: Vec2,
    /// Characteristic horizontal half-size of the obstacle (m).
    pub radius: f32,
    /// Obstacle center in XZ.
    pub center: Vec2,
}

impl WaterObstacle {
    /// Constructs a circular obstacle in XZ.
    pub fn circle(center: Vec2, radius: f32) -> Self {
        let r = radius.max(0.05);
        Self {
            kind: WaterObstacleKind::Circle { center, radius: r },
            aabb_min: center - Vec2::splat(r),
            aabb_max: center + Vec2::splat(r),
            center,
            radius: r,
        }
    }

    /// Constructs a 2D capsule obstacle in XZ (fallen logs, horizontal beams).
    pub fn capsule(a: Vec2, b: Vec2, radius: f32) -> Self {
        let r = radius.max(0.05);
        let center = (a + b) * 0.5;
        let span = a.distance(b) * 0.5 + r;
        Self {
            kind: WaterObstacleKind::Capsule { a, b, radius: r },
            aabb_min: a.min(b) - Vec2::splat(r),
            aabb_max: a.max(b) + Vec2::splat(r),
            center,
            radius: span.clamp(r, MAX_OBSTACLE_RADIUS_M),
        }
    }

    /// Constructs an oriented box obstacle in XZ.
    pub fn oriented_box(center: Vec2, right: Vec2, forward: Vec2, half_extents: Vec2) -> Self {
        let r_dir = right.normalize_or(Vec2::X);
        let f_dir = forward.normalize_or(Vec2::Y);
        let hx = half_extents.x.max(0.05);
        let hz = half_extents.y.max(0.05);
        let extent_x = r_dir.x.abs() * hx + f_dir.x.abs() * hz;
        let extent_z = r_dir.y.abs() * hx + f_dir.y.abs() * hz;
        let eff_r = (hx * hz).sqrt().max(hx.min(hz) * 0.85).clamp(0.05, MAX_OBSTACLE_RADIUS_M);
        Self {
            kind: WaterObstacleKind::OrientedBox {
                center,
                right: r_dir,
                forward: f_dir,
                half_extents: Vec2::new(hx, hz),
            },
            aabb_min: center - Vec2::new(extent_x, extent_z),
            aabb_max: center + Vec2::new(extent_x, extent_z),
            center,
            radius: eff_r,
        }
    }

    /// Constructs a 2D contour slice obstacle from horizontal boundary segments.
    pub fn slice_contour(segments: Vec<[Vec2; 2]>, closed: bool, wall_half_thick: f32) -> Self {
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        for [a, b] in &segments {
            min = min.min(*a).min(*b);
            max = max.max(*a).max(*b);
        }
        if !min.is_finite() {
            min = Vec2::ZERO;
            max = Vec2::ZERO;
        }
        let pad = if closed { 0.0 } else { wall_half_thick.max(0.05) };
        let center = (min + max) * 0.5;
        let half = (max - min) * 0.5 + Vec2::splat(pad);
        let eff_r = half.length().mul_add(0.65, 0.0).clamp(0.05, MAX_OBSTACLE_RADIUS_M);
        Self {
            kind: WaterObstacleKind::SliceContour {
                segments,
                closed,
                wall_half_thick: wall_half_thick.max(0.05),
            },
            aabb_min: min - Vec2::splat(pad),
            aabb_max: max + Vec2::splat(pad),
            center,
            radius: eff_r,
        }
    }

    /// Evaluates the exact signed distance and outward unit normal at `p` in XZ.
    pub fn eval(&self, p: Vec2) -> ObstacleSdfSample {
        let (signed_dist, normal) = match &self.kind {
            WaterObstacleKind::Circle { center, radius } => {
                let d = p - *center;
                let len = d.length();
                let n = if len > 1e-5 { d / len } else { Vec2::X };
                (len - *radius, n)
            }
            WaterObstacleKind::Capsule { a, b, radius } => {
                let ab = *b - *a;
                let len2 = ab.length_squared();
                let t = if len2 > 1e-6 {
                    ((p - *a).dot(ab) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let closest = *a + ab * t;
                let d = p - closest;
                let len = d.length();
                let n = if len > 1e-5 {
                    d / len
                } else {
                    Vec2::new(-ab.y, ab.x).normalize_or(Vec2::X)
                };
                (len - *radius, n)
            }
            WaterObstacleKind::OrientedBox {
                center,
                right,
                forward,
                half_extents,
            } => eval_oriented_box(p, *center, *right, *forward, *half_extents),
            WaterObstacleKind::SliceContour {
                segments,
                closed,
                wall_half_thick,
            } => eval_slice_contour(p, self.center, segments, *closed, *wall_half_thick),
        };
        ObstacleSdfSample {
            signed_dist,
            normal,
            radius: self.radius,
            center: self.center,
        }
    }

    /// Maximum horizontal distance around the AABB where currents can be
    /// disturbed (upstream stagnation, lateral deflection, downstream wake).
    #[inline]
    pub fn wake_reach_m(&self) -> f32 {
        (self.radius * 4.5 + 3.0).clamp(4.0, 22.0)
    }
}

fn eval_oriented_box(
    p: Vec2,
    center: Vec2,
    right: Vec2,
    forward: Vec2,
    half_extents: Vec2,
) -> (f32, Vec2) {
    let rel = p - center;
    let local = Vec2::new(rel.dot(right), rel.dot(forward));
    let q = local.abs() - half_extents;
    let sx = if local.x >= 0.0 { 1.0 } else { -1.0 };
    let sy = if local.y >= 0.0 { 1.0 } else { -1.0 };

    let (sd, local_n) = if q.x > 0.0 || q.y > 0.0 {
        let out_q = q.max(Vec2::ZERO);
        let d = out_q.length();
        let ln = if d > 1e-5 {
            Vec2::new(out_q.x * sx, out_q.y * sy) / d
        } else if q.x > q.y {
            Vec2::new(sx, 0.0)
        } else {
            Vec2::new(0.0, sy)
        };
        (d, ln)
    } else {
        let d = q.x.max(q.y);
        let ln = if q.x > q.y {
            Vec2::new(sx, 0.0)
        } else {
            Vec2::new(0.0, sy)
        };
        (d, ln)
    };
    let normal = (right * local_n.x + forward * local_n.y).normalize_or(Vec2::X);
    (sd, normal)
}

fn eval_slice_contour(
    p: Vec2,
    center: Vec2,
    segments: &[[Vec2; 2]],
    closed: bool,
    wall_half_thick: f32,
) -> (f32, Vec2) {
    let mut best_d2 = f32::INFINITY;
    let mut best_closest = p;
    let mut best_seg_n = Vec2::X;

    for &[a, b] in segments {
        let ab = b - a;
        let len2 = ab.length_squared();
        let t = if len2 > 1e-8 {
            ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let c = a + ab * t;
        let d2 = p.distance_squared(c);
        if d2 < best_d2 {
            best_d2 = d2;
            best_closest = c;
            let perp = Vec2::new(-ab.y, ab.x).normalize_or(Vec2::X);
            best_seg_n = if (c - center).dot(perp) >= 0.0 {
                perp
            } else {
                -perp
            };
        }
    }

    let dist = best_d2.sqrt();
    if !closed {
        let n = if dist > 1e-4 {
            (p - best_closest) / dist
        } else {
            best_seg_n
        };
        return (dist - wall_half_thick, n);
    }

    let inside = point_inside_closed_segments(p, segments);
    let sd = if inside { -dist } else { dist };
    let n = if dist > 1e-4 {
        let dir = (p - best_closest) / dist;
        if inside { -dir } else { dir }
    } else {
        best_seg_n
    };
    (sd, n)
}

/// Four-ray crossing parity test with irrational jitter so open sheets never
/// invert outside space while watertight slices are exact.
fn point_inside_closed_segments(p: Vec2, segments: &[[Vec2; 2]]) -> bool {
    let probe_y = p.y + 1.37e-4;
    let mut pos_x = 0u32;
    let mut neg_x = 0u32;
    for &[a, b] in segments {
        if (a.y > probe_y) != (b.y > probe_y) {
            let t = (probe_y - a.y) / (b.y - a.y);
            let hit_x = a.x + (b.x - a.x) * t;
            if hit_x >= p.x {
                pos_x += 1;
            } else {
                neg_x += 1;
            }
        }
    }
    if (pos_x & 1) == (neg_x & 1) {
        return (pos_x & 1) == 1;
    }

    let probe_x = p.x + 1.13e-4;
    let mut pos_z = 0u32;
    let mut neg_z = 0u32;
    for &[a, b] in segments {
        if (a.x > probe_x) != (b.x > probe_x) {
            let t = (probe_x - a.x) / (b.x - a.x);
            let hit_z = a.y + (b.y - a.y) * t;
            if hit_z >= p.y {
                pos_z += 1;
            } else {
                neg_z += 1;
            }
        }
    }
    (pos_z & 1) == 1 && (neg_z & 1) == 1
}

/// Evaluates the union (minimum signed distance) over all obstacles overlapping `p`.
pub fn nearest_obstacle_at(p: Vec2, obstacles: &[WaterObstacle]) -> Option<ObstacleSdfSample> {
    let mut best: Option<ObstacleSdfSample> = None;
    for obs in obstacles {
        if p.x < obs.aabb_min.x - OBSTACLE_HALO_M
            || p.x > obs.aabb_max.x + OBSTACLE_HALO_M
            || p.y < obs.aabb_min.y - OBSTACLE_HALO_M
            || p.y > obs.aabb_max.y + OBSTACLE_HALO_M
        {
            continue;
        }
        let sample = obs.eval(p);
        if sample.signed_dist < OBSTACLE_HALO_M
            && best.is_none_or(|b| sample.signed_dist < b.signed_dist)
        {
            best = Some(sample);
        }
    }
    best
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < 1e-6 {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Deflects base stream velocity around obstructing boulders, bridge piers, and
/// submerged walls:
/// - **Upstream face**: cancels inward normal component and redirects tangentially
///   around left/right flanks with stagnation slowdown right at the nose.
/// - **Lateral shoulders**: smooth Venturi acceleration squeezing past flanks.
/// - **Downstream lee**: sheltered calm pocket & mild inward-curling return eddies.
pub fn deflect_flow_by_obstacles(
    p: Vec2,
    base_flow: Vec2,
    obstacles: &[WaterObstacle],
) -> Vec2 {
    let speed = base_flow.length();
    if speed < 1e-4 || obstacles.is_empty() {
        return base_flow;
    }

    let u = base_flow / speed;
    let perp_u = Vec2::new(-u.y, u.x);

    let mut cur_flow = base_flow;
    let mut best_wake_shelter = 1.0_f32;
    let mut wake_curling = Vec2::ZERO;

    for obs in obstacles {
        let reach = obs.wake_reach_m();
        if p.x < obs.aabb_min.x - reach
            || p.x > obs.aabb_max.x + reach
            || p.y < obs.aabb_min.y - reach
            || p.y > obs.aabb_max.y + reach
        {
            continue;
        }

        let s = obs.eval(p);
        let r = s.radius.max(0.2);

        // 1. Impermeable boundary layer deflection & Venturi flank acceleration.
        let deflect_range = (r * 1.65).clamp(0.85, 3.6);
        if s.signed_dist < deflect_range {
            if s.signed_dist <= -0.15 {
                cur_flow = Vec2::ZERO;
            } else {
                let inside_damp = smoothstep(-0.15, 0.08, s.signed_dist);
                let bw = smoothstep(deflect_range, 0.0, s.signed_dist);
                let facing = u.dot(s.normal);
                let vn = cur_flow.dot(s.normal);

                // Cancel flow driving into solid upstream face and divert around.
                if vn < 0.0 {
                    cur_flow -= s.normal * (vn * bw);
                }

                // Stagnation point at head-on nose; tangential Venturi speedup on shoulders.
                let rel_lat = (p - obs.center).dot(perp_u);
                let tan_a = Vec2::new(-s.normal.y, s.normal.x);
                let mut tangent = if cur_flow.dot(tan_a) >= 0.0 {
                    tan_a
                } else {
                    -tan_a
                };
                if cur_flow.dot(tangent).abs() < 0.08 * speed {
                    let side_sign = if rel_lat >= 0.0 { 1.0 } else { -1.0 };
                    let split = perp_u * side_sign;
                    tangent = if tan_a.dot(split) >= 0.0 {
                        tan_a
                    } else {
                        -tan_a
                    };
                }

                let nose_stag = smoothstep(-0.65, -0.96, facing) * smoothstep(r * 0.9, 0.0, s.signed_dist);
                let shoulder = (1.0 - facing * facing).max(0.0)
                    * (-s.signed_dist.max(0.0) / (0.65 * r + 0.30)).exp();
                cur_flow *= (1.0 - 0.58 * nose_stag) * inside_damp;
                cur_flow += tangent * (speed * shoulder * 0.42 * inside_damp);
            }
        }

        // 2. Downstream sheltered wake & gentle inward-curling Kármán vortices.
        let rel = p - obs.center;
        let along = rel.dot(u);
        let lat = rel.dot(perp_u);
        let wake_len = (r * 4.5 + 1.5).clamp(2.5, 16.0);
        if along > r * 0.15 && along < wake_len {
            let wake_half_w = r * (1.08 + 0.28 * (along / wake_len));
            let abs_lat = lat.abs();
            if abs_lat < wake_half_w * 1.4 {
                let along_w = smoothstep(wake_len, r * 0.35, along)
                    * smoothstep(r * 0.15, r * 0.85, along);
                let core_w = smoothstep(wake_half_w * 1.15, r * 0.25, abs_lat);
                let shelter = 1.0 - 0.72 * along_w * core_w;
                best_wake_shelter = best_wake_shelter.min(shelter);

                // Shear inward swirl at rear flanks + subtle return eddy right behind rock.
                let lat_sign = if lat >= 0.0 { 1.0 } else { -1.0 };
                let rim_w = smoothstep(r * 0.2, r * 0.85, abs_lat)
                    * smoothstep(wake_half_w * 1.35, r * 0.75, abs_lat);
                wake_curling -= perp_u * (lat_sign * rim_w * along_w * speed * 0.24);

                let reverse_w = smoothstep(r * 1.6, r * 0.35, along)
                    * smoothstep(r * 0.75, 0.0, abs_lat);
                wake_curling -= u * (reverse_w * speed * 0.18);
            }
        }
    }

    (cur_flow * best_wake_shelter + wake_curling).clamp_length_max(speed * 1.55)
}

/// Finds overlapping water heights near an XZ AABB by probing its interior and
/// perimeter against `rt.water_surface_at`, returning `(min_water_y, max_water_y)`.

pub fn rotated_aabb(iso: &Transform, local_min: Vec3, local_max: Vec3) -> (Vec3, Vec3) {
    let local_center = (local_min + local_max) * 0.5;
    let local_half = (local_max - local_min) * 0.5;
    let mat = Mat3::from_quat(iso.rotation);
    let abs_half = Vec3::new(
        mat.x_axis.x.abs() * local_half.x
            + mat.y_axis.x.abs() * local_half.y
            + mat.z_axis.x.abs() * local_half.z,
        mat.x_axis.y.abs() * local_half.x
            + mat.y_axis.y.abs() * local_half.y
            + mat.z_axis.y.abs() * local_half.z,
        mat.x_axis.z.abs() * local_half.x
            + mat.y_axis.z.abs() * local_half.y
            + mat.z_axis.z.abs() * local_half.z,
    );
    let world_center = iso.transform_point(local_center);
    (world_center - abs_half, world_center + abs_half)
}

/// Computes a fast structural hash over active obstacles inside/near the wave
/// window so breaking/moving rocks triggers immediate `obstacle_tex` re-bake.
pub fn obstacles_signature(obstacles: &[WaterObstacle]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    obstacles.len().hash(&mut h);
    for obs in obstacles {
        obs.center.x.to_bits().hash(&mut h);
        obs.center.y.to_bits().hash(&mut h);
        obs.radius.to_bits().hash(&mut h);
        obs.aabb_min.x.to_bits().hash(&mut h);
        obs.aabb_max.y.to_bits().hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_and_oriented_box_sdf_and_normals_match_geometry() {
        let c = WaterObstacle::circle(Vec2::new(4.0, -2.0), 1.5);
        let inside = c.eval(Vec2::new(4.5, -2.0));
        assert!((inside.signed_dist + 1.0).abs() < 1e-4);
        assert!(inside.normal.x > 0.99);

        let b = WaterObstacle::oriented_box(
            Vec2::ZERO,
            Vec2::new(0.0, 1.0),
            Vec2::new(-1.0, 0.0),
            Vec2::new(2.0, 0.5),
        );
        // Point 1.0 m outside along rotated right axis (0, +1).
        let side = b.eval(Vec2::new(0.0, 3.0));
        assert!((side.signed_dist - 1.0).abs() < 1e-4, "sd={}", side.signed_dist);
        assert!(side.normal.y > 0.99);
    }

    #[test]
    fn deflect_flow_stagnates_upstream_speeds_shoulders_and_shelters_wake() {
        let rock = WaterObstacle::circle(Vec2::ZERO, 1.5);
        let base_flow = Vec2::new(2.0, 0.0);

        // Directly ahead of upstream nose: velocity must slow down and lose inward thrust.
        let nose = deflect_flow_by_obstacles(Vec2::new(-1.65, 0.05), base_flow, &[rock.clone()]);
        assert!(
            nose.x < base_flow.x * 0.65,
            "expected stagnation at nose, got {nose:?}"
        );

        // Lateral shoulder tangent: water squeezes past with tangential boost.
        let shoulder = deflect_flow_by_obstacles(Vec2::new(0.0, 1.7), base_flow, &[rock.clone()]);
        assert!(
            shoulder.x > base_flow.x * 1.05,
            "expected Venturi flank boost, got {shoulder:?}"
        );

        // Calm sheltered pocket behind rock.
        let wake = deflect_flow_by_obstacles(Vec2::new(3.2, 0.0), base_flow, &[rock.clone()]);
        assert!(
            wake.length() < base_flow.length() * 0.55,
            "expected sheltered wake behind boulder, got {wake:?}"
        );

        // Far outside rock wake reach: unchanged.
        let far = deflect_flow_by_obstacles(Vec2::new(0.0, 12.0), base_flow, &[rock]);
        assert!((far - base_flow).length() < 1e-4);
    }
}
