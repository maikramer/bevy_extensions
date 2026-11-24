//! Re-bakes a captured navmesh tile and checks it against landmass.
//!
//! A bake is a function of two things — the triangle mesh the backend hands
//! rerecast and the settings it voxelises with — so one capture is the whole of
//! a bake. With `VAEL_NAV_DUMP=<dir>` the game writes one `<dir>/nav-*.vnav`
//! per bake; this example re-runs it with no ECS and says whether landmass
//! takes the result, which is how a tile the game refused ("nav: tile #N
//! recusado pelo landmass") is examined and turned into a test.
//!
//! ```bash
//! cargo run -p bevy_ext_nav --example rebake_dump -- /tmp/nav-dumps/nav-00016*.vnav
//! ```
//!
//! Tile inputs are megabytes, so captures stay out of the repository: a test
//! keeps the contours the generator produced (see `landmass_conformance.rs`).

use std::path::Path;

use bevy::math::{Quat, UVec3, Vec2, Vec3, Vec3A, Vec3Swizzles};
use bevy_ext_nav::landmass_mesh;
use bevy_rerecast::Navmesh;
use bevy_rerecast::rerecast as rc;
use bevy_rerecast::rerecast::{AreaType, ConvexVolume};

fn main() {
    let mut refused = 0;
    let paths: Vec<_> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("uso: rebake_dump <captura.vnav>…");
        std::process::exit(2);
    }
    for path in &paths {
        let navmesh = read_dump(Path::new(path)).and_then(rebake);
        let navmesh = match navmesh {
            Ok(navmesh) => navmesh,
            Err(err) => {
                eprintln!("{}: {err}", Path::new(path).display());
                refused += 1;
                continue;
            }
        };
        let polygons = navmesh.polygon.polygon_count();
        match landmass_mesh(&navmesh) {
            Ok(_) => println!(
                "{}: {polygons} polígonos — aceite pelo landmass",
                Path::new(path).display()
            ),
            Err(err) => {
                println!(
                    "{}: {polygons} polígonos — RECUSADO: {err}",
                    Path::new(path).display()
                );
                if let bevy_landmass::ValidationError::DoublyConnectedEdge(a, b) = err {
                    print_owners(&navmesh, a, b);
                }
                refused += 1;
            }
        }
    }
    if refused > 0 {
        std::process::exit(1);
    }
}

/// Reads a capture: the format `backend::dump_input` writes.
fn read_dump(path: &Path) -> Result<(rc::TriMesh, rc::ConfigBuilder), String> {
    let bytes = std::fs::read(path).map_err(|err| format!("ler: {err}"))?;
    let mut at = 0;
    let mut take = |len: usize| -> Result<&[u8], String> {
        let slice = bytes.get(at..at + len).ok_or("captura truncada")?;
        at += len;
        Ok(slice)
    };
    let f32_at = |slice: &[u8], i: usize| {
        f32::from_le_bytes(slice[i * 4..i * 4 + 4].try_into().unwrap())
    };
    if take(8)? != b"VAELNAV1" {
        return Err("não é uma captura VAELNAV1".into());
    }
    let floats = take(9 * 4)?;
    let shorts = take(5 * 2)?;
    let float = |i: usize| f32::from_le_bytes(floats[i * 4..i * 4 + 4].try_into().unwrap());
    let short = |i: usize| u16::from_le_bytes(shorts[i * 2..i * 2 + 2].try_into().unwrap());
    let solid = u8::from(take(1)?[0] != 0);
    let areas = u8::from(take(1)?[0] != 0);
    let tiling = take(1)?[0] != 0;
    let aabb = if take(1)?[0] != 0 {
        let min = take(12)?;
        let max = take(12)?;
        Some(rc::Aabb3d {
            min: Vec3::new(f32_at(min, 0), f32_at(min, 1), f32_at(min, 2)),
            max: Vec3::new(f32_at(max, 0), f32_at(max, 1), f32_at(max, 2)),
        })
    } else {
        None
    };
    let u32_at = |slice: &[u8]| u32::from_le_bytes(slice.try_into().unwrap());
    let mut volumes = Vec::new();
    let volume_count = u32_at(take(4)?);
    for _ in 0..volume_count {
        let vertex_count = u32_at(take(4)?) as usize;
        let vertices = take(vertex_count * 8)?
            .chunks_exact(8)
            .map(|v| Vec2::new(f32_at(v, 0), f32_at(v, 1)))
            .collect();
        let band = take(8)?;
        volumes.push(ConvexVolume {
            vertices,
            min_y: f32_at(band, 0),
            max_y: f32_at(band, 1),
            area: AreaType(take(1)?[0]),
        });
    }
    let vertex_count = u32_at(take(4)?) as usize;
    let vertices = take(vertex_count * 12)?
        .chunks_exact(12)
        .map(|v| Vec3A::new(f32_at(v, 0), f32_at(v, 1), f32_at(v, 2)))
        .collect();
    let index_count = u32_at(take(4)?) as usize;
    let indices = take(index_count * 12)?
        .chunks_exact(12)
        .map(|i| UVec3::new(u32_at(&i[0..4]), u32_at(&i[4..8]), u32_at(&i[8..12])))
        .collect();
    let area_count = u32_at(take(4)?) as usize;
    let area_types = take(area_count)?
        .iter()
        .map(|area| AreaType(*area))
        .collect();
    if at != bytes.len() {
        return Err("captura com bytes a mais".into());
    }
    Ok((
        rc::TriMesh {
            vertices,
            indices,
            area_types,
        },
        rc::ConfigBuilder {
            agent_height: float(0),
            agent_radius: float(1),
            walkable_climb: float(2),
            walkable_slope_angle: float(3),
            min_region_size: short(0),
            merge_region_size: short(1),
            max_simplification_error: float(6),
            max_vertices_per_polygon: short(3),
            detail_sample_dist: float(7),
            detail_sample_max_error: float(8),
            tile_size: short(4),
            aabb: aabb.unwrap_or_default(),
            contour_flags: rc::BuildContoursFlags::from_bits_retain(solid | areas << 1),
            tiling,
            area_volumes: volumes,
            cell_size_fraction: float(4),
            cell_height_fraction: float(5),
            edge_max_len_factor: short(2),
        },
    ))
}

/// The pipeline of `bevy_rerecast_core::generator`, on a built config. It is
/// private there; the order is the contract (the areas are painted before the
/// distance field, so regions never mix areas).
fn rebake((trimesh, builder): (rc::TriMesh, rc::ConfigBuilder)) -> Result<Navmesh, String> {
    let config = builder.build();
    let mut trimesh = trimesh;
    trimesh.mark_walkable_triangles(config.walkable_slope_angle);
    let mut heightfield = rc::HeightfieldBuilder {
        aabb: config.aabb,
        cell_size: config.cell_size,
        cell_height: config.cell_height,
    }
    .build()
    .map_err(|err| format!("heightfield: {err}"))?;
    heightfield
        .rasterize_triangles(&trimesh, config.walkable_climb)
        .map_err(|err| format!("rasterize: {err}"))?;
    heightfield.filter_low_hanging_walkable_obstacles(config.walkable_climb);
    heightfield.filter_ledge_spans(config.walkable_height, config.walkable_climb);
    heightfield.filter_walkable_low_height_spans(config.walkable_height);
    let mut compact = heightfield
        .into_compact(config.walkable_height, config.walkable_climb)
        .map_err(|err| format!("compact: {err}"))?;
    compact.erode_walkable_area(config.walkable_radius);
    for volume in &config.area_volumes {
        compact.mark_convex_poly_area(volume);
    }
    compact.build_distance_field();
    compact
        .build_regions(
            config.border_size,
            config.min_region_area,
            config.merge_region_area,
        )
        .map_err(|err| format!("regions: {err}"))?;
    let contours = compact.build_contours(
        config.max_simplification_error,
        config.max_edge_len,
        config.contour_flags,
    );
    let polygon = contours
        .into_polygon_mesh(config.max_vertices_per_polygon)
        .map_err(|err| format!("polygon mesh: {err}"))?;
    let detail = rc::DetailNavmesh::new(
        &polygon,
        &compact,
        config.detail_sample_dist,
        config.detail_sample_max_error,
    )
    .map_err(|err| format!("detail mesh: {err}"))?;
    Ok(Navmesh {
        polygon,
        detail,
        settings: Default::default(),
    })
}

/// The polygons on both ends of an edge, in world space.
fn print_owners(navmesh: &Navmesh, a: usize, b: usize) {
    let polygon = &navmesh.polygon;
    let world = |index: usize| {
        let v = polygon.vertices[index];
        polygon.aabb.min.xz() + v.as_vec3().xz() * polygon.cell_size
    };
    println!(
        "  aresta ({a}, {b}) de {:?} a {:?}; donos:",
        world(a),
        world(b)
    );
    for (i, poly) in polygon.polygons().enumerate() {
        let vertices: Vec<usize> = poly.map(usize::from).collect();
        if vertices.contains(&a) && vertices.contains(&b) {
            println!(
                "    polígono {i} (região {:?}): {:?}",
                polygon.regions[i],
                vertices
            );
        }
    }
    // The unused imports keep the example honest about what a dump carries.
    let _ = (Quat::IDENTITY, Vec3A::ZERO);
}
