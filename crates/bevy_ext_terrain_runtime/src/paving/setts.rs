//! A crowned stone street. Union the runs before meshing so even a crossing
//! halfway along a run has one bed, one draining shoulder and one outer kerb.
use super::*;
use bevy::math::Vec3;

type Key = (i32, i32);
type Field = BTreeMap<Key, f32>;

fn point((x, z): Key, step: f32) -> Vec2 {
    Vec2::new(x as f32, z as f32) * step
}

fn stamp(field: &mut Field, run: &PavingCorridor, step: f32, margin: f32) {
    // Take the nearest segment within each arm first. Softening every
    // overlapping segment would make the width depend on station spacing.
    let mut arm = Field::new();
    for (i, segment) in run.stations.windows(2).enumerate() {
        let (a, b) = (segment[0], segment[1]);
        let wa = run.half_width.get(i).copied().unwrap_or(1.0);
        let wb = run.half_width.get(i + 1).copied().unwrap_or(wa);
        let reach = Vec2::splat(wa.max(wb) + margin);
        let lo = ((a.min(b) - reach) / step).floor().as_ivec2();
        let hi = ((a.max(b) + reach) / step).ceil().as_ivec2();
        let ab = b - a;
        for z in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let p = point((x, z), step);
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.0, 1.0);
                let depth = wa + (wb - wa) * t - p.distance(a + ab * t);
                if depth >= -margin {
                    let entry = arm.entry((x, z)).or_insert(depth);
                    *entry = entry.max(depth);
                }
            }
        }
    }
    for (key, depth) in arm {
        field
            .entry(key)
            .and_modify(|old| {
                // A compact smooth union rounds the inside of a junction, while
                // leaving the width and camber of an isolated approach untouched.
                *old = join(*old, depth);
            })
            .or_insert(depth);
    }
}

fn join(a: f32, b: f32) -> f32 {
    let radius = 2.2;
    let overlap = (1.0 - (a - b).abs() / radius).max(0.0);
    a.max(b) + radius * overlap * overlap * 0.25
}

fn bed(depth: f32) -> f32 {
    -0.038 + 0.041 * smootherstep01((depth / 2.3).clamp(0.0, 1.0))
}

fn allowed(grid: &PavingGrid, gid: u16, p: Vec2) -> bool {
    let cell = grid.cell_index(p);
    !grid.area_cells.contains(&cell)
        && grid.cells.get(&cell).is_none_or(|&g| {
            let owner = &grid.groups[g as usize];
            g == gid || owner.setts || owner.trail
        })
}

// Integer millimetres weld the same interpolated crossing from either
// triangle, including opposite interpolation directions.
fn mm(p: Vec2) -> Key {
    let p = (p * 1000.0).round().as_ivec2();
    (p.x, p.y)
}

type Contour = BTreeMap<Key, BTreeSet<Key>>;

fn contour_triangle(out: &mut Contour, corners: [(Vec2, f32); 3]) {
    let mut cuts = [Vec2::ZERO; 2];
    let mut n = 0;
    for i in 0..3 {
        let (a, da) = corners[i];
        let (b, db) = corners[(i + 1) % 3];
        if (da >= 0.0) != (db >= 0.0) {
            cuts[n] = a.lerp(b, da / (da - db));
            n += 1;
        }
    }
    if n == 2 {
        let (a, b) = (mm(cuts[0]), mm(cuts[1]));
        if a != b {
            out.entry(a).or_default().insert(b);
            out.entry(b).or_default().insert(a);
        }
    }
}

fn kerbstone(
    mesh: &mut ChunkMeshData,
    terrain: &BrushGrid,
    group: &PavingGroup,
    a: Vec2,
    b: Vec2,
    side_a: Vec2,
    side_b: Vec2,
    seed: u32,
) {
    let tangent = (b - a).normalize_or_zero();
    let a = a + tangent * 0.012;
    let b = b - tangent * 0.012;
    let w = 0.115 + 0.014 * hash01(seed);
    let foot = [
        a - side_a * w,
        b - side_b * w,
        b + side_b * w,
        a + side_a * w,
    ];
    let bevel = 0.032;
    let lid = [
        a + tangent * bevel - side_a * (w - bevel),
        b - tangent * bevel - side_b * (w - bevel),
        b - tangent * bevel + side_b * (w - bevel),
        a + tangent * bevel + side_a * (w - bevel),
    ];
    let tint = 1.02 + 0.12 * hash01(seed.rotate_left(13));
    let top = group.lift + 0.001 + 0.004 * hash01(!seed);
    let stone_uv = Vec2::new(hash01(seed), hash01(seed.rotate_left(7)));
    let mut face = |points: [Vec2; 4], heights: [f32; 4], shade: f32| {
        let mut idx = [0u32; 4];
        for i in 0..4 {
            let p = points[i];
            idx[i] = push_vertex(mesh, terrain, p.x, p.y, heights[i], 1.0, tint * shade, 1.0);
            // One large piece of stone, with a different cut of the texture on each block.
            mesh.uvs[idx[i] as usize] = (stone_uv + (p - a) * 0.64).to_array();
        }
        let [v0, v1, v2, _] = idx.map(|i| Vec3::from_array(mesh.positions[i as usize]));
        let mut normal = (v1 - v0).cross(v2 - v0).normalize_or_zero();
        if normal.y < 0.0 {
            normal = -normal;
        }
        for i in idx {
            mesh.normals[i as usize] = normal.to_array();
        }
        strip(mesh, [idx[0], idx[1]], [idx[3], idx[2]]);
    };
    for i in 0..4 {
        let j = (i + 1) % 4;
        face(
            [foot[i], foot[j], lid[j], lid[i]],
            [group.lift - 0.036, group.lift - 0.036, top, top],
            0.92,
        );
    }
    face(lid, [top; 4], 1.0);
}

fn kerbs(
    mut edges: Contour,
    terrain: &BrushGrid,
    grid: &PavingGrid,
    gid: u16,
    meshes: &mut BTreeMap<(u16, i32, i32), ChunkMeshData>,
) {
    let group = &grid.groups[gid as usize];
    // Start open chains at an end (where another material clipped the road).
    let ends: Vec<_> = edges
        .iter()
        .filter(|(_, v)| v.len() == 1)
        .map(|(&p, _)| p)
        .collect();
    let mut open = ends.into_iter();
    while !edges.is_empty() {
        let start = open
            .find(|p| edges.contains_key(p))
            .unwrap_or_else(|| *edges.first_key_value().unwrap().0);
        let mut p = start;
        let mut line = vec![point(p, 0.001)];
        while let Some(q) = edges.get(&p).and_then(|v| v.first().copied()) {
            for (a, b) in [(p, q), (q, p)] {
                let links = edges.get_mut(&a).unwrap();
                links.remove(&b);
                if links.is_empty() {
                    edges.remove(&a);
                }
            }
            line.push(point(q, 0.001));
            p = q;
            if p == start {
                break;
            }
        }
        let mut lengths = vec![0.0];
        for pair in line.windows(2) {
            lengths.push(lengths.last().unwrap() + pair[0].distance(pair[1]));
        }
        let total = *lengths.last().unwrap();
        if total < 0.45 {
            continue;
        }
        let blocks = (total / 0.82).round().max(1.0) as usize;
        let at = |d: f32| {
            let dist = if p == start {
                d.rem_euclid(total)
            } else {
                d.clamp(0.0, total)
            };
            let i = lengths
                .partition_point(|&l| l < dist)
                .clamp(1, line.len() - 1);
            line[i - 1].lerp(
                line[i],
                (dist - lengths[i - 1]) / (lengths[i] - lengths[i - 1]),
            )
        };
        let side = |d| (at(d + 0.16) - at(d - 0.16)).normalize_or_zero().perp();
        for i in 0..blocks {
            let d0 = total * i as f32 / blocks as f32;
            let d1 = total * (i + 1) as f32 / blocks as f32;
            let (a, b) = (at(d0), at(d1));
            let mid = (a + b) * 0.5;
            if a.distance(b) < 0.28 || !allowed(grid, gid, mid) {
                continue;
            }
            let mesh = meshes
                .entry(chunk_key(group, grid.kerb_groups[&gid], mid))
                .or_default();
            let key = mm(mid);
            kerbstone(
                mesh,
                terrain,
                group,
                a,
                b,
                side(d0),
                side(d1),
                group.seed ^ (key.0 as u32).wrapping_mul(31) ^ key.1 as u32,
            );
        }
    }
}

// Each point has one elevation and outside edge. The mix only controls which
// material shades it. Stone reaches a little farther into a trail at the mouth.
struct SurfacePoint {
    depth: f32,
    layers: Vec<(u16, f32)>,
    alpha: f32,
    lift: f32,
}

impl SurfacePoint {
    fn resolve(&mut self, grid: &PavingGrid, p: Vec2) {
        let mut peak = f32::NEG_INFINITY;
        for (gid, score) in &mut self.layers {
            if grid.groups[*gid as usize].setts {
                *score += 0.65;
            }
            peak = peak.max(*score);
        }
        let mut sum = 0.0;
        self.layers.retain_mut(|(_, score)| {
            *score = smootherstep01((1.0 + (*score - peak) / 1.6).clamp(0.0, 1.0));
            sum += *score;
            *score > 0.0
        });
        let mut feather = 0.0;
        for (gid, weight) in &mut self.layers {
            *weight /= sum;
            let g = &grid.groups[*gid as usize];
            feather += *weight * feather_width(g, p);
            self.lift += *weight * g.lift;
        }
        self.alpha = smootherstep01((1.0 + self.depth / feather.max(0.001)).clamp(0.0, 1.0));
    }
}

fn surface(grid: &PavingGrid, step: f32) -> BTreeMap<Key, SurfacePoint> {
    let mut layers = BTreeMap::new();
    for run in &grid.corridors {
        let g = &grid.groups[run.group as usize];
        if g.setts || g.trail {
            stamp(
                layers.entry(run.group).or_default(),
                run,
                step,
                g.feather * 1.5 + 2.2 + step * 2.0,
            );
        }
    }
    let mut surface = BTreeMap::new();
    for (gid, field) in layers {
        for (key, depth) in field {
            let v = surface.entry(key).or_insert_with(|| SurfacePoint {
                depth,
                layers: Vec::new(),
                alpha: 0.0,
                lift: 0.0,
            });
            if !v.layers.is_empty() {
                v.depth = join(v.depth, depth);
            }
            v.layers.push((gid, depth));
        }
    }
    for (&key, v) in &mut surface {
        v.resolve(grid, point(key, step));
    }
    surface
}

pub(super) fn emit(
    terrain: &BrushGrid,
    grid: &PavingGrid,
    pieces: &mut Vec<((u16, i32, i32), ChunkMeshData)>,
) {
    let step = grid.module / (grid.module / 0.48).ceil().max(1.0);
    let field = surface(grid, step);
    let empty = SurfacePoint {
        depth: -100.0,
        layers: Vec::new(),
        alpha: 0.0,
        lift: 0.0,
    };
    let mut meshes = BTreeMap::new();
    let mut contours: BTreeMap<u16, Contour> = BTreeMap::new();
    let mut vertices = BTreeMap::new();
    for &(x, z) in field.keys() {
        let keys = [(x, z), (x + 1, z), (x + 1, z + 1), (x, z + 1)];
        let samples = keys.map(|k| field.get(&k).unwrap_or(&empty));
        if samples.iter().all(|s| s.alpha == 0.0) {
            continue;
        }
        let points = keys.map(|k| point(k, step));
        let center = (points[0] + points[2]) * 0.5;
        let mut groups = Vec::new();
        for sample in samples {
            for &(gid, _) in &sample.layers {
                if !groups.contains(&gid) {
                    groups.push(gid);
                }
            }
        }
        groups.sort_by_key(|&gid| grid.material_order(gid));
        if !allowed(grid, groups[0], center) {
            continue;
        }

        // Composite the material transition on identical triangles. The base
        // fills the bed. Only the narrow transition emits a second material.
        let mut remaining = [1.0_f32; 4];
        for (layer, &gid) in groups.iter().enumerate().rev() {
            let group = &grid.groups[gid as usize];
            let bucket = chunk_key(group, gid, center);
            let mesh = meshes.entry(bucket).or_insert_with(ChunkMeshData::default);
            let mut idx = [0u32; 4];
            for (i, &(u, v)) in keys.iter().enumerate() {
                let sample = samples[i];
                let weight = sample
                    .layers
                    .iter()
                    .find(|(g, _)| *g == gid)
                    .map_or(0.0, |s| s.1);
                let amount = sample.alpha * weight;
                let alpha = if layer == 0 && sample.alpha >= 1.0 {
                    1.0
                } else {
                    (amount / remaining[i].max(1e-6_f32)).clamp(0.0, 1.0)
                };
                remaining[i] -= amount;
                idx[i] = *vertices
                    .entry((bucket, u, v, alpha.to_bits()))
                    .or_insert_with(|| {
                        let d = sample.depth;
                        let tint = 0.84 + 0.16 * smootherstep01((d / 1.25).clamp(0.0, 1.0));
                        let id = push_vertex(
                            mesh,
                            terrain,
                            points[i].x,
                            points[i].y,
                            sample.lift + bed(d),
                            group.texture_scale.max(0.01),
                            tint,
                            alpha,
                        );
                        let h = |k| bed(field.get(&k).map_or(d, |v| v.depth));
                        let n = Vec3::from_array(mesh.normals[id as usize]);
                        let grad = Vec3::new(
                            h((u + 1, v)) - h((u - 1, v)),
                            0.0,
                            h((u, v + 1)) - h((u, v - 1)),
                        ) / (2.0 * step);
                        mesh.normals[id as usize] = (n - grad * n.y).normalize().to_array();
                        id
                    });
            }
            strip(mesh, [idx[0], idx[1]], [idx[3], idx[2]]);
        }
        for tri in [[0, 1, 2], [0, 2, 3]] {
            let owner = groups
                .iter()
                .max_by(|a, b| {
                    let total = |gid: u16| {
                        tri.iter()
                            .flat_map(|&i| &samples[i].layers)
                            .filter(|(g, _)| *g == gid)
                            .map(|s| s.1)
                            .sum::<f32>()
                    };
                    total(**a).total_cmp(&total(**b))
                })
                .unwrap();
            if grid.groups[*owner as usize].setts {
                contour_triangle(
                    contours.entry(*owner).or_default(),
                    tri.map(|i| (points[i], samples[i].depth - 0.06)),
                );
            }
        }
    }
    for (gid, contour) in contours {
        kerbs(contour, terrain, grid, gid, &mut meshes);
    }
    pieces.extend(meshes);
}
