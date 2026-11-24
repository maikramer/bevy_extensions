use crate::{
    Aabb3d, AreaType, RegionId,
    contours::{ContourSet, RegionVertexId},
    math::{next, prev},
};
use alloc::vec::Vec;
#[cfg(feature = "bevy_reflect")]
use bevy_reflect::prelude::*;
use glam::{U16Vec2, U16Vec3, Vec3Swizzles as _, u16vec3, uvec3};
use thiserror::Error;

#[derive(Debug, Default, Clone, PartialEq)]
struct InternalPolygonNavmesh {
    vertices: Vec<U16Vec3>,
    nvertices: u16,
    polygons: Vec<u16>,
    npolys: usize,
    regions: Vec<RegionId>,
    flags: Vec<u16>,
    areas: Vec<AreaType>,
    max_polygons: usize,
    max_vertices_per_polygon: u16,
    aabb: Aabb3d,
    cell_size: f32,
    cell_height: f32,
    border_size: u16,
    max_edge_error: f32,
}

/// Represents a polygon mesh suitable for use in building a navigation mesh.
///
/// A mesh of potentially overlapping convex polygons of between three and nvp vertices. The mesh exists within the context of an axis-aligned bounding box (AABB) with vertices laid out in an evenly spaced grid, based on the values of cs and ch.
///
/// The standard process for building a contour set is to allocate it using rcAllocPolyMesh, the initialize it using rcBuildPolyMesh
///
/// Example of iterating the polygons:
/// ```rust
/// # use rerecast::*;
/// # use glam::Vec3;
/// # let mut mesh = PolygonNavmesh::default();
/// # mesh.max_vertices_per_polygon = 1;
/// // Where mesh is a reference to a PolygonNavmesh.
/// let nvp = mesh.max_vertices_per_polygon as usize;
/// let cs = mesh.cell_size;
/// let ch = mesh.cell_height;
/// let orig = mesh.aabb.min;
///
/// for i in 0..mesh.polygon_count() {
///     let p = &mesh.polygons[i * nvp..];
///
///     // Iterate the vertices.
///     for j in 0..nvp {
///         if p[j] == PolygonNavmesh::NO_INDEX {
///             // End of vertices.
///             break;
///         }
///
///         if p[j + nvp] == PolygonNavmesh::NO_CONNECTION {
///             // The edge beginning with this vertex is a solid border.
///         } else {
///             // The edge beginning with this vertex connects to
///             // polygon p[j + nvp].
///         }
///
///         // Convert to world space.
///         let v = &mesh.vertices[p[j] as usize];
///         let world_vertex = Vec3 {
///             x: orig.x + v.x as f32 * cs,
///             y: orig.y + v.y as f32 * ch,
///             z: orig.z + v.z as f32 * cs,
///         };
///         // Do something with the vertices.
///         println!("Vertex: {world_vertex}");
///     }
/// }
/// ```
#[derive(Debug, Default, Clone, PartialEq)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bevy_reflect", derive(Reflect))]
#[cfg_attr(
    all(feature = "serialize", feature = "bevy_reflect"),
    reflect(Serialize, Deserialize)
)]
pub struct PolygonNavmesh {
    /// The mesh vertices.
    ///
    /// The values of [`Aabb3d::min`], [`Self::cell_size`], and [`Self::cell_height`] are used to convert vertex coordinates to world space as follows:
    /// ```rust
    /// # use rerecast::*;
    /// # use glam::{Vec3, U16Vec3};
    /// # let mut mesh = PolygonNavmesh::default();
    /// # mesh.vertices = vec![U16Vec3::ZERO; 1];
    /// # let i = 0;
    /// let world_vertex = Vec3 {
    ///     x: mesh.aabb.min.x + mesh.vertices[i].x as f32 * mesh.cell_size,
    ///     y: mesh.aabb.min.y + mesh.vertices[i].y as f32 * mesh.cell_height,
    ///     z: mesh.aabb.min.z + mesh.vertices[i].z as f32 * mesh.cell_size,
    /// };
    /// ```
    pub vertices: Vec<U16Vec3>,
    /// Polygons. [Length: [Self::polygon_count]].
    ///
    /// Each entry is [`Self::max_vertices_per_polygon`] in length.
    /// The polygons are in counter-clockwise order on the XZ plane. They are guaranteed to be convex. They are not guaranteed to be planar.
    ///
    /// The first instance of [`Self::NO_INDEX`] indicates the end of the indices for the entry.
    ///
    /// For example:
    /// ```txt
    /// vertices_per_polygon = 6
    /// For the entry:
    /// - polygons: (1, 3, 4, 8, NO_INDEX, NO_INDEX)
    /// - polygon_neighbors: (18, NO_CONNECTION , 21, NO_CONNECTION, NO_CONNECTION, NO_CONNECTION)
    ///
    /// (1, 3, 4, 8) defines a polygon with 4 vertices.
    /// Edge 1->3 is shared with polygon 18.
    /// Edge 4->8 is shared with polygon 21.
    /// Edges 3->4 and 4->8 are border edges not shared with any other polygon.
    /// ```
    pub polygons: Vec<u16>,
    /// Contains indices to each edge's connected polygons.
    /// A value of [`Self::NO_CONNECTION`] indicates no connection for the associated edge.
    /// (i.e. The edge is a solid border.)
    pub polygon_neighbors: Vec<u16>,
    /// The user-defined flags assigned to each polygon.
    pub flags: Vec<u16>,
    /// The region id assigned to each polygon.
    pub regions: Vec<RegionId>,
    /// The area id assigned to each polygon.
    ///
    /// The standard build process assigns the value of [`AreaType::DEFAULT_WALKABLE`] to all walkable polygons.
    /// This value can then be changed to meet user requirements.
    pub areas: Vec<AreaType>,
    /// The maximum number of vertices per polygon
    pub max_vertices_per_polygon: u16,
    /// The bounding box of the mesh in world space.
    pub aabb: Aabb3d,
    /// The size of each cell. (On the xz-plane.)
    pub cell_size: f32,
    /// The height of each cell. (The minimum increment along the y-axis.)
    pub cell_height: f32,
    /// The AABB border size used to generate the source data from which the mesh was derived.
    pub border_size: u16,
    /// The max error of the polygon edges in the mesh.
    pub max_edge_error: f32,
}

impl PolygonNavmesh {
    /// The number of polygons in the mesh. Note that this is different from `polygons.len()`,
    /// as each polygon in that collection is represented by [`Self::max_vertices_per_polygon`] vertices.
    #[inline]
    pub fn polygon_count(&self) -> usize {
        self.polygons.len() / self.max_vertices_per_polygon as usize
    }

    /// A value which indicates that a polygon in [`Self::polygons`] has no more vertices starting from this vertex with this value.
    pub const NO_INDEX: u16 = 0xffff;

    /// A value which indicates that an edge of a polygon in [`Self::polygon_neighbors`] has no connection.
    pub const NO_CONNECTION: u16 = 0xffff;

    /// Iterates over all polygons in the mesh.
    pub fn polygons(&self) -> impl Iterator<Item = impl Iterator<Item = u16>> {
        self.polygons
            .chunks_exact(self.max_vertices_per_polygon as usize)
            .map(|chunk| chunk.iter().take_while(|i| **i != Self::NO_INDEX).copied())
    }
}

impl From<InternalPolygonNavmesh> for PolygonNavmesh {
    fn from(mut value: InternalPolygonNavmesh) -> Self {
        let nvp = value.max_vertices_per_polygon as usize;
        value.polygons.truncate(value.npolys * 2 * nvp);
        let mut polygons = Vec::with_capacity(value.polygons.len() / 2);
        let mut polygon_neighbors = Vec::with_capacity(value.polygons.len() / 2);
        for poly in value.polygons.chunks_exact(nvp * 2) {
            let (vertices, neighbors) = poly.split_at(nvp);
            polygons.extend_from_slice(vertices);
            polygon_neighbors.extend_from_slice(neighbors);
        }
        value.vertices.truncate(value.nvertices as usize);
        value.areas.truncate(value.npolys);
        PolygonNavmesh {
            vertices: value.vertices,
            polygons,
            polygon_neighbors,
            regions: value.regions,
            flags: value.flags,
            areas: value.areas,
            max_vertices_per_polygon: value.max_vertices_per_polygon,
            aabb: value.aabb,
            cell_size: value.cell_size,
            cell_height: value.cell_height,
            border_size: value.border_size,
            max_edge_error: value.max_edge_error,
        }
    }
}

impl ContourSet {
    /// Builds a polygon mesh from the provided contours.
    pub fn into_polygon_mesh(
        self,
        max_vertices_per_polygon: u16,
    ) -> Result<PolygonNavmesh, PolygonNavmeshError> {
        let mut mesh = InternalPolygonNavmesh {
            aabb: self.aabb,
            cell_size: self.cell_size,
            cell_height: self.cell_height,
            border_size: self.border_size,
            max_edge_error: self.max_error,
            max_vertices_per_polygon,
            ..Default::default()
        };
        let nvp = max_vertices_per_polygon as usize;

        let mut max_vertices = 0;
        let mut max_tris = 0;
        let mut max_verts_per_cont = 0;
        for contour in &self.contours {
            // Skip null contours.
            if contour.vertices.len() < 3 {
                continue;
            }
            max_vertices += contour.vertices.len();
            max_tris += contour.vertices.len() - 2;
            max_verts_per_cont = max_verts_per_cont.max(contour.vertices.len());
        }

        if max_vertices > u16::MAX as usize {
            // Jan: Is this sensible? It's the original, but I suspect u32 is fine
            return Err(PolygonNavmeshError::TooManyVertices {
                actual: max_vertices,
                max: u16::MAX as usize,
            });
        }

        let mut vflags = vec![false; max_vertices];
        mesh.vertices = vec![U16Vec3::ZERO; max_vertices];
        mesh.polygons = vec![u16::MAX; max_tris * nvp * 2];
        mesh.regions = vec![RegionId::default(); max_tris];
        mesh.areas = vec![AreaType::default(); max_tris];

        let mut next_vert = vec![Some(0); max_vertices];
        let mut first_vert = [None; VERTEX_BUCKET_COUNT];
        let mut indices = vec![0; max_verts_per_cont];
        let mut tris = vec![U16Vec3::ZERO; max_verts_per_cont];
        let mut polys = vec![u16::MAX; (max_verts_per_cont + 1) * nvp];

        let temp_poly_index = max_verts_per_cont * nvp;

        for cont in &self.contours {
            // Skip null contours.
            if cont.vertices.len() < 3 {
                continue;
            }

            // Triangulate contour
            #[expect(clippy::needless_range_loop)]
            for j in 0..cont.vertices.len() {
                indices[j] = j;
            }

            // Jan: we treat an invalid triangulation as an error instead of a warning.
            let ntris = triangulate(cont.vertices.len(), &cont.vertices, &mut indices, &mut tris)?;
            // Add and merge vertices.
            for j in 0..cont.vertices.len() {
                let (v, region) = &cont.vertices[j];
                indices[j] = add_vertex(
                    *v,
                    &mut mesh.vertices,
                    &mut first_vert,
                    &mut next_vert,
                    &mut mesh.nvertices,
                ) as usize;
                if (region & RegionVertexId::BORDER_VERTEX.bits()) != 0 {
                    // This vertex should be removed.
                    vflags[indices[j]] = true;
                }
            }
            // Build initial polygons.
            let mut npolys = 0;
            polys.fill(u16::MAX);
            for t in tris.iter().take(ntris) {
                // Viber patch: ver `wound_right`.
                if t.x != t.y
                    && t.x != t.z
                    && t.y != t.z
                    && wound_right(&cont.vertices, *t)
                {
                    polys[npolys * nvp] = indices[t.x as usize] as u16;
                    polys[npolys * nvp + 1] = indices[t.y as usize] as u16;
                    polys[npolys * nvp + 2] = indices[t.z as usize] as u16;
                    npolys += 1;
                }
            }

            if npolys == 0 {
                continue;
            }

            // Merge polygons.
            if nvp > 3 {
                loop {
                    // Find best polygons to merge.
                    let mut best_merge_val = 0;
                    let mut best_pa = 0;
                    let mut best_pb = 0;
                    let mut best_ea = 0;
                    let mut best_eb = 0;
                    for j in 0..npolys.saturating_sub(1) {
                        let pj = &polys[(j * nvp)..];
                        for k in (j + 1)..npolys {
                            let pk = &polys[(k * nvp)..];
                            let result = get_poly_merge_value(pj, pk, &mesh, nvp);
                            if let Some(PolyMergeValue {
                                length_squared: v,
                                edge_a: ea,
                                edge_b: eb,
                            }) = result
                                && v > best_merge_val
                            {
                                best_merge_val = v;
                                best_pa = j;
                                best_pb = k;
                                best_ea = ea;
                                best_eb = eb;
                            };
                        }
                    }
                    if best_merge_val > 0 {
                        // Found best, merge.
                        let pa_index = best_pa * nvp;
                        let pb_index = best_pb * nvp;
                        merge_poly_verts(
                            &mut polys,
                            pa_index,
                            pb_index,
                            best_ea,
                            best_eb,
                            temp_poly_index,
                            nvp,
                        );
                        let last_poly = (npolys - 1) * nvp;
                        if pb_index != last_poly {
                            polys.copy_within(last_poly..last_poly + nvp, pb_index);
                        }
                        npolys -= 1;
                    } else {
                        // Could not merge any polygons, stop.
                        break;
                    }
                }
            }

            // Store polygons.
            for j in 0..npolys {
                let q = &polys[j * nvp..];
                if !mesh.valid_face(&q[..count_poly_verts(q, nvp)]) {
                    continue;
                }
                let p = &mut mesh.polygons[mesh.npolys * nvp * 2..];
                p[..nvp].copy_from_slice(&q[..nvp]);
                mesh.regions[mesh.npolys] = cont.region;
                mesh.areas[mesh.npolys] = cont.area;
                mesh.npolys += 1;
                if mesh.npolys > max_tris {
                    // Jan: we are comparing polys with tris. Why? Shouldn't we compare polys with polys?
                    return Err(PolygonNavmeshError::TooManyPolygons {
                        actual: mesh.npolys,
                        max: max_tris,
                    });
                }
            }
        }

        // Remove edge vertices before truncating `mesh.polygons`: `remove_vertex`
        // may leave more triangles than it removed when convexity checks reject
        // a merge, growing `mesh.npolys` past the post-contour count.
        let mut i = 0;
        while i < mesh.nvertices as usize {
            if vflags[i] {
                if !mesh.can_remove_vertex(i as u16) {
                    i += 1;
                    continue;
                }
                mesh.remove_vertex(i as u16, max_tris)?;
                // Remove vertex
                // Note: nverts is already decremented inside removeVertex()!
                // Fixup vertex flags
                let nv = mesh.nvertices as usize;
                if i < nv {
                    vflags.copy_within((i + 1)..=nv, i);
                }
                vflags[nv] = false;
                continue;
            }

            i += 1;
        }
        // Viber patch: ver `cull_overused_edges`.
        let culled = mesh.cull_overused_edges();
        #[cfg(feature = "tracing")]
        if culled > 0 {
            tracing::debug!(
                "rerecast: {culled} polígonos descartados (aresta com mais de dois donos)"
            );
        }
        mesh.polygons.truncate(mesh.npolys * nvp * 2);
        // Calculate adjacency.
        mesh.build_mesh_adjacency()?;

        // Find portal edges
        if self.border_size > 0 {
            let w = self.width;
            let h = self.height;
            for i in 0..mesh.npolys {
                let p = &mut mesh.polygons[i * 2 * nvp..];
                for j in 0..nvp {
                    if p[j] == PolygonNavmesh::NO_INDEX {
                        break;
                    }
                    // Skip connected edges.
                    if p[nvp + j] != PolygonNavmesh::NO_CONNECTION {
                        continue;
                    }
                    let nj = j + 1;
                    let nj = if nj >= nvp || p[nj] == PolygonNavmesh::NO_INDEX {
                        0
                    } else {
                        nj
                    };
                    let va = mesh.vertices[p[j] as usize];
                    let vb = mesh.vertices[p[nj] as usize];
                    if va.x == 0 && vb.x == 0 {
                        p[nvp + j] = RegionId::BORDER_REGION.bits();
                    } else if va.z == h && vb.z == h {
                        p[nvp + j] = RegionId::BORDER_REGION.bits() | 1;
                    } else if va.x == w && vb.x == w {
                        p[nvp + j] = RegionId::BORDER_REGION.bits() | 2;
                    } else if va.z == 0 && vb.z == 0 {
                        p[nvp + j] = RegionId::BORDER_REGION.bits() | 3;
                    }
                }
            }
        }
        // Just allocate the mesh flags array. The user is resposible to fill it.
        mesh.flags = vec![0; mesh.npolys];
        // Jan: Rust's type system makes it impossible for the number of verts and polys to be greater than the max index.

        Ok(mesh.into())
    }
}

#[derive(Debug, Default, Clone)]
struct Edge {
    vert: U16Vec2,
    poly_edge: U16Vec2,
    poly: U16Vec2,
}

impl InternalPolygonNavmesh {
    /// Viber patch: descarta os polígonos que dão a uma aresta mais de dois
    /// donos, e devolve quantos descartou.
    ///
    /// A simplificação dos contornos não é simétrica: os vértices obrigatórios
    /// de uma região são os pontos onde muda o VIZINHO dela, por isso duas
    /// regiões que partilham uma parede podem simplificá-la a partir de cordas
    /// diferentes e uma delas fica com um vértice a mais no troço. Quando esse
    /// vértice é convexo, a orelha `(vizinho, vértice, vizinho)` e o polígono do
    /// outro lado da corda ficam ambos com ela — e o consumidor que liga
    /// polígonos por arestas iguais (o `landmass`) conta três donos e recusa a
    /// malha inteira. A orelha é uma fenda de até `max_error` de profundidade
    /// que já está coberta pelo polígono do vizinho: descartá-la não abre
    /// buraco. De cada aresta em excesso sai o polígono de menor área.
    fn cull_overused_edges(&mut self) -> usize {
        let nvp = self.max_vertices_per_polygon as usize;
        let mut culled = 0;
        loop {
            let mut edges: Vec<(u16, u16, u16)> = Vec::with_capacity(self.npolys * nvp);
            for i in 0..self.npolys {
                let p = &self.polygons[i * nvp * 2..];
                let nv = count_poly_verts(p, nvp);
                for j in 0..nv {
                    let (a, b) = (p[j], p[(j + 1) % nv]);
                    edges.push((a.min(b), a.max(b), i as u16));
                }
            }
            edges.sort_unstable();
            let mut drop = vec![false; self.npolys];
            let mut dropped = 0;
            let mut k = 0;
            while k < edges.len() {
                let mut end = k + 1;
                while end < edges.len() && edges[end].0 == edges[k].0 && edges[end].1 == edges[k].1
                {
                    end += 1;
                }
                let mut owners: Vec<(i64, usize)> = edges[k..end]
                    .iter()
                    .map(|&(_, _, i)| {
                        let i = usize::from(i);
                        (self.polygon_area(i), i)
                    })
                    .collect();
                owners.sort_unstable();
                let mut excess = owners.iter().filter(|(_, i)| !drop[*i]).count().saturating_sub(2);
                for (_, i) in owners {
                    if excess == 0 {
                        break;
                    }
                    if !drop[i] {
                        drop[i] = true;
                        excess -= 1;
                        dropped += 1;
                    }
                }
                k = end;
            }
            if dropped == 0 {
                return culled;
            }
            for i in (0..self.npolys).rev() {
                if !drop[i] {
                    continue;
                }
                let last = (self.npolys - 1) * nvp * 2;
                if i * nvp * 2 != last {
                    self.polygons.copy_within(last..last + nvp * 2, i * nvp * 2);
                    self.regions[i] = self.regions[self.npolys - 1];
                    self.areas[i] = self.areas[self.npolys - 1];
                }
                self.npolys -= 1;
            }
            culled += dropped;
        }
    }

    /// Área com sinal de um polígono, em células ao quadrado (o sinal é o da
    /// orientação do mesh; só a magnitude interessa aqui).
    fn polygon_area(&self, i: usize) -> i64 {
        let nvp = self.max_vertices_per_polygon as usize;
        let p = &self.polygons[i * nvp * 2..];
        let nv = count_poly_verts(p, nvp);
        let point = |j: usize| {
            let v = self.vertices[usize::from(p[j % nv])];
            (i64::from(v.x), i64::from(v.z))
        };
        let (x0, z0) = point(0);
        let mut area = 0;
        for j in 1..nv.saturating_sub(1) {
            let (x1, z1) = point(j);
            let (x2, z2) = point(j + 1);
            area += (x1 - x0) * (z2 - z0) - (x2 - x0) * (z1 - z0);
        }
        area.abs()
    }

    fn build_mesh_adjacency(&mut self) -> Result<(), PolygonNavmeshError> {
        let nvp = self.max_vertices_per_polygon as usize;
        // Based on code by Eric Lengyel from:
        // https://web.archive.org/web/20080704083314/http://www.terathon.com/code/edges.php
        let max_edge_count = self.npolys * nvp;
        let mut first_edge =
            vec![PolygonNavmesh::NO_CONNECTION; self.nvertices as usize + max_edge_count];
        let next_edge_index = self.nvertices as usize;
        let mut edge_count = 0;
        let mut edges = vec![Edge::default(); max_edge_count];
        for i in 0..self.npolys {
            let t = &self.polygons[i * nvp * 2..];
            for j in 0..nvp {
                if t[j] == PolygonNavmesh::NO_INDEX {
                    break;
                }
                let v0 = t[j];
                let v1 = if j + 1 >= nvp || t[j + 1] == PolygonNavmesh::NO_INDEX {
                    t[0]
                } else {
                    t[j + 1]
                };
                if v0 < v1 {
                    let edge = &mut edges[edge_count];
                    edge.vert.x = v0;
                    edge.vert.y = v1;
                    edge.poly.x = i as u16;
                    edge.poly_edge.x = j as u16;
                    edge.poly.y = i as u16;
                    edge.poly_edge.y = 0;
                    // Insert edge
                    first_edge[next_edge_index + edge_count] = first_edge[v0 as usize];
                    first_edge[v0 as usize] = edge_count as u16;
                    edge_count += 1;
                }
            }
        }
        for i in 0..self.npolys {
            let t = &self.polygons[i * nvp * 2..];
            let nv = count_poly_verts(t, nvp);
            for j in 0..nv {
                if t[j] == PolygonNavmesh::NO_INDEX {
                    break;
                }
                let v0 = t[j];
                let v1 = if j + 1 >= nvp || t[j + 1] == PolygonNavmesh::NO_INDEX {
                    t[0]
                } else {
                    t[j + 1]
                };
                if v0 > v1 {
                    let mut e = first_edge[v1 as usize];
                    while e != PolygonNavmesh::NO_CONNECTION {
                        let edge = &mut edges[e as usize];
                        if edge.vert.y == v0 && edge.poly.x == edge.poly.y {
                            edge.poly.y = i as u16;
                            edge.poly_edge.y = j as u16;
                            break;
                        }
                        e = first_edge[next_edge_index + e as usize];
                    }
                }
            }
        }

        // Store adjacency
        for e in edges.iter().take(edge_count) {
            if e.poly.x != e.poly.y {
                {
                    let p0 = &mut self.polygons[e.poly.x as usize * nvp * 2..];
                    p0[nvp + e.poly_edge.x as usize] = e.poly.y;
                }
                let p1 = &mut self.polygons[e.poly.y as usize * nvp * 2..];
                p1[nvp + e.poly_edge.y as usize] = e.poly.x;
            }
        }
        Ok(())
    }

    fn remove_vertex(&mut self, rem: u16, max_tris: usize) -> Result<(), PolygonNavmeshError> {
        let nvp = self.max_vertices_per_polygon as usize;

        // Count number of polygons to remove.
        let mut num_removed_verts = 0;
        for i in 0..self.npolys {
            let p = &self.polygons[i * nvp * 2..];
            let nv = count_poly_verts(p, nvp);
            for pj in p.iter().take(nv) {
                if *pj == rem {
                    num_removed_verts += 1;
                }
            }
        }

        let mut nedges = 0;
        // Format: [polygon1, polygon2, region, area]
        #[derive(Debug, Clone, Default)]
        struct Edge {
            polygon1: u16,
            polygon2: u16,
            region: RegionId,
            area: AreaType,
        }
        let mut edges = vec![Edge::default(); num_removed_verts * nvp];
        let mut nhole = 0;
        let mut hole = vec![0; num_removed_verts * nvp];
        let mut nhreg = 0;
        let mut hreg = vec![RegionId::default(); num_removed_verts * nvp];
        let mut nharea = 0;
        let mut harea = vec![AreaType::default(); num_removed_verts * nvp];

        let mut i = 0;
        while i < self.npolys {
            let i1 = i * nvp * 2;
            {
                // Scope p immutable in this scope
                let p = &self.polygons[i1..];
                let nv = count_poly_verts(p, nvp);
                let has_rem = (0..nv).any(|j| p[j] == rem);
                if !has_rem {
                    i += 1;
                    continue;
                }
                // Collect edges which does not touch the removed vertex.
                for j in 0..nv {
                    let k = prev(j, nv);
                    if p[j] != rem && p[k] != rem {
                        let e = &mut edges[nedges];
                        e.polygon1 = p[k];
                        e.polygon2 = p[j];
                        e.region = self.regions[i];
                        e.area = self.areas[i];
                        nedges += 1;
                    }
                }
            }
            // Remove the polygon.
            let i2 = (self.npolys - 1) * nvp * 2;
            if i1 != i2 {
                self.polygons.copy_within(i2..(i2 + nvp), i1);
            }
            self.polygons[i1 + nvp..(i1 + 2 * nvp)].fill(u16::MAX);
            self.regions[i] = self.regions[self.npolys - 1];
            self.areas[i] = self.areas[self.npolys - 1];
            self.npolys -= 1;
        }

        // Remove vertex.
        for i in rem..self.nvertices - 1 {
            let i = i as usize;
            self.vertices[i] = self.vertices[i + 1];
        }
        self.nvertices -= 1;

        // Adjust indices to match the removed vertex layout.
        for i in 0..self.npolys {
            let p = &mut self.polygons[i * nvp * 2..];
            let nv = count_poly_verts(p, nvp);
            for pj in p.iter_mut().take(nv) {
                if *pj > rem {
                    *pj -= 1;
                }
            }
        }
        for edge in edges.iter_mut().take(nedges) {
            if edge.polygon1 > rem {
                edge.polygon1 -= 1;
            }
            if edge.polygon2 > rem {
                edge.polygon2 -= 1;
            }
        }

        if nedges == 0 {
            return Ok(());
        }

        // Start with one vertex, keep appending connected
        // segments to the start and end of the hole.
        push_back(edges[0].polygon1, &mut hole, &mut nhole);
        push_back(edges[0].region, &mut hreg, &mut nhreg);
        push_back(edges[0].area, &mut harea, &mut nharea);

        while nedges != 0 {
            let mut match_ = false;
            let mut i = 0;
            while i < nedges {
                let edge = edges[i].clone();
                let ea = edge.polygon1;
                let eb = edge.polygon2;
                let r = edge.region;
                let a = edge.area;
                let mut add = false;
                if hole[0] == eb {
                    // The segment matches the beginning of the hole boundary.
                    push_front(ea, &mut hole, &mut nhole);
                    push_front(r, &mut hreg, &mut nhreg);
                    push_front(a, &mut harea, &mut nharea);
                    add = true;
                } else if hole[nhole - 1] == ea {
                    // The segment matches the end of the hole boundary.
                    push_back(eb, &mut hole, &mut nhole);
                    push_back(r, &mut hreg, &mut nhreg);
                    push_back(a, &mut harea, &mut nharea);
                    add = true;
                }
                if add {
                    // The edge segment was added, remove it.
                    edges[i] = edges[nedges - 1].clone();
                    nedges -= 1;
                    match_ = true;
                } else {
                    i += 1;
                }
            }
            if !match_ {
                break;
            }
        }
        let mut tris = vec![U16Vec3::default(); nhole];
        let mut tverts = vec![(U16Vec3::default(), 0); nhole];
        let mut thole = vec![0; nhole];

        // Generate temp vertex array for triangulation.
        for i in 0..nhole {
            let pi = hole[i] as usize;
            tverts[i].0 = self.vertices[pi];
            thole[i] = i;
        }

        // Triangulate the hole.
        // Jan: we treat errors here as a hard error instead of printing a warning.
        let ntris = triangulate(nhole, &tverts, &mut thole, &mut tris)?;

        // Merge the hole triangles back to polygons.
        let mut polys = vec![0; (ntris + 1) * nvp];
        let mut pregs = vec![RegionId::default(); ntris];
        let mut pareas = vec![AreaType::default(); ntris];
        let tmp_poly_index = ntris * nvp;

        // Build initial polygons.
        let mut npolys = 0;
        polys[..ntris * nvp].fill(u16::MAX);
        for t in tris.iter().take(ntris) {
            // Viber patch: ver `wound_right`.
            if t.x != t.y && t.x != t.z && t.y != t.z && wound_right(&tverts, *t) {
                let t_x = t.x as usize;
                let t_y = t.y as usize;
                let t_z = t.z as usize;
                polys[npolys * nvp] = hole[t_x];
                polys[npolys * nvp + 1] = hole[t_y];
                polys[npolys * nvp + 2] = hole[t_z];

                // If this polygon covers multiple region types then
                // mark it as such
                if hreg[t_x] != hreg[t_y] || hreg[t_y] != hreg[t_z] {
                    pregs[npolys] = RegionId::NONE;
                } else {
                    pregs[npolys] = hreg[t_x];
                }
                pareas[npolys] = harea[t_x];
                npolys += 1;
            }
        }
        if npolys == 0 {
            return Ok(());
        }

        // Merge polygons.
        if nvp > 3 {
            loop {
                // Find best polygons to merge.
                let mut best_merge_val = 0;
                let mut best_pa = 0;
                let mut best_pb = 0;
                let mut best_ea = 0;
                let mut best_eb = 0;
                for j in 0..npolys.saturating_sub(1) {
                    let pj = &polys[j * nvp..];
                    for k in (j + 1)..npolys {
                        let pk = &polys[k * nvp..];
                        let value = get_poly_merge_value(pj, pk, self, nvp);
                        if let Some(value) = value
                            && value.length_squared > best_merge_val
                        {
                            best_merge_val = value.length_squared;
                            best_pa = j;
                            best_pb = k;
                            best_ea = value.edge_a;
                            best_eb = value.edge_b;
                        }
                    }
                }

                if best_merge_val > 0 {
                    // Found best, merge.
                    let pa_index = best_pa * nvp;
                    let pb_index = best_pb * nvp;
                    merge_poly_verts(
                        &mut polys,
                        pa_index,
                        pb_index,
                        best_ea,
                        best_eb,
                        tmp_poly_index,
                        nvp,
                    );
                    if pregs[best_pa] != pregs[best_pb] {
                        pregs[best_pa] = RegionId::NONE;
                    }

                    let last_index = (npolys - 1) * nvp;
                    if pb_index != last_index {
                        polys.copy_within(last_index..last_index + nvp, pb_index);
                    }
                    pregs[best_pb] = pregs[npolys - 1];
                    pareas[best_pb] = pareas[npolys - 1];
                    npolys -= 1;
                } else {
                    // Cound not merge any polygons, stop.
                    break;
                }
            }
        }

        // Store polygons.
        for i in 0..npolys {
            if self.npolys >= max_tris || (self.npolys + 1) * nvp * 2 > self.polygons.len() {
                break;
            }
            let face = &polys[i * nvp..];
            if !self.valid_face(&face[..count_poly_verts(face, nvp)]) {
                continue;
            }
            let p = &mut self.polygons[self.npolys * nvp * 2..self.npolys * nvp * 2 + nvp * 2];
            p[..nvp * 2].fill(u16::MAX);
            for j in 0..nvp {
                p[j] = polys[i * nvp + j];
            }
            self.regions[self.npolys] = pregs[i];
            self.areas[self.npolys] = pareas[i];
            self.npolys += 1;
            if self.npolys > max_tris {
                return Err(PolygonNavmeshError::TooManyPolygons {
                    actual: self.npolys,
                    max: max_tris,
                });
            }
        }

        Ok(())
    }

    fn can_remove_vertex(&self, rem: u16) -> bool {
        let nvp = self.max_vertices_per_polygon as usize;

        // Count number of polygons to remove.
        let mut num_touched_verts = 0;
        let mut num_remaining_edges = 0;
        for i in 0..self.npolys {
            let p = &self.polygons[i * nvp * 2..];
            let nv = count_poly_verts(p, nvp);
            let mut num_removed = 0;
            let mut num_verts = 0;
            for pj in p.iter().take(nv) {
                if *pj == rem {
                    num_touched_verts += 1;
                    num_removed += 1;
                }
                num_verts += 1;
            }
            if num_removed != 0 {
                num_remaining_edges += num_verts - (num_removed + 1);
            }
        }

        // There would be too few edges remaining to create a polygon.
        // This can happen for example when a tip of a triangle is marked
        // as deletion, but there are no other polys that share the vertex.
        // In this case, the vertex should not be removed.
        if num_remaining_edges <= 2 {
            return false;
        }
        // Find edges which share the removed vertex.
        let max_edges = num_touched_verts * 2;
        let mut nedges = 0;
        // Format: [poly1, poly2, vertex share count]
        let mut edges = vec![U16Vec3::ZERO; max_edges];
        for i in 0..self.npolys {
            let p = &self.polygons[i * nvp * 2..];
            let nv = count_poly_verts(p, nvp);

            // Collect edges which touches the removed vertex.
            for j in 0..nv {
                let k = prev(j, nv);
                if !(p[j] == rem || p[k] == rem) {
                    continue;
                }
                // Arrange edge so that a=rem.
                let a = p[j];
                let b = p[k];
                let (a, b) = if b != rem { (a, b) } else { (b, a) };

                // Check if the edge exists
                let mut exists = false;
                for e in edges.iter_mut().take(nedges) {
                    if e[1] == b {
                        // Exists, increment vertex share count.
                        e[2] += 1;
                        exists = true;
                    }
                }
                // Add new edge
                if !exists {
                    let e = &mut edges[nedges];
                    e[0] = a;
                    e[1] = b;
                    e[2] = 1;
                    nedges += 1;
                }
            }
        }

        // There should be no more than 2 open edges.
        // This catches the case that two non-adjacent polygons
        // share the removed vertex. In that case, do not remove the vertex.
        let num_open_edges = edges.iter().take(nedges).filter(|e| e[2] < 2).count();
        num_open_edges <= 2
    }
}

fn push_back<T>(value: T, vec: &mut [T], index: &mut usize) {
    vec[*index] = value;
    *index += 1;
}

fn push_front<T: Clone>(value: T, vec: &mut [T], index: &mut usize) {
    *index += 1;
    for i in (1..*index).rev() {
        vec[i] = vec[i - 1].clone();
    }
    vec[0] = value;
}

// Jan: signature changed to align with the borrow checker :)
fn merge_poly_verts(
    polys: &mut [u16],
    pa_index: usize,
    pb_index: usize,
    ea: usize,
    eb: usize,
    tmp_index: usize,
    nvp: usize,
) {
    let na = count_poly_verts(&polys[pa_index..], nvp);
    let nb = count_poly_verts(&polys[pb_index..], nvp);

    // Merge polygons.
    polys[tmp_index..tmp_index + nvp].fill(u16::MAX);
    let mut n = 0;
    // Add pa
    for i in 0..na - 1 {
        polys[tmp_index + n] = polys[pa_index + (ea + 1 + i) % na];
        n += 1;
    }
    // Add pb
    for i in 0..nb - 1 {
        polys[tmp_index + n] = polys[pb_index + (eb + 1 + i) % nb];
        n += 1;
    }

    // Implicit assumption of the original code
    polys.copy_within(tmp_index..tmp_index + nvp, pa_index);
}

impl InternalPolygonNavmesh {
    fn valid_face(&self, indices: &[u16]) -> bool {
        self.convex_world_ring(indices.len(), |i| indices[i])
    }

    // Test the same XZ coordinates the consumer receives after dequantization.
    // A straight boundary is valid; a zero-length edge or a reversal is not.
    fn convex_world_ring(&self, len: usize, index: impl Fn(usize) -> u16) -> bool {
        if len < 3 {
            return false;
        }
        let point = |i| {
            self.aabb.min.xz()
                + self.vertices[usize::from(index(i))].as_vec3().xz() * self.cell_size
        };
        let mut prev = point(len - 2);
        let mut center = point(len - 1);
        (0..len).all(|i| {
            let next = point(i);
            let left = prev - center;
            let right = next - center;
            let turn = left.perp_dot(right);
            prev = center;
            center = next;
            turn > 0.0 || (turn == 0.0 && left.dot(right) < 0.0)
        })
    }
}

fn get_poly_merge_value(
    pa: &[u16],
    pb: &[u16],
    mesh: &InternalPolygonNavmesh,
    nvp: usize,
) -> Option<PolyMergeValue> {
    let verts = &mesh.vertices;
    let na = count_poly_verts(pa, nvp);
    let nb = count_poly_verts(pb, nvp);

    // If the merged polygon would be too big, do not merge.
    if na + nb - 2 > nvp {
        return None;
    }

    // Check if the polygons share an edge.
    let mut ea = None;
    let mut eb = None;

    for i in 0..na {
        let va0 = pa[i];
        let va1 = pa[next(i, na)];
        for j in 0..nb {
            let vb0 = pb[j];
            let vb1 = pb[next(j, nb)];
            if va0 == vb1 && va1 == vb0 {
                ea = Some(i);
                eb = Some(j);
                break;
            }
        }
    }

    // No common edge, cannot merge.
    let (ea, eb) = (ea?, eb?);

    // Check to see if the merged polygon would be convex.
    let mut va = pa[(ea + na - 1) % na] as usize;
    let mut vb = pa[ea] as usize;
    let mut vc = pb[(eb + 2) % nb] as usize;
    if !uleft(verts[va], verts[vb], verts[vc]) {
        return None;
    }

    va = pb[(eb + nb - 1) % nb] as usize;
    vb = pb[eb] as usize;
    vc = pa[(ea + 2) % na] as usize;
    if !uleft(verts[va], verts[vb], verts[vc]) {
        return None;
    };

    // A collinear ear can preserve a shared boundary vertex after merging,
    // but converting its grid coordinates to f32 can bend that edge inward.
    let merged_vertex = |i: usize| {
        if i < na - 1 {
            pa[(ea + 1 + i) % na]
        } else {
            pb[(eb + 1 + i - (na - 1)) % nb]
        }
    };
    if !mesh.convex_world_ring(na + nb - 2, merged_vertex) {
        return None;
    }

    va = pa[ea] as usize;
    vb = pa[(ea + 1) % na] as usize;

    let d = verts[va].as_ivec3() - verts[vb].as_ivec3();
    let length_squared = d.xz().length_squared() as u32;
    Some(PolyMergeValue {
        length_squared,
        edge_a: ea,
        edge_b: eb,
    })
}

#[inline]
/// Viber patch: o triângulo tem a orientação dos polígonos convexos do mesh
/// (`uleft`: área < 0). A triangulação recuperada de um contorno dobrado (o
/// diagonal "loose" ou o resultado parcial do patch 1) pode produzir um
/// triângulo invertido ou de área zero; fundido com os vizinhos, dava um
/// polígono côncavo/no sentido errado e o `landmass` rejeitava a navmesh do
/// tile INTEIRO (`The polygon at index N is concave or has edges in clockwise
/// order`). Deitá-lo fora deixa um buraco do tamanho do sliver.
fn wound_right<T>(verts: &[(U16Vec3, T)], t: U16Vec3) -> bool {
    match (
        verts.get(t.x as usize),
        verts.get(t.y as usize),
        verts.get(t.z as usize),
    ) {
        (Some(a), Some(b), Some(c)) => area2(a.0, b.0, c.0) <= 0,
        _ => false,
    }
}

fn uleft(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    let cross = (b.x as i32 - a.x as i32) * (c.z as i32 - a.z as i32)
        - (c.x as i32 - a.x as i32) * (b.z as i32 - a.z as i32);
    cross < 0
}

fn count_poly_verts(p: &[u16], nvp: usize) -> usize {
    p.iter()
        .take(nvp)
        .position(|p| *p == PolygonNavmesh::NO_INDEX)
        .unwrap_or(nvp)
}

#[derive(Debug)]
struct PolyMergeValue {
    length_squared: u32,
    edge_a: usize,
    edge_b: usize,
}

fn add_vertex(
    vertex: U16Vec3,
    verts: &mut [U16Vec3],
    first_vert: &mut [Option<u16>],
    next_vert: &mut [Option<u16>],
    nverts: &mut u16,
) -> u16 {
    let bucket = compute_vertex_hash(u16vec3(vertex.x, 0, vertex.z));
    let mut i_iter = first_vert[bucket];

    while let Some(i) = i_iter {
        let v = verts[i as usize];
        if v.x == vertex.x && (v.y as i32 - vertex.y as i32).abs() <= 2 && v.z == vertex.z {
            return i;
        }
        i_iter = next_vert[i as usize];
    }

    // Could not find, create new.
    let i = *nverts;
    *nverts += 1;
    verts[i as usize] = vertex;
    next_vert[i as usize] = first_vert[bucket];
    first_vert[bucket] = Some(i);

    i
}

fn compute_vertex_hash(vertex: U16Vec3) -> usize {
    let h = uvec3(
        0x8da6b343, // Large multiplicative constants;
        0xd8163841, // here arbitrarily chosen primes
        0xcb1ab31f,
    );
    let v = vertex.as_uvec3();
    let n = h[0]
        .wrapping_mul(v[0])
        .wrapping_add(h[1].wrapping_mul(v[1]))
        .wrapping_add(h[2].wrapping_mul(v[2]));
    n as usize & (VERTEX_BUCKET_COUNT - 1)
}

const VERTEX_BUCKET_COUNT: usize = 1 << 12;

fn triangulate(
    mut n: usize,
    verts: &[(U16Vec3, u32)],
    indices: &mut [usize],
    tris: &mut [U16Vec3],
) -> Result<usize, PolygonNavmeshError> {
    let mut ntris = 0;

    // The last bit of the index is used to indicate if the vertex can be removed.
    for i in 0..n {
        let i1 = next(i, n);
        let i2 = next(i1, n);
        if is_diagonal(i, i2, n, verts, indices) {
            indices[i1] |= CAN_REMOVE;
        }
    }
    while n > 3 {
        let mut min_len = None;
        let mut mini = None;
        for i in 0..n {
            let i1 = next(i, n);
            if (indices[i1] & CAN_REMOVE) != 0 {
                let p0 = verts[indices[i] & INDEX_MASK].0;
                let p2 = verts[indices[next(i1, n)] & INDEX_MASK].0;

                let d = p2.as_ivec3() - p0.as_ivec3();
                // Viber patch: `as u16` wrapped any diagonal longer than 255
                // cells and made the ear picker choose arbitrary ears.
                let len = d.xz().length_squared() as u32;
                if min_len.is_none_or(|min| len < min) {
                    min_len = Some(len);
                    mini = Some(i);
                }
            }
        }
        if mini.is_none() {
            // We might get here because the contour has overlapping segments, like this:
            //
            //  A o-o=====o---o B
            //   /  |C   D|    \.
            //  o   o     o     o
            //  :   :     :     :
            // We'll try to recover by loosing up the inCone test a bit so that a diagonal
            // like A-B or C-D can be found and we can continue.
            min_len = None;
            for i in 0..n {
                let i1 = next(i, n);
                let i2 = next(i1, n);
                if is_diagonal_loose(i, i2, n, verts, indices) {
                    let p0 = verts[indices[i] & INDEX_MASK].0;
                    let p2 = verts[indices[next(i2, n)] & INDEX_MASK].0;
                    let d = p2.as_ivec3() - p0.as_ivec3();
                    // Viber patch: `as u16` wrapped any diagonal longer than
                    // 255 cells and made the ear picker choose arbitrary ears.
                    let len = d.xz().length_squared() as u32;
                    if min_len.is_none_or(|min| len < min) {
                        min_len = Some(len);
                        mini = Some(i);
                    }
                }
            }
        }

        let Some(mini) = mini else {
            // The contour is messed up. This sometimes happens
            // if the contour simplification is too aggressive.
            //
            // Viber patch: Recast (C++) returns the triangles built so far
            // (`return -ntris`) and `rcBuildPolyMesh` only logs a warning —
            // one folded contour leaves a small hole in the navmesh. Upstream
            // rerecast turns it into a hard error, which throws away the
            // WHOLE navmesh for one bad contour (a 256 m tile in the Viber
            // simple-rpg failed deterministically this way).
            #[cfg(feature = "tracing")]
            tracing::warn!(
                "rerecast: bad triangulation ({n} contour vertices left) — keeping {ntris} triangles"
            );
            return Ok(ntris);
        };

        let mut i = mini;
        let mut i1 = next(i, n);
        let i2 = next(i1, n);

        tris[ntris].x = (indices[i] & INDEX_MASK) as u16;
        tris[ntris].y = (indices[i1] & INDEX_MASK) as u16;
        tris[ntris].z = (indices[i2] & INDEX_MASK) as u16;
        ntris += 1;

        // Removes P[i1] by copying P[i+1]...P[n-1] left one index.
        n -= 1;
        for k in i1..n {
            indices[k] = indices[k + 1];
        }

        if i1 >= n {
            i1 = 0;
        }
        i = prev(i1, n);
        // Update diagonal flags.
        if is_diagonal(prev(i, n), i1, n, verts, indices) {
            indices[i] |= CAN_REMOVE;
        } else {
            indices[i] &= INDEX_MASK;
        }

        if is_diagonal(i, next(i1, n), n, verts, indices) {
            indices[i1] |= CAN_REMOVE;
        } else {
            indices[i1] &= INDEX_MASK;
        }
    }
    // Append the remaining triangle.
    tris[ntris].x = (indices[0] & INDEX_MASK) as u16;
    tris[ntris].y = (indices[1] & INDEX_MASK) as u16;
    tris[ntris].z = (indices[2] & INDEX_MASK) as u16;
    ntris += 1;

    Ok(ntris)
}

const CAN_REMOVE: usize = 0x80000000;

/// Returns true iff (v_i, v_j) is a proper internal diagonal of P.
fn is_diagonal(i: usize, j: usize, n: usize, verts: &[(U16Vec3, u32)], indices: &[usize]) -> bool {
    in_cone(i, j, n, verts, indices) && is_diagonal_internal_or_external(i, j, n, verts, indices)
}

/// Returns true iff the diagonal (i,j) is strictly internal to the
/// polygon P in the neighborhood of the i endpoint.
fn in_cone(i: usize, j: usize, n: usize, verts: &[(U16Vec3, u32)], indices: &[usize]) -> bool {
    let pi = verts[indices[i] & INDEX_MASK].0;
    let pj = verts[indices[j] & INDEX_MASK].0;
    let pi1 = verts[indices[next(i, n)] & INDEX_MASK].0;
    let pin1 = verts[indices[prev(i, n)] & INDEX_MASK].0;

    // If P[i] is a convex vertex [ i+1 left or on (i-1,i) ].
    if is_left_on(pin1, pi, pi1) {
        is_left(pi, pj, pin1) && is_left(pj, pi, pi1)
    } else {
        // Assume (i-1,i,i+1) not collinear.
        // else P[i] is reflex.
        !(is_left_on(pi, pj, pi1) && is_left_on(pj, pi, pin1))
    }
}

#[inline]
fn is_left_on(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    area2(a, b, c) <= 0
}

/// Returns true iff c is strictly to the left of the directed line through a to b.
#[inline]
fn is_left(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    area2(a, b, c) < 0
}

#[inline]
fn area2(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> i32 {
    let a = a.as_ivec3();
    let b = b.as_ivec3();
    let c = c.as_ivec3();
    (b.x - a.x) * (c.z - a.z) - (c.x - a.x) * (b.z - a.z)
}

// Returns T iff (v_i, v_j) is a proper internal *or* external
// diagonal of P, *ignoring edges incident to v_i and v_j*.
fn is_diagonal_internal_or_external(
    i: usize,
    j: usize,
    n: usize,
    verts: &[(U16Vec3, u32)],
    indices: &[usize],
) -> bool {
    let d0 = verts[indices[i] & INDEX_MASK].0;
    let d1 = verts[indices[j] & INDEX_MASK].0;

    // For each edge (k,k+1) of P
    for k in 0..n {
        let k1 = next(k, n);
        // Skip edges incident to i or j
        if !((k == i) || (k1 == i) || (k == j) || (k1 == j)) {
            let p0 = verts[indices[k] & INDEX_MASK].0;
            let p1 = verts[indices[k1] & INDEX_MASK].0;
            if vequal(d0, p0) || vequal(d1, p0) || vequal(d0, p1) || vequal(d1, p1) {
                continue;
            }
            if intersect(d0, d1, p0, p1) {
                return false;
            }
        }
    }
    true
}

const INDEX_MASK: usize = 0x0fffffff;

#[inline]
fn vequal(a: U16Vec3, b: U16Vec3) -> bool {
    a.xz() == b.xz()
}

/// Returns true iff segments ab and cd intersect, properly or improperly.
#[inline]
fn intersect(a: U16Vec3, b: U16Vec3, c: U16Vec3, d: U16Vec3) -> bool {
    if intersect_prop(a, b, c, d) {
        return true;
    }
    between(a, b, c) || between(a, b, d) || between(c, d, a) || between(c, d, b)
}

/// Returns true iff ab properly intersects cd: they share
/// a point interior to both segments.  The properness of the
/// intersection is ensured by using strict leftness.
#[inline]
fn intersect_prop(a: U16Vec3, b: U16Vec3, c: U16Vec3, d: U16Vec3) -> bool {
    // Eliminate improper cases.
    if collinear(a, b, c) || collinear(a, b, d) || collinear(c, d, a) || collinear(c, d, b) {
        return false;
    }
    (left(a, b, c) ^ left(a, b, d)) && (left(c, d, a) ^ left(c, d, b))
}

#[inline]
fn collinear(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    area2(a, b, c) == 0
}

/// Returns true iff c is strictly to the left of the directed
/// line through a to b.
#[inline]
fn left(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    area2(a, b, c) < 0
}

#[inline]
fn left_on(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    area2(a, b, c) <= 0
}

/// Returns T iff (a,b,c) are collinear and point c lies
/// on the closed segment ab.
#[inline]
fn between(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    if !collinear(a, b, c) {
        return false;
    }
    // If ab not vertical, check betweenness on x; else on z.
    if a.x != b.x {
        (a.x <= c.x && c.x <= b.x) || (a.x >= c.x && c.x >= b.x)
    } else {
        (a.z <= c.z && c.z <= b.z) || (a.z >= c.z && c.z >= b.z)
    }
}

fn is_diagonal_loose(
    i: usize,
    j: usize,
    n: usize,
    verts: &[(U16Vec3, u32)],
    indices: &[usize],
) -> bool {
    in_cone_loose(i, j, n, verts, indices)
        && is_diagonal_internal_or_external_loose(i, j, n, verts, indices)
}

fn in_cone_loose(
    i: usize,
    j: usize,
    n: usize,
    verts: &[(U16Vec3, u32)],
    indices: &[usize],
) -> bool {
    let pi = verts[indices[i] & INDEX_MASK].0;
    let pj = verts[indices[j] & INDEX_MASK].0;
    let pi1 = verts[indices[next(i, n)] & INDEX_MASK].0;
    let pin1 = verts[indices[prev(i, n)] & INDEX_MASK].0;

    // If P[i] is a convex vertex [ i+1 left or on (i-1,i) ].
    if left_on(pin1, pi, pi1) {
        left_on(pi, pj, pin1) && left_on(pj, pi, pi1)
    } else {
        !(left_on(pi, pj, pi1) && left_on(pj, pi, pin1))
    }
}

fn is_diagonal_internal_or_external_loose(
    i: usize,
    j: usize,
    n: usize,
    verts: &[(U16Vec3, u32)],
    indices: &[usize],
) -> bool {
    let d0 = verts[indices[i] & INDEX_MASK].0;
    let d1 = verts[indices[j] & INDEX_MASK].0;

    // For each edge (k,k+1) of P
    for k in 0..n {
        let k1 = next(k, n);
        // Skip edges incident to i or j
        if !(k == i || k1 == i || k == j || k1 == j) {
            let p0 = verts[indices[k] & INDEX_MASK].0;
            let p1 = verts[indices[k1] & INDEX_MASK].0;
            if vequal(d0, p0) || vequal(d1, p0) || vequal(d0, p1) || vequal(d1, p1) {
                continue;
            }
            if intersect_prop(d0, d1, p0, p1) {
                return false;
            }
        }
    }
    true
}

#[derive(Error, Debug)]
pub enum PolygonNavmeshError {
    #[error("Too many vertices: {actual} > {max}")]
    TooManyVertices { actual: usize, max: usize },
    #[error("Too many polygons: {actual} > {max}")]
    TooManyPolygons { actual: usize, max: usize },
    #[error(
        "Invalid contour. This sometimes happens if the contour simplification is too aggressive."
    )]
    InvalidContour,
}

#[cfg(test)]
mod viber_patch_tests {
    use super::*;

    #[test]
    fn collinear_ears_only_survive_as_straight_boundary_vertices() {
        let mut mesh = InternalPolygonNavmesh::default();
        mesh.cell_size = 0.5;
        mesh.vertices = [(0, 0), (0, 4), (4, 4), (4, 0), (2, 0)]
            .map(|(x, z)| U16Vec3::new(x, 0, z))
            .to_vec();
        assert!(!mesh.valid_face(&[0, 4, 3]));
        assert!(mesh.valid_face(&[0, 1, 2, 3, 4]));
        assert!(!mesh.valid_face(&[0, 1, 2, 4, 3]));
    }

    #[test]
    fn polygon_build_keeps_walkable_faces_beside_degenerate_contours() {
        let contour = |points: &[(u16, u16)]| crate::Contour {
            vertices: points
                .iter()
                .map(|&(x, z)| (U16Vec3::new(x, 0, z), 0))
                .collect(),
            area: AreaType::DEFAULT_WALKABLE,
            ..Default::default()
        };
        let contours = ContourSet {
            // The middle point lies exactly on the grid edge but rounds
            // inward in world space near the reported (-13, 74) tile.
            contours: vec![
                contour(&[(100, 343), (103, 345), (106, 347), (106, 340)]),
                contour(&[(0, 0), (0, 1), (0, 2)]),
            ],
            aabb: Aabb3d {
                min: glam::Vec3::new(-141., 0., -54.),
                ..Default::default()
            },
            cell_size: 1. / 6.,
            cell_height: 0.1,
            ..Default::default()
        };
        let mesh = contours.into_polygon_mesh(6).unwrap();
        let mut area = 0;
        for poly in mesh.polygons() {
            let vertices: Vec<_> = poly.map(|i| mesh.vertices[usize::from(i)]).collect();
            for i in 0..vertices.len() {
                let p = |j: usize| {
                    mesh.aabb.min.xz()
                        + vertices[j % vertices.len()].as_vec3().xz() * mesh.cell_size
                };
                let left = p(i) - p(i + 1);
                let right = p(i + 2) - p(i + 1);
                assert!(
                    left.perp_dot(right) > 0.
                        || (left.perp_dot(right) == 0. && left.dot(right) < 0.)
                );
            }
            for pair in vertices[1..].windows(2) {
                area -= area2(vertices[0], pair[0], pair[1]);
            }
        }
        assert_eq!(area, 42, "all non-degenerate floor area survives");
    }

    #[test]
    fn only_inverted_triangles_are_dropped() {
        let v = |x, z| (U16Vec3::new(x, 0, z), 0u32);
        let verts = [v(0, 0), v(0, 4), v(4, 0), v(8, 0)];
        // (0,1,2): área2 = (0)(0) - (4)(4) = -16 → orientação do mesh.
        assert!(wound_right(&verts, U16Vec3::new(0, 1, 2)));
        // (0,2,1): invertido.
        assert!(!wound_right(&verts, U16Vec3::new(0, 2, 1)));
        // (0,2,3): colinear — fica (não abre T-junctions).
        assert!(wound_right(&verts, U16Vec3::new(0, 2, 3)));
        // Índice fora: fora.
        assert!(!wound_right(&verts, U16Vec3::new(0, 1, 9)));
    }

    #[test]
    fn convex_world_ring_rejects_short_rings_without_underflow() {
        let mut mesh = InternalPolygonNavmesh::default();
        mesh.vertices = vec![U16Vec3::new(0, 0, 0), U16Vec3::new(2, 0, 0)];
        assert!(!mesh.valid_face(&[]));
        assert!(!mesh.valid_face(&[0]));
        assert!(!mesh.valid_face(&[0, 1]));
    }

    #[test]
    fn remove_vertex_at_zero_and_expanding_past_pre_removal_count_succeeds() {
        let nv_per_poly = 6usize;
        let max_tris = 18usize;
        // Interior vertex 0 is shared by four convex hexagons (`npolys = 4`),
        // with all incident edges internal so `can_remove_vertex(0)` is true.
        // Removing 0 yields a 16-gon hole: four 5-gon pockets plus a central
        // quad that cannot merge with any 5-gon (`5 + 4 - 2 = 7 > 6`), and
        // the reflex notches block ear-to-triangle merges (`after > before = 4`).
        let mut mesh = InternalPolygonNavmesh {
            aabb: Aabb3d {
                min: glam::Vec3::ZERO,
                ..Default::default()
            },
            cell_size: 1.0,
            cell_height: 0.1,
            max_vertices_per_polygon: nv_per_poly as u16,
            nvertices: 17,
            npolys: 4,
            vertices: vec![
                U16Vec3::new(100, 0, 100), // 0: interior vertex to remove
                U16Vec3::new(65, 0, 100),  // 1 (reflex)
                U16Vec3::new(40, 0, 125),  // 2
                U16Vec3::new(60, 0, 155),  // 3
                U16Vec3::new(85, 0, 155),  // 4
                U16Vec3::new(100, 0, 135), // 5 (reflex)
                U16Vec3::new(125, 0, 160), // 6
                U16Vec3::new(155, 0, 140), // 7
                U16Vec3::new(155, 0, 115), // 8
                U16Vec3::new(135, 0, 100), // 9 (reflex)
                U16Vec3::new(160, 0, 75),  // 10
                U16Vec3::new(140, 0, 45),  // 11
                U16Vec3::new(115, 0, 45),  // 12
                U16Vec3::new(100, 0, 65),  // 13 (reflex)
                U16Vec3::new(75, 0, 40),   // 14
                U16Vec3::new(45, 0, 60),   // 15
                U16Vec3::new(45, 0, 85),   // 16
            ],
            polygons: vec![u16::MAX; max_tris * nv_per_poly * 2],
            regions: vec![RegionId::from(1); max_tris],
            areas: vec![AreaType::DEFAULT_WALKABLE; max_tris],
            ..Default::default()
        };
        let p0 = &mut mesh.polygons[0..nv_per_poly];
        p0[..6].copy_from_slice(&[0, 1, 2, 3, 4, 5]);
        let p1 = &mut mesh.polygons[nv_per_poly * 2..nv_per_poly * 2 + nv_per_poly];
        p1[..6].copy_from_slice(&[0, 5, 6, 7, 8, 9]);
        let p2 = &mut mesh.polygons[nv_per_poly * 4..nv_per_poly * 4 + nv_per_poly];
        p2[..6].copy_from_slice(&[0, 9, 10, 11, 12, 13]);
        let p3 = &mut mesh.polygons[nv_per_poly * 6..nv_per_poly * 6 + nv_per_poly];
        p3[..6].copy_from_slice(&[0, 13, 14, 15, 16, 1]);
        mesh.build_mesh_adjacency().unwrap();

        assert!(mesh.can_remove_vertex(0));
        let before = mesh.npolys;
        mesh.remove_vertex(0, max_tris).unwrap();
        assert!(
            mesh.npolys > before,
            "hole triangulation must grow polygon count ({before} -> {})",
            mesh.npolys
        );
        assert!(mesh.npolys <= max_tris);

        // Full ContourSet::into_polygon_mesh with vertex 0 flagged as
        // BORDER_VERTEX shared by 3 contours meeting at (4, 4), so the
        // while-loop removes index 0 cleanly without i -= 1 underflow.
        let bv = RegionVertexId::BORDER_VERTEX.bits();
        let pt = |x, z, r| (U16Vec3::new(x, 0, z), r);
        let contours = ContourSet {
            contours: vec![
                crate::Contour {
                    vertices: vec![
                        pt(4, 4, bv),
                        pt(0, 4, 0),
                        pt(0, 8, 0),
                        pt(4, 8, 0),
                    ],
                    area: AreaType::DEFAULT_WALKABLE,
                    region: RegionId::from(1),
                    ..Default::default()
                },
                crate::Contour {
                    vertices: vec![
                        pt(4, 4, bv),
                        pt(4, 8, 0),
                        pt(8, 8, 0),
                        pt(8, 4, 0),
                    ],
                    area: AreaType::DEFAULT_WALKABLE,
                    region: RegionId::from(1),
                    ..Default::default()
                },
                crate::Contour {
                    vertices: vec![
                        pt(4, 4, bv),
                        pt(8, 4, 0),
                        pt(4, 0, 0),
                        pt(0, 4, 0),
                    ],
                    area: AreaType::DEFAULT_WALKABLE,
                    region: RegionId::from(1),
                    ..Default::default()
                },
            ],
            aabb: Aabb3d {
                min: glam::Vec3::ZERO,
                max: glam::Vec3::new(10., 10., 10.),
            },
            cell_size: 1.0,
            cell_height: 0.5,
            ..Default::default()
        };
        let built = contours.into_polygon_mesh(6).unwrap();
        assert!(built.polygon_count() >= 1);
        assert!(built.vertices.len() < 7, "border vertex at index 0 should be removed, verts={}", built.vertices.len());
    }

    /// Dois contornos do Vael, tal como o tile de canto (-40.4, 54.8) os
    /// produziu a 2026-11-23 (`(x, y, z, flags)` em células): o chão (região
    /// 365) e a estrada (368) que o rodeia a nordeste. O `landmass` recusou esse
    /// tile com `The edge made from vertices 5043 and 5044 is used by more than
    /// two polygons`.
    const GROUND: [(u16, u16, u16, u32); 33] = [
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
    const ROAD: [(u16, u16, u16, u32); 44] = [
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

    fn vael_tile_corner() -> ContourSet {
        let contour = |region: u16, area: u8, vertices: &[(u16, u16, u16, u32)]| crate::Contour {
            vertices: vertices
                .iter()
                .map(|&(x, y, z, flags)| (U16Vec3::new(x, y, z), flags))
                .collect::<Vec<_>>(),
            region: RegionId::from(region),
            area: AreaType(area),
            ..Default::default()
        };
        ContourSet {
            contours: vec![
                contour(365, 255, &GROUND),
                contour(368, 1, &ROAD),
            ],
            aabb: Aabb3d {
                min: glam::Vec3::new(-39.406_464, -200.0, 55.795_258),
                max: glam::Vec3::new(216.593_54, 400.0, 311.795_26),
            },
            width: 1524,
            height: 1524,
            cell_size: 0.166_666_67,
            cell_height: 0.125,
            border_size: 6,
            max_error: 1.3,
        }
    }

    /// Quantos polígonos têm os dois pontos (por coordenada XZ) como vértices.
    fn owners(mesh: &PolygonNavmesh, a: (u16, u16), b: (u16, u16)) -> usize {
        mesh.polygons
            .chunks_exact(mesh.max_vertices_per_polygon as usize)
            .filter(|chunk| {
                let xz: Vec<(u16, u16)> = chunk
                    .iter()
                    .take_while(|i| **i != PolygonNavmesh::NO_INDEX)
                    .map(|i| {
                        let v = mesh.vertices[usize::from(*i)];
                        (v.x, v.z)
                    })
                    .collect();
                xz.contains(&a) && xz.contains(&b)
            })
            .count()
    }

    /// Os vértices, por coordenada XZ, de cada polígono do mesh.
    fn xz_polygons(mesh: &PolygonNavmesh) -> Vec<Vec<(u16, u16)>> {
        let mut polygons: Vec<Vec<(u16, u16)>> = mesh
            .polygons()
            .map(|poly| {
                poly.map(|i| {
                    let v = mesh.vertices[usize::from(i)];
                    (v.x, v.z)
                })
                .collect()
            })
            .collect();
        for polygon in &mut polygons {
            polygon.sort_unstable();
        }
        polygons.sort_unstable();
        polygons
    }

    #[test]
    fn an_edge_keeps_at_most_two_owners_losing_the_sliver() {
        use crate::contours::viber_patch_tests::overused_edges;
        let mesh = vael_tile_corner().into_polygon_mesh(6).unwrap();

        assert!(
            overused_edges(&mesh).is_empty(),
            "{:?}",
            overused_edges(&mesh)
        );
        // A corda entre (25, 1477) e (22, 1472) ficou com a estrada de um lado
        // e o chão do outro: as duas regiões continuam ligadas.
        assert_eq!(owners(&mesh, (25, 1477), (22, 1472)), 2);
        // Só saiu a fenda — a orelha `(22,1472) (23,1475) (25,1477)`.
        assert!(!xz_polygons(&mesh).contains(&vec![(22, 1472), (23, 1475), (25, 1477)]));
    }

    #[test]
    fn culling_the_smallest_owner_keeps_the_regions_and_areas_in_step() {
        // Um quadrado em dois triângulos, com uma fenda `(0, 4, 2)` em cima da
        // aresta (0, 2) no MEIO da lista.
        let mut mesh = InternalPolygonNavmesh {
            max_vertices_per_polygon: 3,
            nvertices: 5,
            vertices: [(0, 0), (4, 0), (4, 4), (0, 4), (2, 1)]
                .map(|(x, z)| U16Vec3::new(x, 0, z))
                .to_vec(),
            npolys: 3,
            polygons: vec![
                0, 1, 2, u16::MAX, u16::MAX, u16::MAX, //
                0, 4, 2, u16::MAX, u16::MAX, u16::MAX, //
                0, 2, 3, u16::MAX, u16::MAX, u16::MAX, //
            ],
            areas: vec![AreaType(255), AreaType(9), AreaType(1)],
            regions: vec![RegionId::from(1), RegionId::from(9), RegionId::from(2)],
            ..Default::default()
        };

        assert_eq!(mesh.cull_overused_edges(), 1);

        assert_eq!(mesh.npolys, 2);
        assert_eq!(&mesh.polygons[..9], &[0, 1, 2, u16::MAX, u16::MAX, u16::MAX, 0, 2, 3]);
        assert_eq!(&mesh.areas[..2], &[AreaType(255), AreaType(1)]);
        assert_eq!(&mesh.regions[..2], &[RegionId::from(1), RegionId::from(2)]);
        assert_eq!(mesh.cull_overused_edges(), 0, "já não há excesso");
    }
}
