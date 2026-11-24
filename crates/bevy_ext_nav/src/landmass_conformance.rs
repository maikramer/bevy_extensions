//! Tests: every tile this crate bakes is a mesh landmass accepts.
//!
//! `landmass` validates a navmesh as a whole (convex polygons, each edge shared
//! by at most two polygons, a consistent height mesh) and refuses it
//! otherwise. The generator has no such contract, so the contract is ours: the
//! worlds below imitate what `backend::nav_backend` hands rerecast — a ground
//! lattice with holes for water, cliffs and the ocean, boxes and compound walls
//! as obstacles, roads painted as convex areas — and each baked tile has to go
//! through [`super::landmass_mesh`].

use super::*;

use bevy::math::{Quat, U16Vec3, UVec3, Vec2, Vec3A, Vec3Swizzles};
use bevy_rerecast::rerecast as rc;
use bevy_rerecast::rerecast::{AreaType, ConvexVolume};

// ── gerador determinístico (o mundo de uma semente é sempre o mesmo) ──────

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    fn u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f32 {
        (self.u64() >> 40) as f32 * (1.0 / 16777216.0)
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

fn hash2(x: i32, z: i32, seed: u32) -> f32 {
    let h = (x as u32)
        .wrapping_mul(374761393)
        .wrapping_add((z as u32).wrapping_mul(668265263))
        ^ seed;
    let h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn value_noise(x: f32, z: f32, seed: u32) -> f32 {
    let (xi, zi) = (x.floor(), z.floor());
    let (tx, tz) = (smooth(x - xi), smooth(z - zi));
    let (xi, zi) = (xi as i32, zi as i32);
    let a = hash2(xi, zi, seed);
    let b = hash2(xi + 1, zi, seed);
    let c = hash2(xi, zi + 1, seed);
    let d = hash2(xi + 1, zi + 1, seed);
    (a * (1.0 - tx) + b * tx) * (1.0 - tz) + (c * (1.0 - tx) + d * tx) * tz
}

/// Um mundo sintético com a forma do que o `nav_backend` entrega ao rerecast.
struct World {
    seed: u32,
    terrace: f32,
    lakes: Vec<(Vec3, f32)>,
    river: Option<(Vec<Vec3>, f32)>,
    cliff_seed: u32,
    cliff_threshold: f32,
    ocean_z: f32,
    roads: Vec<(Vec<Vec3>, f32)>,
    /// (centro, meia-extensão, yaw)
    obstacles: Vec<(Vec3, Vec3, f32)>,
}

impl World {
    fn terrain(&self, x: f32, z: f32) -> f32 {
        let h = (value_noise(x * 0.045, z * 0.045, self.seed) - 0.5) * 14.0
            + (value_noise(x * 0.21, z * 0.21, self.seed ^ 0x55) - 0.5) * 1.4;
        if self.terrace > 0.0 {
            (h / self.terrace).round() * self.terrace
        } else {
            h
        }
    }

    fn in_water(&self, p: Vec3) -> bool {
        self.lakes
            .iter()
            .any(|&(c, r)| (p.xz() - c.xz()).length() <= r)
            || self.river.as_ref().is_some_and(|(stations, r)| {
                stations.windows(2).any(|pair| {
                    let (a, b) = (pair[0].xz(), pair[1].xz());
                    let ab = b - a;
                    let t = ((p.xz() - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                    (p.xz() - (a + ab * t)).length() <= *r
                })
            })
    }

    fn is_cliff(&self, x: f32, z: f32) -> bool {
        value_noise(x * 0.06, z * 0.06, self.cliff_seed) > self.cliff_threshold
    }

    fn ocean_at(&self, p: Vec3) -> bool {
        p.z < self.ocean_z + (value_noise(p.x * 0.02, 0.0, self.seed ^ 0x99) - 0.5) * 60.0
    }

    /// Terreno suave sem buracos e uma caixa ao centro — o tile mais simples
    /// que assa vários polígonos.
    fn plain(center: Vec3) -> Self {
        let mut world = Self {
            seed: 7,
            terrace: 0.0,
            lakes: Vec::new(),
            river: None,
            cliff_seed: 0,
            cliff_threshold: 2.0,
            ocean_z: f32::MIN,
            roads: Vec::new(),
            obstacles: [(center, Vec3::new(1.5, 1.5, 1.5), 0.4)].into(),
        };
        let ground = world.terrain(center.x, center.z);
        world.obstacles[0].0.y = ground;
        world
    }

    /// O mundo da semente `seed` à volta de `center`, num tile de `half` de
    /// meio-lado. A ordem das chamadas ao gerador faz parte da semente: mudá-la
    /// dá outros mundos (e as sementes dos testes deixam de ser as mesmas).
    fn generate(seed: u64, center: Vec3, half: f32) -> Self {
        let mut rng = Rng::new(seed);
        let ext = half + 16.0;
        let lake_count = rng.u64() % 5;
        let lakes = (0..lake_count)
            .map(|_| {
                (
                    Vec3::new(
                        center.x + rng.range(-ext, ext),
                        0.0,
                        center.z + rng.range(-ext, ext),
                    ),
                    rng.range(1.5, 14.0),
                )
            })
            .collect();
        let river = if rng.unit() < 0.4 {
            let n = 3 + rng.u64() % 5;
            let from = Vec3::new(center.x + rng.range(-ext, ext), 0.0, center.z - ext);
            let to = Vec3::new(center.x + rng.range(-ext, ext), 0.0, center.z + ext);
            let stations = (0..n)
                .map(|i| {
                    let t = i as f32 / (n - 1) as f32;
                    from.lerp(to, t) + Vec3::new(rng.range(-12.0, 12.0), 0.0, 0.0)
                })
                .collect();
            Some((stations, rng.range(1.2, 5.0)))
        } else {
            None
        };
        let density_scale = rng.range(0.03, 1.5);
        let mut obstacles = Vec::new();
        let density = (4.0 * ext * ext / 22.0 * density_scale) as u64;
        for _ in 0..density {
            let c = Vec3::new(
                center.x + rng.range(-ext, ext),
                0.0,
                center.z + rng.range(-ext, ext),
            );
            let is_wall = rng.unit() < 0.25;
            let half_ext = if is_wall {
                Vec3::new(
                    rng.range(2.0, 12.0),
                    rng.range(0.6, 3.0),
                    rng.range(0.15, 0.6),
                )
            } else {
                Vec3::new(
                    rng.range(0.12, 3.5),
                    rng.range(0.1, 4.0),
                    rng.range(0.12, 3.5),
                )
            };
            let yaw = rng.range(0.0, std::f32::consts::TAU);
            obstacles.push((c, half_ext, yaw));
            // Muro composto: segmentos encadeados com vãos estreitos (L/U/Z).
            if is_wall && rng.unit() < 0.6 {
                let segments = 1 + rng.u64() % 3;
                let mut prev = (c, half_ext, yaw);
                for _ in 0..segments {
                    let turn = match rng.u64() % 4 {
                        0 => 0.0,
                        1 => std::f32::consts::FRAC_PI_2,
                        2 => std::f32::consts::PI,
                        _ => -std::f32::consts::FRAC_PI_2,
                    };
                    let new_yaw = prev.2 + turn;
                    // O novo segmento sai da ponta do anterior, com um vão.
                    let along = Vec3::new(prev.2.cos(), 0.0, -prev.2.sin())
                        * (prev.1.x + rng.range(-0.3, 1.6));
                    let thickness = rng.range(0.15, 0.6);
                    let length = rng.range(1.5, 10.0);
                    let new_half = Vec3::new(length, rng.range(0.6, 3.0), thickness);
                    let nc =
                        prev.0 + along + Vec3::new(new_yaw.cos(), 0.0, -new_yaw.sin()) * length;
                    obstacles.push((nc, new_half, new_yaw));
                    prev = (nc, new_half, new_yaw);
                }
            }
        }
        let road_count = rng.u64() % 4;
        let roads = (0..road_count)
            .map(|_| {
                let n = 3 + rng.u64() % 6;
                let start = Vec3::new(
                    center.x + rng.range(-ext, ext),
                    0.0,
                    center.z + rng.range(-ext, ext),
                );
                let stations = (0..n)
                    .map(|i| {
                        let t = i as f32 / (n - 1) as f32;
                        start
                            + Vec3::new(t * rng.range(-40.0, 40.0), 0.0, t * rng.range(-40.0, 40.0))
                    })
                    .collect();
                (stations, rng.range(1.2, 4.5))
            })
            .collect();
        let mut world = Self {
            seed: (seed >> 32) as u32 ^ (seed as u32).rotate_left(17),
            terrace: if rng.unit() < 0.5 {
                rng.range(0.3, 0.7)
            } else {
                0.0
            },
            lakes,
            river,
            cliff_seed: rng.u64() as u32,
            cliff_threshold: rng.range(0.80, 0.93),
            ocean_z: if rng.unit() < 0.3 {
                center.z + rng.range(-ext, ext)
            } else {
                f32::MIN
            },
            roads,
            obstacles,
        };
        // Assenta as caixas no terreno do centro.
        let ground: Vec<f32> = world
            .obstacles
            .iter()
            .map(|(c, _, _)| world.terrain(c.x, c.z))
            .collect();
        for ((c, _, _), y) in world.obstacles.iter_mut().zip(ground) {
            c.y = y;
        }
        world
    }

    /// Lattice do chão + obstáculos, como `backend::nav_backend`.
    fn trimesh(&self, min: Vec3, max: Vec3, cell: f32) -> rc::TriMesh {
        let mut mesh = rc::TriMesh::default();
        let (min_xz, max_xz) = (min.xz(), max.xz());
        let cols = (((max_xz.x - min_xz.x) / cell).ceil() as usize).max(1) + 1;
        let rows = (((max_xz.y - min_xz.y) / cell).ceil() as usize).max(1) + 1;
        let mut heights = vec![f32::NAN; cols * rows];
        for row in 0..rows {
            for col in 0..cols {
                let (x, z) = (min_xz.x + col as f32 * cell, min_xz.y + row as f32 * cell);
                let p = Vec3::new(x, 0.0, z);
                if self.in_water(p) || self.ocean_at(p) || self.is_cliff(x, z) {
                    continue;
                }
                heights[row * cols + col] = self.terrain(x, z);
            }
        }
        let mut vertex_of = vec![u32::MAX; cols * rows];
        let mut push_vertex = |mesh: &mut rc::TriMesh, index: usize| -> u32 {
            if vertex_of[index] != u32::MAX {
                return vertex_of[index];
            }
            let (col, row) = (index % cols, index / cols);
            let id = mesh.vertices.len() as u32;
            mesh.vertices.push(Vec3A::new(
                min_xz.x + col as f32 * cell,
                heights[index],
                min_xz.y + row as f32 * cell,
            ));
            vertex_of[index] = id;
            id
        };
        for row in 0..rows.saturating_sub(1) {
            for col in 0..cols.saturating_sub(1) {
                let corners = [
                    row * cols + col,
                    row * cols + col + 1,
                    (row + 1) * cols + col + 1,
                    (row + 1) * cols + col,
                ];
                if corners.iter().any(|i| heights[*i].is_nan()) {
                    continue;
                }
                let ids = corners.map(|i| push_vertex(&mut mesh, i));
                mesh.indices.push(UVec3::new(ids[0], ids[3], ids[1]));
                mesh.indices.push(UVec3::new(ids[1], ids[3], ids[2]));
                mesh.area_types
                    .extend([AreaType::DEFAULT_WALKABLE, AreaType::DEFAULT_WALKABLE]);
            }
        }
        for (c, half, yaw) in &self.obstacles {
            let rot = Quat::from_rotation_y(*yaw);
            let corners = [
                Vec3::new(c.x - half.x, c.y - 0.2, c.z - half.z),
                Vec3::new(c.x + half.x, c.y - 0.2, c.z - half.z),
                Vec3::new(c.x + half.x, c.y - 0.2, c.z + half.z),
                Vec3::new(c.x - half.x, c.y - 0.2, c.z + half.z),
                Vec3::new(c.x - half.x, c.y + 2.0 * half.y, c.z - half.z),
                Vec3::new(c.x + half.x, c.y + 2.0 * half.y, c.z - half.z),
                Vec3::new(c.x + half.x, c.y + 2.0 * half.y, c.z + half.z),
                Vec3::new(c.x - half.x, c.y + 2.0 * half.y, c.z + half.z),
            ];
            let base = mesh.vertices.len() as u32;
            for corner in corners {
                let d = corner - Vec3::new(c.x, corner.y, c.z);
                let d = rot * d;
                mesh.vertices
                    .push(Vec3A::from(Vec3::new(c.x + d.x, corner.y, c.z + d.z)));
            }
            // As faces de `backend::box_trimesh`, na mesma ordem.
            const FACES: [[u32; 6]; 6] = [
                [0, 1, 2, 0, 2, 3],
                [4, 5, 6, 4, 6, 7],
                [0, 4, 5, 0, 5, 1],
                [3, 2, 6, 3, 6, 7],
                [0, 3, 7, 0, 7, 4],
                [1, 5, 6, 1, 6, 2],
            ];
            for face in FACES {
                mesh.indices
                    .push(UVec3::new(base + face[0], base + face[1], base + face[2]));
                mesh.indices
                    .push(UVec3::new(base + face[3], base + face[4], base + face[5]));
                mesh.area_types
                    .extend([AreaType::NOT_WALKABLE, AreaType::NOT_WALKABLE]);
            }
        }
        mesh
    }

    /// Estradas como volumes convexos, como `tile::road_area_volumes`.
    fn road_volumes(&self, center: Vec3, half: f32) -> Vec<ConvexVolume> {
        let (min, max) = (
            center.xz() - Vec2::splat(half),
            center.xz() + Vec2::splat(half),
        );
        let mut out = Vec::new();
        for (stations, half_width) in &self.roads {
            for pair in stations.windows(2) {
                let (a, b) = (pair[0].xz(), pair[1].xz());
                let seg_min = a.min(b) - Vec2::splat(*half_width);
                let seg_max = a.max(b) + Vec2::splat(*half_width);
                if seg_max.x < min.x || seg_min.x > max.x || seg_max.y < min.y || seg_min.y > max.y
                {
                    continue;
                }
                let along = b - a;
                if along.length_squared() < 1e-6 {
                    continue;
                }
                let side = Vec2::new(-along.y, along.x).normalize() * *half_width;
                let mid = (a + b) * 0.5;
                let y = self.terrain(mid.x, mid.y);
                out.push(ConvexVolume {
                    vertices: vec![a - side, a + side, b + side, b - side],
                    min_y: y - tile::ROAD_BAND_BELOW,
                    max_y: y + tile::ROAD_BAND_ABOVE,
                    area: tile::AREA_ROAD,
                });
            }
        }
        out
    }
}

// ── o pipeline do `bevy_rerecast_core::generator`, com os números do jogo ──

const CONFIG: NavConfig = NavConfig {
    enabled: true,
    agent_radius: crate::DEFAULT_AGENT_RADIUS,
    agent_height: crate::DEFAULT_AGENT_HEIGHT,
    tile_size: crate::DEFAULT_TILE_SIZE,
    retile_margin: crate::DEFAULT_RETILE_MARGIN,
    ground_cell: crate::DEFAULT_GROUND_CELL,
    walkable_climb: 0.5,
    max_slope_deg: 50.0,
    offroad_cost: crate::DEFAULT_OFFROAD_COST,
    lookahead_secs: 2.0,
    min_lookahead: 3.0,
    max_speed_factor: 1.35,
    arrive_distance: 1.5,
    debug: false,
};

/// Assa o mundo `world` à volta de `center` com as definições do jogo e devolve
/// o que o gerador entregaria ao armazém de assets.
fn bake(world: &World, center: Vec3, half: f32) -> bevy_rerecast::Navmesh {
    let mut settings = tile::tile_settings(&CONFIG, center.xz(), None);
    // Sem terreno o `tile_settings` estica o tile até 256 m de altura. O mundo
    // destes testes não sai de ±8 m, e em debug cada célula a mais custa.
    settings.aabb = Some(bevy::shape::Aabb3d {
        min: Vec3A::new(center.x - half, -16.0, center.z - half),
        max: Vec3A::new(center.x + half, 32.0, center.z + half),
    });
    let aabb = settings.aabb.expect("the tile is bounded just above");
    let trimesh = world.trimesh(
        Vec3::from(aabb.min),
        Vec3::from(aabb.max),
        CONFIG.ground_cell,
    );
    let volumes = world.road_volumes(center, half);

    // `NavmeshSettings::into_rerecast_config` é do `bevy_rerecast_core`; os
    // números são os que `tile_settings` põe nas definições.
    let config = rc::ConfigBuilder {
        agent_height: settings.agent_height,
        agent_radius: settings.agent_radius,
        walkable_climb: settings.walkable_climb,
        walkable_slope_angle: settings.walkable_slope_angle,
        aabb: rc::Aabb3d {
            min: Vec3::from(aabb.min),
            max: Vec3::from(aabb.max),
        },
        ..Default::default()
    }
    .build();

    let mut trimesh = trimesh;
    trimesh.mark_walkable_triangles(config.walkable_slope_angle);
    let mut heightfield = rc::HeightfieldBuilder {
        aabb: config.aabb,
        cell_size: config.cell_size,
        cell_height: config.cell_height,
    }
    .build()
    .expect("heightfield");
    heightfield
        .rasterize_triangles(&trimesh, config.walkable_climb)
        .expect("rasterize");
    heightfield.filter_low_hanging_walkable_obstacles(config.walkable_climb);
    heightfield.filter_ledge_spans(config.walkable_height, config.walkable_climb);
    heightfield.filter_walkable_low_height_spans(config.walkable_height);
    let mut compact = heightfield
        .into_compact(config.walkable_height, config.walkable_climb)
        .expect("compact heightfield");
    compact.erode_walkable_area(config.walkable_radius);
    for volume in &volumes {
        compact.mark_convex_poly_area(volume);
    }
    compact.build_distance_field();
    compact
        .build_regions(
            config.border_size,
            config.min_region_area,
            config.merge_region_area,
        )
        .expect("regions");
    let contours = compact.build_contours(
        config.max_simplification_error,
        config.max_edge_len,
        config.contour_flags,
    );
    let polygon = contours
        .into_polygon_mesh(config.max_vertices_per_polygon)
        .expect("polygon mesh");
    let detail = rc::DetailNavmesh::new(
        &polygon,
        &compact,
        config.detail_sample_dist,
        config.detail_sample_max_error,
    )
    .expect("detail mesh");
    bevy_rerecast::Navmesh {
        polygon,
        detail,
        settings,
    }
}

/// O mundo da semente `seed`, num tile de `half` metros de meio-lado centrado
/// onde o próprio gerador decidir.
fn seeded_tile(seed: u64, half: f32) -> bevy_rerecast::Navmesh {
    let mut rng = Rng::new(seed ^ 0xABCDEF);
    let center = Vec3::new(rng.range(-500.0, 500.0), 0.0, rng.range(-500.0, 500.0));
    let world = World::generate(seed, center, half);
    bake(&world, center, half)
}

// ── os testes ────────────────────────────────────────────────────────────

/// Uma amostra de mundos pelo pipeline inteiro, `(semente, meio-lado do
/// tile)`. O caso do Vael que o `landmass` recusou está acima, com os contornos
/// reais; estes são a rede para o que ainda não aconteceu. Tiles pequenos de
/// propósito: em debug um bake custa segundos por milhar de metros quadrados.
const SAMPLED_TILES: [(u64, f32); 6] = [
    (218, 10.0),
    (306, 12.0),
    (388, 12.0),
    (0, 8.0),
    (1, 8.0),
    (2, 8.0),
];

#[test]
fn tiles_the_generator_bakes_are_accepted_by_landmass() {
    for (seed, half) in SAMPLED_TILES {
        let navmesh = seeded_tile(seed, half);
        landmass_mesh(&navmesh).unwrap_or_else(|err| {
            panic!("tile da semente {seed} ({half} m de meio-lado) recusado: {err}")
        });
    }
}

fn plain_tile() -> bevy_rerecast::Navmesh {
    let center = Vec3::new(37.3, 0.0, -12.7);
    bake(&World::plain(center), center, 6.0)
}

/// O mesmo tile com o primeiro polígono repetido: cada aresta dele passa a ter
/// três donos, exactamente o que o landmass recusa.
fn tile_with_a_doubled_polygon() -> bevy_rerecast::Navmesh {
    let mut navmesh = plain_tile();
    let nvp = navmesh.polygon.max_vertices_per_polygon as usize;
    assert!(navmesh.polygon.polygon_count() > 1, "o tile tem vizinhos");
    for _ in 0..2 {
        let first = navmesh.polygon.polygons[..nvp].to_vec();
        navmesh.polygon.polygons.extend_from_slice(&first);
        navmesh
            .polygon
            .polygon_neighbors
            .extend(std::iter::repeat_n(u16::MAX, nvp));
        navmesh.polygon.areas.push(navmesh.polygon.areas[0]);
        navmesh.detail.meshes.push(navmesh.detail.meshes[0].clone());
    }
    navmesh
}

/// Dois contornos do tile que o `landmass` recusou no Vael a 2026-11-23
/// (`vendor/rerecast/VIBER_PATCH.md` #8), tal como o gerador os produziu —
/// `(x, y, z, flags)` em células: o chão (região 365) e a estrada (368). A
/// estrada simplificou a parede com o chão com um vértice a mais, e a orelha
/// desse vértice partilhou a corda com o corpo e com o chão.
const REFUSED_GROUND: [(u16, u16, u16, u32); 33] = [
    (5, 2154, 1476, 418),
    (11, 2154, 1481, 131072),
    (21, 2153, 1480, 131072),
    (26, 2154, 1482, 418),
    (27, 2154, 1481, 131440),
    (27, 2152, 1478, 131440),
    (31, 2151, 1475, 131440),
    (25, 2151, 1477, 131440),
    (22, 2148, 1472, 131440),
    (27, 2146, 1469, 131534),
    (21, 2147, 1471, 131534),
    (19, 2142, 1466, 131534),
    (26, 2140, 1462, 131533),
    (19, 2139, 1463, 131533),
    (18, 2132, 1457, 131533),
    (26, 2133, 1456, 131072),
    (26, 2132, 1455, 131531),
    (17, 2130, 1454, 131531),
    (19, 2120, 1442, 131531),
    (25, 2121, 1442, 131489),
    (19, 2118, 1440, 131489),
    (22, 2110, 1429, 131072),
    (17, 2112, 1433, 131072),
    (10, 2105, 1433, 131072),
    (9, 2106, 1435, 131072),
    (14, 2115, 1438, 131072),
    (14, 2121, 1444, 131072),
    (10, 2124, 1448, 131072),
    (10, 2128, 1452, 131072),
    (6, 2128, 1455, 131072),
    (5, 2135, 1462, 131072),
    (0, 2138, 1467, 131072),
    (5, 2148, 1473, 131072),
];

const REFUSED_ROAD: [(u16, u16, u16, u32); 44] = [
    (30, 2146, 1468, 462),
    (29, 2146, 1468, 131717),
    (27, 2146, 1469, 131437),
    (22, 2148, 1472, 131437),
    (23, 2150, 1475, 131437),
    (25, 2151, 1477, 131437),
    (31, 2151, 1475, 131720),
    (32, 2151, 1474, 131719),
    (33, 2150, 1473, 131719),
    (32, 2151, 1474, 131720),
    (31, 2151, 1475, 131437),
    (27, 2152, 1478, 131437),
    (27, 2154, 1481, 131490),
    (31, 2158, 1484, 131490),
    (36, 2154, 1480, 131721),
    (37, 2154, 1479, 131721),
    (36, 2154, 1480, 131490),
    (32, 2158, 1484, 131490),
    (34, 2169, 1490, 131490),
    (28, 2177, 1498, 131072),
    (25, 2183, 1501, 131559),
    (6, 2207, 1524, 32772),
    (37, 2208, 1524, 163844),
    (38, 2209, 1524, 131467),
    (39, 2208, 1522, 131467),
    (35, 2206, 1519, 131467),
    (40, 2206, 1517, 131072),
    (34, 2204, 1517, 131072),
    (32, 2195, 1512, 131072),
    (37, 2194, 1510, 131072),
    (36, 2184, 1505, 131072),
    (40, 2183, 1503, 131072),
    (38, 2175, 1498, 131072),
    (41, 2172, 1495, 131072),
    (49, 2170, 1494, 131072),
    (49, 2162, 1484, 131722),
    (48, 2161, 1483, 131072),
    (44, 2155, 1478, 131072),
    (38, 2154, 1478, 131072),
    (35, 2150, 1473, 131718),
    (35, 2150, 1472, 131072),
    (35, 2148, 1469, 131072),
    (32, 2146, 1467, 462),
    (31, 2145, 1467, 131716),
];

fn vael_refused_tile() -> bevy_rerecast::Navmesh {
    let contour = |region: u16, area: u8, vertices: &[(u16, u16, u16, u32)]| rc::Contour {
        vertices: vertices
            .iter()
            .map(|&(x, y, z, flags)| (U16Vec3::new(x, y, z), flags))
            .collect(),
        region: rc::RegionId::from(region),
        area: AreaType(area),
        ..Default::default()
    };
    let contours = rc::ContourSet {
        contours: vec![
            contour(365, 255, &REFUSED_GROUND),
            contour(368, 1, &REFUSED_ROAD),
        ],
        aabb: rc::Aabb3d {
            min: Vec3::new(-39.406_464, -200.0, 55.795_258),
            max: Vec3::new(216.593_54, 400.0, 311.795_26),
        },
        width: 1524,
        height: 1524,
        cell_size: 0.166_666_67,
        cell_height: 0.125,
        border_size: 6,
        max_error: 1.3,
    };
    let polygon = contours
        .into_polygon_mesh(6)
        .expect("the contours of a baked tile build a polygon mesh");
    // O detail mesh não interessa à validação das arestas; cada polígono
    // aponta para o mesmo triângulo.
    let detail = rc::DetailNavmesh {
        meshes: (0..polygon.polygon_count())
            .map(|_| rc::SubMesh {
                base_vertex_index: 0,
                vertex_count: 3,
                base_triangle_index: 0,
                triangle_count: 1,
            })
            .collect(),
        triangles: vec![[0, 1, 2]],
        vertices: vec![Vec3::ZERO, Vec3::Z, Vec3::X],
        ..Default::default()
    };
    bevy_rerecast::Navmesh {
        polygon,
        detail,
        settings: NavmeshSettings::default(),
    }
}

#[test]
fn the_tile_landmass_refused_in_vael_is_accepted() {
    landmass_mesh(&vael_refused_tile()).unwrap_or_else(|err| {
        panic!("o tile que o landmass recusou no Vael continua recusado: {err}")
    });
}

#[test]
fn landmass_mesh_refuses_an_edge_shared_by_three_polygons() {
    let refused = tile_with_a_doubled_polygon();
    assert!(matches!(
        landmass_mesh(&refused),
        Err(bevy_landmass::ValidationError::DoublyConnectedEdge(_, _))
    ));
}

#[test]
fn publish_tile_puts_an_accepted_bake_on_the_island() {
    let mut landmass = Assets::<NavMesh3d>::default();
    let island = bevy_landmass::NavMeshHandle(landmass.reserve_handle());
    let mut tile = NavTile {
        generating: true,
        failures: 2,
        ..Default::default()
    };

    assert!(publish_tile(
        &mut tile,
        Some(plain_tile()),
        Some(&island),
        &mut landmass
    ));

    assert!(
        landmass.get(&island.0).is_some(),
        "a malha da ilha passou a existir"
    );
    assert!(!tile.generating, "o tile deixou de estar em voo");
    assert_eq!(tile.failures, 0, "um tile aceite repõe as falhas");
}

#[test]
fn publish_tile_keeps_the_previous_mesh_and_queues_a_retry_when_refused() {
    let mut landmass = Assets::<NavMesh3d>::default();
    let island = bevy_landmass::NavMeshHandle(landmass.reserve_handle());
    let previous = landmass_mesh(&plain_tile()).unwrap();
    let previous = Arc::clone(&previous.nav_mesh);
    landmass
        .insert(
            island.0.id(),
            bevy_landmass::NavMesh {
                nav_mesh: Arc::clone(&previous),
            },
        )
        .unwrap();
    let mut tile = NavTile {
        generating: true,
        failures: 1,
        ..Default::default()
    };

    assert!(!publish_tile(
        &mut tile,
        Some(tile_with_a_doubled_polygon()),
        Some(&island),
        &mut landmass
    ));

    assert!(
        Arc::ptr_eq(&landmass.get(&island.0).unwrap().nav_mesh, &previous),
        "a ilha fica com a malha que tinha"
    );
    assert!(tile.generating, "o tile continua em voo: é um retry");
    assert!(tile.rejected);
    assert_eq!(
        tile.probe_cooldown,
        tile::failure_backoff(1),
        "o retry respeita o recuo das falhas"
    );
}

#[test]
fn publish_tile_without_a_bake_changes_nothing() {
    let mut landmass = Assets::<NavMesh3d>::default();
    let island = bevy_landmass::NavMeshHandle(landmass.reserve_handle());
    let mut tile = NavTile {
        generating: true,
        ..Default::default()
    };

    assert!(!publish_tile(&mut tile, None, Some(&island), &mut landmass));

    assert!(landmass.get(&island.0).is_none());
    assert_eq!(tile.generating, true);
    assert_eq!(tile.probe_cooldown, 0.0);
}
