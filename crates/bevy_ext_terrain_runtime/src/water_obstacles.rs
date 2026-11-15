//! Obstáculos de corrente do terreno: conversão de colliders avian (trimesh/compound) e sólidos
//! voxel para os primitivos 2D da crate `bevy_ext_water`.
//!
//! A matemática (SDF, deflexão do fluxo, halo/esteira) vive na crate — esta
//! camada só conhece o motor de física e o runtime do terreno deste jogo,
//! que a crate de propósito não conhece.

pub use bevy_ext_water::obstacles::*;

use bevy::prelude::*;
use avian3d::parry::shape::TypedShape;
use avian3d::prelude::Collider;

use super::mesh::HeightField as _;
use super::runtime::TerrainRuntime;

pub fn water_surface_range_in_aabb(
    rt: &TerrainRuntime,
    min_xz: Vec2,
    max_xz: Vec2,
) -> Option<(f32, f32)> {
    if rt.water.is_empty() {
        return None;
    }
    let span = (max_xz - min_xz).max(Vec2::ZERO);
    let nx = ((span.x / 2.2).ceil() as usize).clamp(1, 5);
    let nz = ((span.y / 2.2).ceil() as usize).clamp(1, 5);

    let mut min_wy = f32::INFINITY;
    let mut max_wy = f32::NEG_INFINITY;
    let mut hit = false;

    let pad = 0.35;
    let lo = min_xz - Vec2::splat(pad);
    let hi = max_xz + Vec2::splat(pad);

    for iz in 0..=nz {
        let fz = iz as f32 / nz as f32;
        let z = lo.y + (hi.y - lo.y) * fz;
        for ix in 0..=nx {
            let fx = ix as f32 / nx as f32;
            let x = lo.x + (hi.x - lo.x) * fx;
            if let Some(wy) = rt.water_surface_at(x, z) {
                min_wy = min_wy.min(wy);
                max_wy = max_wy.max(wy);
                hit = true;
            }
        }
    }
    hit.then_some((min_wy, max_wy))
}

/// Extracts all horizontal waterline obstacle footprints from a Rapier `Collider`
/// positioned at world transform `iso` (com a escala já dobrada na `shape_scaled` do avian).
pub fn collect_collider_water_obstacles(
    collider: &Collider,
    iso: Transform,
    water_min_y: f32,
    water_max_y: f32,
    out: &mut Vec<WaterObstacle>,
) {
    let y_lo = water_min_y - OBSTACLE_SLICE_BELOW_M;
    let y_hi = water_max_y + OBSTACLE_SLICE_ABOVE_M;
    let ref_wy = (water_min_y + water_max_y) * 0.5;
    collect_shape_obstacles(collider.shape_scaled().as_typed_shape(), iso, y_lo, y_hi, ref_wy, out);
}

fn collect_shape_obstacles(
    shape: TypedShape<'_>,
    iso: Transform,
    y_lo: f32,
    y_hi: f32,
    ref_wy: f32,
    out: &mut Vec<WaterObstacle>,
) {
    match shape {
        TypedShape::Compound(compound) => {
            for (sub_iso, sub_shape) in compound.shapes() {
                let local_tf = Transform {
                    translation: sub_iso.translation,
                    rotation: sub_iso.rotation,
                    scale: Vec3::ONE,
                };
                collect_shape_obstacles(
                    sub_shape.as_typed_shape(),
                    iso * local_tf,
                    y_lo,
                    y_hi,
                    ref_wy,
                    out,
                );
            }
        }
        TypedShape::Ball(ball) => {
            let c = iso.translation;
            let r = ball.radius;
            if r < MIN_OBSTACLE_RADIUS_M || c.y + r < y_lo || c.y - r > y_hi {
                return;
            }
            let dy = (ref_wy - c.y).abs();
            let slice_r = if dy < r {
                (r * r - dy * dy).sqrt()
            } else {
                r * 0.65
            };
            if slice_r >= MIN_OBSTACLE_RADIUS_M {
                out.push(WaterObstacle::circle(c.xz(), slice_r));
            }
        }
        TypedShape::Cylinder(cyl) => {
            let axis_y = (iso.rotation * Vec3::Y).normalize_or(Vec3::Y);
            let h = cyl.half_height;
            let r = cyl.radius;
            if r < MIN_OBSTACLE_RADIUS_M {
                return;
            }
            let a = iso.translation - axis_y * h;
            let b = iso.translation + axis_y * h;
            let min_y = a.y.min(b.y) - r * (1.0 - axis_y.y.abs());
            let max_y = a.y.max(b.y) + r * (1.0 - axis_y.y.abs());
            if max_y < y_lo || min_y > y_hi {
                return;
            }
            if axis_y.y.abs() > 0.85 {
                out.push(WaterObstacle::circle(iso.translation.xz(), r));
            } else if let Some((ca, cb)) = clip_segment_to_y_slab(a, b, y_lo - r, y_hi + r) {
                out.push(WaterObstacle::capsule(ca.xz(), cb.xz(), r));
            }
        }
        TypedShape::Capsule(cap) => {
            let a = iso.transform_point(Vec3::new(cap.segment.a.x, cap.segment.a.y, cap.segment.a.z));
            let b = iso.transform_point(Vec3::new(cap.segment.b.x, cap.segment.b.y, cap.segment.b.z));
            let r = cap.radius;
            if r < MIN_OBSTACLE_RADIUS_M {
                return;
            }
            if let Some((ca, cb)) = clip_segment_to_y_slab(a, b, y_lo - r, y_hi + r) {
                out.push(WaterObstacle::capsule(ca.xz(), cb.xz(), r));
            }
        }
        TypedShape::Cuboid(cuboid) => {
            let he = Vec3::new(cuboid.half_extents.x, cuboid.half_extents.y, cuboid.half_extents.z);
            collect_cuboid_obstacle(iso, he, y_lo, y_hi, out);
        }
        TypedShape::ConvexPolyhedron(poly) => {
            let pts: Vec<Vec3> = poly
                .points()
                .iter()
                .map(|p| iso.transform_point(Vec3::new(p.x, p.y, p.z)))
                .collect();
            collect_convex_points_obstacle(&pts, y_lo, y_hi, ref_wy, out);
        }
        TypedShape::TriMesh(trimesh) => {
            let verts = trimesh.vertices();
            let tris: Vec<[Vec3; 3]> = trimesh
                .indices()
                .iter()
                .filter_map(|idx| {
                    let a = verts.get(idx[0] as usize)?;
                    let b = verts.get(idx[1] as usize)?;
                    let c = verts.get(idx[2] as usize)?;
                    let wa = iso.transform_point(Vec3::new(a.x, a.y, a.z));
                    let wb = iso.transform_point(Vec3::new(b.x, b.y, b.z));
                    let wc = iso.transform_point(Vec3::new(c.x, c.y, c.z));
                    let tmin = wa.y.min(wb.y).min(wc.y);
                    let tmax = wa.y.max(wb.y).max(wc.y);
                    (tmax >= y_lo && tmin <= y_hi).then_some([wa, wb, wc])
                })
                .collect();
            if !tris.is_empty() {
                collect_trimesh_slab_clusters(&tris, ref_wy, out);
            }
        }
        _ => {}
    }
}

fn collect_cuboid_obstacle(
    iso: Transform,
    he: Vec3,
    y_lo: f32,
    y_hi: f32,
    out: &mut Vec<WaterObstacle>,
) {
    let ax = iso.rotation * Vec3::X;
    let ay = iso.rotation * Vec3::Y;
    let az = iso.rotation * Vec3::Z;
    let world_half_y = ax.y.abs() * he.x + ay.y.abs() * he.y + az.y.abs() * he.z;
    let cy = iso.translation.y;
    if cy + world_half_y < y_lo || cy - world_half_y > y_hi {
        return;
    }

    if ay.y.abs() > 0.85 {
        let right = ax.xz().normalize_or(Vec2::X);
        let forward = az.xz().normalize_or(Vec2::Y);
        if he.x.max(he.z) >= MIN_OBSTACLE_RADIUS_M {
            out.push(WaterObstacle::oriented_box(
                iso.translation.xz(),
                right,
                forward,
                Vec2::new(he.x, he.z),
            ));
        }
        return;
    }

    let corners: [Vec3; 8] = [
        iso.transform_point(Vec3::new(-he.x, -he.y, -he.z)),
        iso.transform_point(Vec3::new(he.x, -he.y, -he.z)),
        iso.transform_point(Vec3::new(he.x, -he.y, he.z)),
        iso.transform_point(Vec3::new(-he.x, -he.y, he.z)),
        iso.transform_point(Vec3::new(-he.x, he.y, -he.z)),
        iso.transform_point(Vec3::new(he.x, he.y, -he.z)),
        iso.transform_point(Vec3::new(he.x, he.y, he.z)),
        iso.transform_point(Vec3::new(-he.x, he.y, he.z)),
    ];
    let ref_wy = (y_lo + OBSTACLE_SLICE_BELOW_M + y_hi - OBSTACLE_SLICE_ABOVE_M) * 0.5;
    collect_convex_points_obstacle(&corners, y_lo, y_hi, ref_wy, out);
}

fn collect_convex_points_obstacle(
    pts: &[Vec3],
    y_lo: f32,
    y_hi: f32,
    ref_wy: f32,
    out: &mut Vec<WaterObstacle>,
) {
    if pts.is_empty() {
        return;
    }
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for &p in pts {
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    if max_y < y_lo || min_y > y_hi {
        return;
    }

    let slice_y = ref_wy.clamp(min_y + 1e-3, max_y - 1e-3);
    let mut xz_pts = Vec::new();

    if min_y < slice_y && max_y > slice_y && pts.len() <= 48 {
        for &a in pts {
            if a.y > slice_y {
                continue;
            }
            for &b in pts {
                if b.y <= slice_y {
                    continue;
                }
                let t = ((slice_y - a.y) / (b.y - a.y)).clamp(0.0, 1.0);
                xz_pts.push(a.lerp(b, t).xz());
            }
        }
    }
    if xz_pts.len() < 3 {
        for &p in pts {
            if p.y >= y_lo && p.y <= y_hi {
                xz_pts.push(p.xz());
            }
        }
    }
    if xz_pts.len() < 3 {
        for &p in pts {
            xz_pts.push(p.xz());
        }
    }

    let hull = convex_hull_2d(&mut xz_pts);
    if hull.len() >= 3 {
        let mut segs = Vec::with_capacity(hull.len());
        for i in 0..hull.len() {
            segs.push([hull[i], hull[(i + 1) % hull.len()]]);
        }
        let obs = WaterObstacle::slice_contour(segs, true, 0.0);
        if obs.radius >= MIN_OBSTACLE_RADIUS_M {
            out.push(obs);
        }
    }
}

fn collect_trimesh_slab_clusters(
    tris: &[[Vec3; 3]],
    ref_wy: f32,
    out: &mut Vec<WaterObstacle>,
) {
    // Cluster slab triangles by touching/overlapping XZ AABBs (expanded 0.35 m) so
    // separate bridge piers in one mesh remain independent water obstacles.
    const LINK_GAP: f32 = 0.35;
    let n = tris.len();
    let mut aabbs = Vec::with_capacity(n);
    for &[a, b, c] in tris {
        let min = a.xz().min(b.xz()).min(c.xz()) - Vec2::splat(LINK_GAP);
        let max = a.xz().max(b.xz()).max(c.xz()) + Vec2::splat(LINK_GAP);
        aabbs.push((min, max));
    }

    let mut visited = vec![false; n];
    let mut cluster = Vec::new();
    let mut stack = Vec::new();

    for start in 0..n {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        cluster.clear();
        stack.clear();
        stack.push(start);

        while let Some(cur) = stack.pop() {
            cluster.push(tris[cur]);
            let (cmin, cmax) = aabbs[cur];
            for next in 0..n {
                if !visited[next] {
                    let (nmin, nmax) = aabbs[next];
                    if cmax.x >= nmin.x && cmin.x <= nmax.x && cmax.y >= nmin.y && cmin.y <= nmax.y
                    {
                        visited[next] = true;
                        stack.push(next);
                    }
                }
            }
        }

        if let Some(obs) = build_obstacle_from_tri_cluster(&cluster, ref_wy) {
            if obs.radius >= MIN_OBSTACLE_RADIUS_M {
                out.push(obs);
            }
        }
    }
}

fn build_obstacle_from_tri_cluster(cluster: &[[Vec3; 3]], ref_wy: f32) -> Option<WaterObstacle> {
    const SLICE_OFFSETS: [f32; 4] = [0.0, -0.22, -0.44, 0.22];
    for &dy in &SLICE_OFFSETS {
        let y = ref_wy + dy + 1.19e-4;
        let mut segs = Vec::new();
        for &[a, b, c] in cluster {
            if let Some(seg) = slice_triangle_at_y(a, b, c, y) {
                segs.push(seg);
            }
        }
        if !segs.is_empty() {
            let closed = segs_are_closed_contour(&segs);
            return Some(WaterObstacle::slice_contour(segs, closed, 0.16));
        }
    }

    let mut xz_pts = Vec::with_capacity(cluster.len() * 3);
    for &[a, b, c] in cluster {
        xz_pts.push(a.xz());
        xz_pts.push(b.xz());
        xz_pts.push(c.xz());
    }
    let hull = convex_hull_2d(&mut xz_pts);
    if hull.len() >= 3 {
        let mut segs = Vec::with_capacity(hull.len());
        for i in 0..hull.len() {
            segs.push([hull[i], hull[(i + 1) % hull.len()]]);
        }
        return Some(WaterObstacle::slice_contour(segs, true, 0.0));
    }
    None
}

fn slice_triangle_at_y(a: Vec3, b: Vec3, c: Vec3, y: f32) -> Option<[Vec2; 2]> {
    let above = [a.y > y, b.y > y, c.y > y];
    let count = above.iter().filter(|&&v| v).count();
    if count == 0 || count == 3 {
        return None;
    }
    let edges = [(a, b), (b, c), (c, a)];
    let mut hits = [Vec2::ZERO; 2];
    let mut n = 0;
    for (p0, p1) in edges {
        if (p0.y > y) != (p1.y > y) {
            let t = ((y - p0.y) / (p1.y - p0.y)).clamp(0.0, 1.0);
            hits[n] = p0.lerp(p1, t).xz();
            n += 1;
            if n == 2 {
                break;
            }
        }
    }
    (n == 2 && hits[0].distance_squared(hits[1]) > 1e-8).then_some(hits)
}

fn segs_are_closed_contour(segs: &[[Vec2; 2]]) -> bool {
    if segs.len() < 3 {
        return false;
    }
    let mut degrees = std::collections::HashMap::<(i64, i64), u32>::with_capacity(segs.len() * 2);
    for &[a, b] in segs {
        for p in [a, b] {
            let key = ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64);
            *degrees.entry(key).or_default() += 1;
        }
    }
    degrees.values().all(|&deg| deg % 2 == 0)
}

fn clip_segment_to_y_slab(a: Vec3, b: Vec3, y_lo: f32, y_hi: f32) -> Option<(Vec3, Vec3)> {
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    let dy = b.y - a.y;
    if dy.abs() < 1e-6 {
        if a.y < y_lo || a.y > y_hi {
            return None;
        }
        return Some((a, b));
    }
    let inv_dy = 1.0 / dy;
    let s0 = (y_lo - a.y) * inv_dy;
    let s1 = (y_hi - a.y) * inv_dy;
    t0 = t0.max(s0.min(s1));
    t1 = t1.min(s0.max(s1));
    (t0 <= t1).then(|| (a.lerp(b, t0), a.lerp(b, t1)))
}

fn convex_hull_2d(pts: &mut Vec<Vec2>) -> Vec<Vec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then_with(|| a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| a.distance_squared(*b) < 1e-6);
    if pts.len() < 3 {
        return Vec::new();
    }

    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut hull = Vec::with_capacity(pts.len() * 2);

    for &p in pts.iter() {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 1e-6 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower_len = hull.len() + 1;
    for &p in pts.iter().rev() {
        while hull.len() >= lower_len
            && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 1e-6
        {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

/// Computes a tight world-space AABB for a local AABB under rigid `iso`.
pub fn collect_voxel_water_obstacles(
    rt: &TerrainRuntime,
    min_xz: Vec2,
    max_xz: Vec2,
    out: &mut Vec<WaterObstacle>,
) {
    if rt.voxel.is_flat() || rt.water.is_empty() {
        return;
    }
    const STEP: f32 = 1.0;
    let base = rt.base();
    let nx = ((max_xz.x - min_xz.x) / STEP).ceil().max(1.0) as i32;
    let nz = ((max_xz.y - min_xz.y) / STEP).ceil().max(1.0) as i32;

    for iz in 0..=nz {
        let wz = min_xz.y + iz as f32 * STEP;
        for ix in 0..=nx {
            let wx = min_xz.x + ix as f32 * STEP;
            let Some(wy) = rt.water_surface_at(wx, wz) else {
                continue;
            };
            // Only CSG solids protruding out of an actually submerged base bed
            // (distinguishes river/lake bridge piers from natural riverbanks).
            if rt.in_field(wx, wz) && base.sample(wx, wz) >= wy - 0.25 {
                continue;
            }
            if rt.voxel.density(&base, Vec3::new(wx, wy, wz)) < 0.0 {
                out.push(WaterObstacle::oriented_box(
                    Vec2::new(wx, wz),
                    Vec2::X,
                    Vec2::Y,
                    Vec2::splat(STEP * 0.52),
                ));
            }
        }
    }
}
