//! Grid paving — the road surface as an **exclusive lattice of cells**.
//!
//! A road used to be a [`super::decal::GroundDecalSpec`] (an ellipse with an
//! organic rim) or a [`super::roads::RoadSpec`] ribbon. Both are free-form
//! curves: two of them meeting share no triangulation, so the overlap has to
//! be settled by lifting one a few millimetres above the other
//! ([`super::decal::stack_decal_lifts`]) — a tie-breaker, not a resolution.
//! Corners do not exist either: a street corner is a stretched ellipse whose
//! rim crosses the rim of the street crossing it.
//!
//! Paving replaces that model with the one every city builder uses:
//!
//! * **One lattice.** Every paved node rasterizes into the same world grid,
//!   anchored at the origin, with the finest `cell` among the nodes as the
//!   module. That is what makes corners line up: the plaza edge and the
//!   street that leaves it land on the same lattice lines.
//! * **Exclusive occupancy.** A cell has **one** owner. Two nodes that want
//!   the same cell resolve by `priority` (declaration order breaks ties), so
//!   no two paving surfaces ever draw the same ground. Overlap is not
//!   resolved after the fact — it cannot happen.
//! * **Corner detection.** Each cell reads its 8 neighbours and shapes its
//!   outline: a **convex** corner (both adjacent edges are unowned) gets a
//!   chamfer arc, a **concave** one (both open, diagonal unowned) gets a
//!   fillet patch in the notch. A turn reads as a rounded kerb instead of a
//!   miter point, and a staircase of cells along a diagonal reads as a
//!   faceted but continuous band.
//! * **Seamless by construction.** UVs are world-space `[x/scale, z/scale]`,
//!   exactly like the decals, so the stone pattern is one continuous field
//!   over the plaza, the streets and the aprons — there is no per-node UV
//!   frame to line up. Interior cell edges share exact vertex positions, so
//!   the merged surface has no cracks.
//! * **One merge.** Cells are merged per (material, chunk) instead of one
//!   mesh per authored node: a city that used to be dozens of blended draw
//!   calls becomes a handful, and [`super::runtime`] splits the merged mesh
//!   into an opaque core plus a blended feather band.
//!
//! Paving never touches the heightfield. Every vertex samples the
//! [`BrushGrid`] at its own XZ, so the surface follows slopes, and the cell
//! outline is subdivided to [`MAX_EDGE`] before triangulating so the interior
//! follows the slope too — a centre fan alone is planar between hub and rim,
//! which is what made the old 16 m junction discs cut through the hillside.

use std::collections::BTreeMap;

use bevy::math::Vec2;
use bevy::prelude::Reflect;

use super::brush::BrushGrid;
use super::mesh::ChunkMeshData;

/// Lift of the paving layer above the sampled ground (meters). One layer for
/// the whole world: cross-node disagreements are settled by cell ownership,
/// not by height, so there is nothing to z-fight.
///
/// The full ladder, bottom to top:
///
/// ```text
/// 0.040  DECAL_LIFT            patches on bare terrain
/// 0.052  DECAL_STACK_CEILING   a patch under the pavement is covered by it
/// 0.060  RIBBON_LIFT           legacy road ribbons (<  PAVING_LIFT by 4 mm)
/// 0.064  PAVING_LIFT           the lattice, one layer
/// 0.072  ON_PAVING_LIFT        insets authored on top of the pavement
/// 0.082  ON_PAVING_CEILING
/// 0.100  JUNCTION_LIFT         fusion discs of the legacy ribbons
/// ```
///
/// It sits just above [`super::roads::RIBBON_LIFT`] so that where a shipped
/// dirt trail (still a ribbon) crosses a paved street the pavement wins
/// deterministically instead of the two sharing a plane.
pub const PAVING_LIFT: f32 = 0.064;
/// Lift of a decal authored ON the paving (`on-paving`) — a tree pit, a worn
/// patch, an inset. Everything else that lands under the paving is simply
/// covered: [`super::decal::DECAL_STACK_CEILING`] sits below [`PAVING_LIFT`]
/// on purpose.
pub const ON_PAVING_LIFT: f32 = 0.072;
/// Top of the `on-paving` decal stack.
pub const ON_PAVING_CEILING: f32 = 0.082;
/// Default lattice module (meters) when no node asks for one.
pub const DEFAULT_CELL: f32 = 2.0;
/// Default mesh bucket (meters). A single mesh spanning a whole world cannot
/// be frustum-culled; 64 m keeps a handful of draws per view.
pub const DEFAULT_CHUNK: f32 = 64.0;
/// Longest outline segment before triangulating (meters) — the drape
/// resolution. One meter matches the terrain texel and the road stations.
pub const MAX_EDGE: f32 = 1.0;
/// Segments per corner arc. Three keeps the chamfer curved instead of a
/// single flat cut at mid radii.
const CORNER_SEGS: usize = 3;
/// Rings across the feather band.
const FEATHER_RINGS: usize = 3;
/// A resolved radius under this is treated as "off" (meters).
const MIN_RADIUS: f32 = 0.02;
/// How far the material that feathers over a boundary sits above the one it
/// covers (meters). Enough to win the depth test at the world's far plane, and
/// far below anything the eye resolves on a flat square.
const SEAM_LIFT: f32 = 0.0015;
/// Length under which two outline points collapse into one (meters).
const WELD: f32 = 1e-4;

/// Grid tuning of a paved node (`paving-cell`, `paving-corner`, …).
///
/// Deliberately free of material and shape: the owning
/// [`super::decal::GroundDecalSpec`] / [`super::roads::RoadSpec`] keeps its
/// `texture`, `texture-scale`, `base-color`, `roughness`, `feather`, `noise`,
/// `seed` and `lift`, and [`PavingSpec`] copies them in. One vocabulary, one
/// place.
#[derive(Debug, Clone, PartialEq, Default, Reflect)]
#[type_path = "viber::terrain::paving"]
pub struct PavingStyle {
    /// Lattice module (meters). `0` inherits the world default
    /// ([`DEFAULT_CELL`], or the finest module among the paved nodes).
    pub cell: f32,
    /// Convex corner chamfer radius (meters). Negative = auto (`cell / 2`,
    /// i.e. the outline's arc reaches the edge midpoints and an isolated cell
    /// becomes a disc). `0` disables the chamfer.
    pub corner: f32,
    /// Concave corner fillet radius (meters). Negative = auto
    /// (`cell * 0.35`). `0` disables the fillet.
    pub inner_corner: f32,
    /// Mesh bucket (meters). Negative = [`DEFAULT_CHUNK`]; `0` = one mesh.
    pub chunk: f32,
    /// Claim priority: a higher value wins a contested cell. Declaration
    /// order breaks ties (later wins).
    pub priority: i32,
    /// Rasterize a `<GroundDecal>` footprint as a disc instead of its
    /// rectangle (`paving-round="1"`). Roads are corridors, so this only
    /// applies to decal-shaped nodes.
    pub round: bool,
}

impl PavingStyle {
    /// Module this style asks for, before the world-wide minimum is applied.
    fn requested_cell(&self) -> f32 {
        if self.cell > 0.0 {
            self.cell
        } else {
            DEFAULT_CELL
        }
    }

    fn chunk(&self) -> f32 {
        if self.chunk < 0.0 {
            DEFAULT_CHUNK
        } else {
            self.chunk
        }
    }

    /// Fills every field this style left at its "inherit" sentinel
    /// (`cell <= 0`, negative radii/chunk) from the world defaults.
    pub fn inherit_from(&mut self, default: &Self) {
        if self.cell <= 0.0 {
            self.cell = default.cell;
        }
        if self.corner < 0.0 {
            self.corner = default.corner;
        }
        if self.inner_corner < 0.0 {
            self.inner_corner = default.inner_corner;
        }
        if self.chunk < 0.0 {
            self.chunk = default.chunk;
        }
    }
}

/// A shape rasterized into the lattice.
#[derive(Debug, Clone, PartialEq, Reflect)]
#[type_path = "viber::terrain::paving"]
pub enum PavingShape {
    /// Axis-aligned rectangle (`at` = centre, `size` = full extent) — the
    /// `<GroundDecal size>` footprint.
    Rect { at: Vec2, size: Vec2 },
    /// Disc (`radius`) — the `<GroundDecal radius>` footprint.
    Disc { at: Vec2, radius: f32 },
    /// Corridor along carved road stations with a per-station half width —
    /// the stations are already Chaikin-smoothed, resampled to
    /// [`super::roads::STATION_SPACING`] and flared, so the raster reuses
    /// them instead of re-sampling the authoring polyline.
    Run {
        stations: Vec<Vec2>,
        half_width: Vec<f32>,
    },
}

/// One paved node (`<GroundDecal paving="1">`, `<Road paving="1">`,
/// `<RoadNetwork paving="1">`).
#[derive(Debug, Clone, PartialEq, Reflect)]
#[type_path = "viber::terrain::paving"]
pub struct PavingSpec {
    pub name: Option<String>,
    pub style: PavingStyle,
    /// Shapes in declaration order (a node with several `<Area>`s).
    pub shapes: Vec<PavingShape>,
    /// Base colour map (world asset path).
    pub texture: Option<String>,
    /// Normal map (world asset path).
    pub normal: Option<String>,
    /// Meters per texture repeat — world-space UVs.
    pub texture_scale: f32,
    /// Material tint under the texture.
    pub base_color: [f32; 3],
    pub roughness: f32,
    /// Alpha fade band outside the paved edge (meters). `0` = hard edge.
    pub feather: f32,
    /// Outward wobble of the feather band's outer rim, as a fraction of
    /// `feather`. The paved edge itself never wobbles: it has to stay on the
    /// lattice or the neighbouring cell would not meet it.
    pub noise: f32,
    /// Determinism seed for the rim harmonics.
    pub seed: u32,
    /// Lift override; `0.0` keeps [`PAVING_LIFT`].
    pub lift: f32,
}

impl Default for PavingSpec {
    fn default() -> Self {
        Self {
            name: None,
            style: PavingStyle {
                cell: 0.0,
                corner: -1.0,
                inner_corner: -1.0,
                chunk: -1.0,
                priority: 0,
                round: false,
            },
            shapes: Vec::new(),
            texture: None,
            normal: None,
            texture_scale: 6.0,
            base_color: [0.62, 0.60, 0.56],
            roughness: 0.9,
            feather: 1.0,
            noise: 0.1,
            seed: 0,
            lift: 0.0,
        }
    }
}

impl PavingSpec {
    pub fn lift(&self) -> f32 {
        if self.lift > 0.0 {
            self.lift
        } else {
            PAVING_LIFT
        }
    }

    /// A paved area from a `<GroundDecal>` footprint. `None` when the node did
    /// not ask for paving.
    pub fn from_decal(dec: &super::decal::GroundDecalSpec) -> Option<Self> {
        let style = dec.paving.clone()?;
        let shapes = vec![if style.round {
            PavingShape::Disc {
                at: dec.at,
                radius: dec.half_extent.max_element(),
            }
        } else {
            PavingShape::Rect {
                at: dec.at,
                size: dec.half_extent * 2.0,
            }
        }];
        Some(Self {
            name: dec.name.clone(),
            style,
            shapes,
            texture: dec.texture.clone(),
            normal: None,
            texture_scale: dec.texture_scale,
            base_color: dec.base_color,
            roughness: dec.roughness,
            feather: dec.feather,
            noise: dec.noise,
            seed: dec.seed,
            // A decal's default lift is `DECAL_LIFT`, which is *below* the
            // paving: leaving it there would tuck the pavement under the
            // terrain patches. Only an authored value overrides the paving
            // lift.
            lift: if (dec.lift - super::decal::DECAL_LIFT).abs() < 1e-6 {
                0.0
            } else {
                dec.lift
            },
        })
    }

    /// A paved corridor from a carved road. The stations come from
    /// [`super::roads::RoadPath`], so they are already Chaikin-smoothed,
    /// resampled to [`super::roads::STATION_SPACING`] and flared. `None` for a
    /// node that did not ask for paving, and for a bridge deck (which is a
    /// suspended span, not ground).
    pub fn from_road(path: &super::roads::RoadPath, spec: &super::roads::RoadSpec) -> Option<Self> {
        let style = spec.paving.clone()?;
        if path.bridge || path.profile == super::roads::RoadProfile::Bridge {
            return None;
        }
        if path.stations.len() < 2 || path.half_width.len() != path.stations.len() {
            return None;
        }
        let widest = path.half_width.iter().copied().fold(0.0f32, f32::max);
        // `edge-feather` is authored as a fraction of the half width for
        // ribbons; the lattice wants meters.
        let feather = (spec.edge_feather * widest).clamp(0.0, 3.0);
        let seed = spec
            .name
            .as_deref()
            .map(|n| {
                n.bytes().fold(0x811c_9dc5u32, |h, b| {
                    (h ^ b as u32).wrapping_mul(0x0100_0193)
                })
            })
            .unwrap_or_else(|| {
                let s = path.stations[0];
                s.x.to_bits() ^ s.y.to_bits().rotate_left(16)
            });
        Some(Self {
            name: path.name.clone().or_else(|| spec.name.clone()),
            style,
            shapes: vec![PavingShape::Run {
                stations: path.stations.clone(),
                half_width: path.half_width.clone(),
            }],
            texture: spec.texture.clone(),
            normal: None,
            texture_scale: spec.texture_scale,
            base_color: spec.base_color,
            roughness: spec.roughness,
            feather,
            noise: spec.edge_noise,
            seed,
            lift: 0.0,
        })
    }
}

/// Resolved material + shaping of one paving group. Cells carry a group id, so
/// two nodes with the same material share one mesh while a material change
/// produces a hard lattice seam between them (no feather, no double blend).
#[derive(Debug, Clone, PartialEq, Reflect)]
#[type_path = "viber::terrain::paving"]
pub struct PavingGroup {
    pub texture: Option<String>,
    pub normal: Option<String>,
    pub texture_scale: f32,
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub feather: f32,
    pub noise: f32,
    pub seed: u32,
    pub lift: f32,
    pub corner: f32,
    pub inner_corner: f32,
    pub chunk: f32,
    pub priority: i32,
    /// Index of the last node that contributes to this group, i.e. its place
    /// in the `.bsn` reading order. Used to decide which side of a material
    /// boundary feathers over the other: later paints over earlier, like a
    /// painter's algorithm.
    pub order: u32,
}

impl PavingGroup {
    /// Everything except the declaration order — two nodes with the same
    /// material share one mesh however far apart they sit in the file.
    fn same_material(&self, other: &Self) -> bool {
        self.texture == other.texture
            && self.normal == other.normal
            && self.texture_scale == other.texture_scale
            && self.base_color == other.base_color
            && self.roughness == other.roughness
            && self.feather == other.feather
            && self.noise == other.noise
            && self.seed == other.seed
            && self.lift == other.lift
            && self.corner == other.corner
            && self.inner_corner == other.inner_corner
            && self.chunk == other.chunk
            && self.priority == other.priority
    }
}

/// The world's paved surface: one lattice, one owner per cell.
#[derive(Debug, Clone, Default, Reflect)]
#[type_path = "viber::terrain::paving"]
pub struct PavingGrid {
    /// Lattice module (meters). `0` when empty.
    module: f32,
    /// Cell `(i, j)` covers `[i·module, (i+1)·module]` on both axes; its
    /// centre is `(i + 0.5, j + 0.5)·module`.
    cells: BTreeMap<(i32, i32), u16>,
    groups: Vec<PavingGroup>,
}

impl PavingGrid {
    /// Lattice module (meters); `0` when nothing is paved.
    pub fn module(&self) -> f32 {
        self.module
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Number of paved cells.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn groups(&self) -> &[PavingGroup] {
        &self.groups
    }

    /// Group id owning a cell (lattice indices).
    pub fn group_at_cell(&self, i: i32, j: i32) -> Option<u16> {
        self.cells.get(&(i, j)).copied()
    }

    /// Group id owning the cell under a world XZ point.
    pub fn group_at(&self, p: Vec2) -> Option<u16> {
        if self.module <= 0.0 {
            return None;
        }
        let (i, j) = self.cell_index(p);
        self.group_at_cell(i, j)
    }

    /// True when `p` lies on a paved cell.
    pub fn covers(&self, p: Vec2) -> bool {
        self.group_at(p).is_some()
    }

    /// Lattice indices of the cell containing `p`.
    pub fn cell_index(&self, p: Vec2) -> (i32, i32) {
        let m = self.module.max(1e-6);
        ((p.x / m).floor() as i32, (p.y / m).floor() as i32)
    }

    /// Centre of cell `(i, j)` in world XZ.
    pub fn cell_center(&self, i: i32, j: i32) -> Vec2 {
        Vec2::new(i as f32 + 0.5, j as f32 + 0.5) * self.module
    }

    /// World AABB of the paved cells `(min, max)`; `None` when empty.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        for &(i, j) in self.cells.keys() {
            min = min.min(Vec2::new(i as f32, j as f32) * self.module);
            max = max.max(Vec2::new(i as f32 + 1.0, j as f32 + 1.0) * self.module);
        }
        (min.x <= max.x).then_some((min, max))
    }

    /// Builds the lattice from the paved nodes, in declaration order.
    pub fn build(specs: &[PavingSpec]) -> Self {
        let mut grid = Self::default();
        if specs.is_empty() {
            return grid;
        }
        // One lattice for the whole world: the finest module any node asks
        // for wins, and the coarser ones simply cover groups of cells. Two
        // non-nested modules (1.5 m and 2 m) would otherwise claim the same
        // ground at different boundaries and no priority rule could make the
        // result seamless.
        let module = specs
            .iter()
            .map(|s| s.style.requested_cell())
            .fold(f32::INFINITY, f32::min)
            .max(0.05);
        grid.module = module;

        for (index, spec) in specs.iter().enumerate() {
            let group = PavingGroup {
                texture: spec.texture.clone(),
                normal: spec.normal.clone(),
                texture_scale: spec.texture_scale,
                base_color: spec.base_color,
                roughness: spec.roughness,
                feather: spec.feather.max(0.0),
                noise: spec.noise.clamp(0.0, 1.0),
                seed: spec.seed,
                lift: spec.lift(),
                corner: resolve_radius(spec.style.corner, module * 0.5),
                inner_corner: resolve_radius(spec.style.inner_corner, module * 0.35),
                chunk: spec.style.chunk(),
                priority: spec.style.priority,
                order: index as u32,
            };
            let gid = match grid.groups.iter().position(|g| g.same_material(&group)) {
                Some(i) => {
                    // Same material further down the file: the newer
                    // declaration is the one that paints on top.
                    grid.groups[i].order = index as u32;
                    i as u16
                }
                None => {
                    grid.groups.push(group);
                    (grid.groups.len() - 1) as u16
                }
            };
            for shape in &spec.shapes {
                grid.rasterize(shape, gid);
            }
        }
        grid
    }

    /// Claims every cell the shape touches. A cell's square only has to
    /// *intersect* the shape, so touching shapes never leave a one-cell gap;
    /// the cost is that a shape grows by at most one module per side
    /// (authoring in multiples of the module keeps it exact).
    fn rasterize(&mut self, shape: &PavingShape, gid: u16) {
        let m = self.module;
        match shape {
            PavingShape::Rect { at, size } => {
                if size.x <= 0.0 || size.y <= 0.0 {
                    return;
                }
                let h = *size * 0.5;
                let i0 = ((at.x - h.x) / m).floor() as i32;
                let i1 = ((at.x + h.x) / m).ceil() as i32;
                let j0 = ((at.y - h.y) / m).floor() as i32;
                let j1 = ((at.y + h.y) / m).ceil() as i32;
                for j in j0..j1 {
                    for i in i0..i1 {
                        self.claim((i, j), gid);
                    }
                }
            }
            PavingShape::Disc { at, radius } => {
                if *radius <= 0.0 {
                    return;
                }
                self.stamp(*at, *radius, gid);
            }
            PavingShape::Run {
                stations,
                half_width,
            } => {
                for (k, a) in stations.iter().enumerate() {
                    let hw = half_width.get(k).copied().unwrap_or(1.0).max(0.0);
                    if hw <= 0.0 {
                        continue;
                    }
                    match stations.get(k + 1) {
                        Some(b) => self.stamp_segment(*a, *b, hw, gid),
                        // The last station of an open path: a disc, so a road
                        // tip is not a hairline.
                        None => self.stamp(*a, hw, gid),
                    }
                }
            }
        }
    }

    /// Claims the cells within `radius` of `at`.
    fn stamp(&mut self, at: Vec2, radius: f32, gid: u16) {
        let m = self.module;
        let r = radius + m;
        let i0 = ((at.x - r) / m).floor() as i32;
        let i1 = ((at.x + r) / m).ceil() as i32;
        let j0 = ((at.y - r) / m).floor() as i32;
        let j1 = ((at.y + r) / m).ceil() as i32;
        for j in j0..=j1 {
            for i in i0..=i1 {
                let (min, max) = self.cell_box(i, j);
                if point_box_distance(at, min, max) <= radius {
                    self.claim((i, j), gid);
                }
            }
        }
    }

    /// Claims the cells within `half_width` of the segment `a → b`.
    fn stamp_segment(&mut self, a: Vec2, b: Vec2, half_width: f32, gid: u16) {
        let m = self.module;
        let min = a.min(b) - Vec2::splat(half_width + m);
        let max = a.max(b) + Vec2::splat(half_width + m);
        let i0 = (min.x / m).floor() as i32;
        let i1 = (max.x / m).ceil() as i32;
        let j0 = (min.y / m).floor() as i32;
        let j1 = (max.y / m).ceil() as i32;
        // Stations are one meter apart and cells are at least that big, but a
        // segment can cross a box without either endpoint landing in it
        // (a corner clip), so walk the segment in sub-metre steps.
        let steps = (((b - a).length() / (m * 0.5)).ceil() as usize).clamp(1, 8);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let (bmin, bmax) = self.cell_box(i, j);
                for k in 0..=steps {
                    let p = a + (b - a) * (k as f32 / steps as f32);
                    // Strict: a box merely *tangent* to the corridor has zero
                    // overlap with it, and counting it would double the road
                    // width whenever the centreline sits on a lattice line.
                    if point_box_distance(p, bmin, bmax) < half_width {
                        self.claim((i, j), gid);
                        break;
                    }
                }
            }
        }
    }

    fn claim(&mut self, key: (i32, i32), gid: u16) {
        let replace = match self.cells.get(&key) {
            None => true,
            // `>=` on purpose: iterating in declaration order, a tie lets the
            // later node win, which is the reading order of the file.
            Some(&cur) => self.groups[gid as usize].priority >= self.groups[cur as usize].priority,
        };
        if replace {
            self.cells.insert(key, gid);
        }
    }

    fn cell_box(&self, i: i32, j: i32) -> (Vec2, Vec2) {
        let m = self.module;
        (
            Vec2::new(i as f32, j as f32) * m,
            Vec2::new(i as f32 + 1.0, j as f32 + 1.0) * m,
        )
    }

    /// Occupancy class of one of the cell's edges, from the owner's view.
    fn edge(&self, i: i32, j: i32, gid: u16, d: (i32, i32)) -> Edge {
        match self.cells.get(&(i + d.0, j + d.1)) {
            None => Edge::Wall,
            Some(&g) if g == gid => Edge::Open,
            Some(_) => Edge::Seam,
        }
    }
}

/// Distance from a point to an axis-aligned box (`0` inside).
fn point_box_distance(p: Vec2, min: Vec2, max: Vec2) -> f32 {
    let d = (min - p).max(p - max).max(Vec2::ZERO);
    d.length()
}

fn resolve_radius(value: f32, auto: f32) -> f32 {
    let r = if value < 0.0 { auto } else { value };
    if r < MIN_RADIUS { 0.0 } else { r }
}

/// How a cell edge reads for its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// Same group on the other side — interior, no outline, no band.
    Open,
    /// Nothing on the other side — the paved rim. Gets a feather band.
    Wall,
    /// A different group: a hard lattice edge, no chamfer and no band (the
    /// neighbour draws its own side, and feathering it would double-blend).
    Seam,
}

/// Corner treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CornerKind {
    Plain,
    /// Convex corner: rounded inward, the visible kerb of a turn.
    Chamfer,
    /// Concave corner: a patch filling the notch so the inside of a turn is
    /// rounded instead of a miter point.
    Fillet,
}

/// Ordered outline of one cell plus, per outgoing segment, whether it is a
/// paved rim (needs a feather band).
struct Outline {
    nodes: Vec<Vec2>,
    /// `wall[i]` applies to the segment `nodes[i] → nodes[i+1]`.
    wall: Vec<bool>,
}

struct OutlineBuilder {
    nodes: Vec<Vec2>,
    wall: Vec<bool>,
}

impl OutlineBuilder {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            wall: Vec::new(),
        }
    }

    /// Appends `p`; `wall` describes the segment from the previous node.
    fn push(&mut self, p: Vec2, wall: bool) {
        if let Some(&last) = self.nodes.last()
            && (p - last).length_squared() <= WELD * WELD
        {
            return;
        }
        if !self.nodes.is_empty() {
            self.wall.push(wall);
        }
        self.nodes.push(p);
    }

    /// Appends the interior subdivision of `from → to` (both exclusive).
    fn run(&mut self, from: Vec2, to: Vec2, wall: bool) {
        let len = (to - from).length();
        let steps = (len / MAX_EDGE).ceil() as usize;
        for k in 1..steps {
            let t = k as f32 / steps as f32;
            self.push(from + (to - from) * t, wall);
        }
    }

    /// Closes the loop: the last node's segment meets the first.
    fn finish(mut self, closing_wall: bool) -> Outline {
        // A cell whose chamfers weld at the edge midpoints (every corner at
        // half a module) walks back onto its first point. Drop the duplicate
        // so the ring has one node per position; the segment that brought us
        // there is the closing one.
        if self.nodes.len() > 2
            && (self.nodes[self.nodes.len() - 1] - self.nodes[0]).length_squared()
                <= WELD * WELD
        {
            self.nodes.pop();
            let closing = self.wall.pop().unwrap_or(closing_wall);
            self.wall.push(closing);
        } else {
            self.wall.push(closing_wall);
        }
        Outline {
            nodes: self.nodes,
            wall: self.wall,
        }
    }
}

/// Samples the 90° arc between `start` and `end` around `center`, picking the
/// sweep whose midpoint lands nearest `bias`.
///
/// Both a chamfer and a fillet bulge toward the corner point they treat (one
/// cutting it, the other filling the notch beside it), so "nearer the corner"
/// is the one rule that covers both.
fn arc(center: Vec2, start: Vec2, end: Vec2, bias: Vec2, segs: usize) -> Vec<Vec2> {
    let r = (start - center).length();
    if r <= MIN_RADIUS {
        return vec![start];
    }
    let a0 = (start - center).to_angle();
    let a1 = (end - center).to_angle();
    let mut delta = a1 - a0;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    let other = if delta > 0.0 {
        delta - std::f32::consts::TAU
    } else {
        delta + std::f32::consts::TAU
    };
    let mid = |d: f32| center + Vec2::from_angle(a0 + d * 0.5) * r;
    let delta = if (mid(delta) - bias).length_squared() <= (mid(other) - bias).length_squared() {
        delta
    } else {
        other
    };
    (0..=segs)
        .map(|k| center + Vec2::from_angle(a0 + delta * (k as f32 / segs as f32)) * r)
        .collect()
}

/// Builds one cell's outline and its concave fillet patches.
///
/// `p` are the four corners in walk order `(+h,-h) → (+h,+h) → (-h,+h) →
/// (-h,-h)`; `n` the matching outward normals of the four edges
/// (`E, N, W, S`); `edge` the occupancy class per edge; `diag` the group id of
/// the four diagonals in the same corner order.
fn cell_outline(
    center: Vec2,
    half: f32,
    edge: [Edge; 4],
    diag: [Option<u16>; 4],
    corner: f32,
    inner_corner: f32,
) -> (Outline, Vec<Vec<Vec2>>) {
    // Walk order: each corner k sits between edge (k + 3) % 4 (incoming) and
    // edge k (outgoing).
    let corners = [
        Vec2::new(half, -half),
        Vec2::new(half, half),
        Vec2::new(-half, half),
        Vec2::new(-half, -half),
    ];
    // Outward normal per edge, indexed E, N, W, S.
    let normals = [
        Vec2::new(1.0, 0.0),
        Vec2::new(0.0, 1.0),
        Vec2::new(-1.0, 0.0),
        Vec2::new(0.0, -1.0),
    ];

    let mut kinds = [CornerKind::Plain; 4];
    for k in 0..4 {
        let a = edge[(k + 3) % 4];
        let b = edge[k];
        kinds[k] = if a == Edge::Seam || b == Edge::Seam {
            // A shared lattice edge must stay straight: chamfering it would
            // open a wedge against the neighbouring group.
            CornerKind::Plain
        } else if a == Edge::Wall && b == Edge::Wall {
            if corner >= MIN_RADIUS {
                CornerKind::Chamfer
            } else {
                CornerKind::Plain
            }
        } else if a == Edge::Open && b == Edge::Open {
            // Concave: the paving wraps around an unpaved diagonal cell. Only
            // when that diagonal belongs to nobody — a fillet must never spill
            // into another group's cells.
            match diag[k] {
                None if inner_corner >= MIN_RADIUS => CornerKind::Fillet,
                _ => CornerKind::Plain,
            }
        } else {
            CornerKind::Plain
        };
    }

    // Per-corner point lists; `first` touches the incoming edge, `last` the
    // outgoing one.
    let mut pts: Vec<Vec<Vec2>> = Vec::with_capacity(4);
    let mut corner_wall = [false; 4];
    let mut fillets: Vec<Vec<Vec2>> = Vec::new();

    for k in 0..4 {
        let p = center + corners[k];
        let na = normals[(k + 3) % 4];
        let nb = normals[k];
        match kinds[k] {
            CornerKind::Plain => pts.push(vec![p]),
            CornerKind::Chamfer => {
                // Circle tangent to both walls from the inside: the outline
                // leaves the incoming edge `corner` meters early and rejoins
                // the outgoing one the same way.
                let start = p - nb * corner;
                let end = p - na * corner;
                let arc_center = p - na * corner - nb * corner;
                pts.push(arc(arc_center, start, end, p, CORNER_SEGS));
                corner_wall[k] = true;
            }
            CornerKind::Fillet => {
                // The polygon itself keeps the sharp reflex corner: the fillet
                // is a separate patch in the unpaved diagonal. Filling the
                // notch from inside the cell would either miss it (the notch
                // is outside) or duplicate the neighbours' paving.
                pts.push(vec![p]);
                // Circle tangent to the two *neighbour* walls from the notch
                // side; the patch is the region between it and the corner.
                let start = p + nb * inner_corner;
                let end = p + na * inner_corner;
                let arc_center = p + na * inner_corner + nb * inner_corner;
                let mut patch = vec![p];
                patch.extend(arc(arc_center, start, end, p, CORNER_SEGS));
                fillets.push(patch);
            }
        }
    }

    let mut b = OutlineBuilder::new();
    let mut closing_wall = false;
    for k in 0..4 {
        // The first point of a corner terminates the run along the *incoming*
        // edge; the rest of the corner's points continue its own arc.
        let in_wall = edge[(k + 3) % 4] == Edge::Wall;
        for (n, p) in pts[k].iter().enumerate() {
            b.push(*p, if n == 0 { in_wall } else { corner_wall[k] });
        }
        // Straight run along edge k to the first point of the next corner.
        let from = *pts[k].last().expect("corner has points");
        let to = pts[(k + 1) % 4][0];
        b.run(from, to, edge[k] == Edge::Wall);
        if k == 3 {
            closing_wall = edge[3] == Edge::Wall;
        }
    }
    (b.finish(closing_wall), fillets)
}

/// Pushes a draped vertex; returns its index.
fn push_vertex(
    mesh: &mut ChunkMeshData,
    grid: &BrushGrid,
    x: f32,
    z: f32,
    lift: f32,
    scale: f32,
    tint: f32,
    alpha: f32,
) -> u32 {
    let y = grid.sample(x, z) + lift;
    let index = mesh.positions.len() as u32;
    mesh.positions.push([x, y, z]);
    mesh.normals
        .push(grid.sample_normal(x, z, grid.texel()).to_array());
    mesh.uvs.push([x / scale, z / scale]);
    mesh.colors.push([tint, tint, tint, alpha]);
    index
}

/// Signed area of a polygon in the XZ plane, using the sign convention of the
/// `+Y` face winding (see [`up_normal`]).
fn signed_area(nodes: &[Vec2]) -> f32 {
    let mut sum = 0.0;
    for (k, a) in nodes.iter().enumerate() {
        let b = nodes[(k + 1) % nodes.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    sum * 0.5
}

/// `y` component of `(b - a) × (c - a)` for points in the XZ plane. Positive
/// is the `+Y` face winding (same convention as the decal hub fan).
fn up_normal(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b.y - a.y) * (c.x - a.x) - (b.x - a.x) * (c.y - a.y)
}

/// World-position wobble used on the feather band's outer rim. A function of
/// position, not of the segment index, so two cells' bands agree exactly where
/// they meet.
fn rim_wobble(seed: u32, x: f32, z: f32) -> f32 {
    let s = hash01(seed) * std::f32::consts::TAU;
    0.6 * (x * 0.73 + s).sin() + 0.4 * (z * 1.17 + s * 1.7).sin()
}

fn hash01(i: u32) -> f32 {
    let mut h = i.wrapping_mul(0x27d4_eb2d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x8549_5cb5);
    h ^= h >> 16;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

fn smootherstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Builds one mesh per (material group, chunk bucket).
///
/// The caller pairs `group` with [`PavingGrid::groups`] to build the material.
/// Buckets keep frustum culling useful on a 4 km world; `chunk = 0` collapses
/// everything into one mesh.
///
/// A cell normally feeds its own group's mesh, but not only: where it shares a
/// lattice edge with a **material declared later**, that material also emits a
/// feather band *inside this cell*, a couple of millimetres above it. That is
/// how a dirt street fades into a cobbled square instead of cutting it with a
/// razor line — the same trick a painter's algorithm plays with opacities, and
/// the reason the bands can never be coplanar with what they cover.
pub fn paving_meshes(terrain: &BrushGrid, paving: &PavingGrid) -> Vec<(usize, ChunkMeshData)> {
    if paving.is_empty() {
        return Vec::new();
    }
    let module = paving.module();
    let half = module * 0.5;

    // Bucket key for a given group at a given world point.
    let bucket = |gid: u16, at: Vec2| -> (u16, i32, i32) {
        let chunk = paving.groups[gid as usize].chunk.max(0.0);
        if chunk > 0.0 {
            (
                gid,
                (at.x / chunk).floor() as i32,
                (at.y / chunk).floor() as i32,
            )
        } else {
            (gid, 0, 0)
        }
    };

    let mut pieces: Vec<((u16, i32, i32), ChunkMeshData)> = Vec::new();
    for (&(i, j), &gid) in &paving.cells {
        let center = paving.cell_center(i, j);
        let group = &paving.groups[gid as usize];
        let scale = if group.texture_scale > 0.0 {
            group.texture_scale
        } else {
            1.0
        };
        let edge = [
            paving.edge(i, j, gid, (1, 0)),
            paving.edge(i, j, gid, (0, 1)),
            paving.edge(i, j, gid, (-1, 0)),
            paving.edge(i, j, gid, (0, -1)),
        ];
        let diag = [
            paving.group_at_cell(i + 1, j - 1),
            paving.group_at_cell(i + 1, j + 1),
            paving.group_at_cell(i - 1, j + 1),
            paving.group_at_cell(i - 1, j - 1),
        ];
        let (outline, fillets) = cell_outline(
            center,
            half,
            edge,
            diag,
            group.corner,
            group.inner_corner,
        );
        let mut piece = ChunkMeshData::default();
        emit_cell(&mut piece, terrain, &outline, group, scale, center);
        for patch in fillets {
            emit_fillet(&mut piece, terrain, &patch, group, scale);
        }
        pieces.push((bucket(gid, center), piece));

        // Material boundaries this cell loses: the neighbour declared later
        // feathers over it.
        let dirs = [
            ((1, 0), Vec2::new(1.0, 0.0)),
            ((0, 1), Vec2::new(0.0, 1.0)),
            ((-1, 0), Vec2::new(-1.0, 0.0)),
            ((0, -1), Vec2::new(0.0, -1.0)),
        ];
        for (dir, normal) in dirs {
            if edge[edge_index(dir)] != Edge::Seam {
                continue;
            }
            let Some(&other) = paving.cells.get(&(i + dir.0, j + dir.1)) else {
                continue;
            };
            if other == gid {
                continue;
            }
            let upper = &paving.groups[other as usize];
            if upper.order <= group.order {
                continue;
            }
            let up_scale = if upper.texture_scale > 0.0 {
                upper.texture_scale
            } else {
                1.0
            };
            let mut band = ChunkMeshData::default();
            emit_seam_band(&mut band, terrain, center, half, normal, upper, up_scale);
            if !band.indices.is_empty() {
                pieces.push((bucket(other, center), band));
            }
        }
    }

    let mut merged: BTreeMap<(u16, i32, i32), ChunkMeshData> = BTreeMap::new();
    for (key, piece) in pieces {
        if piece.indices.is_empty() {
            continue;
        }
        merge_into(merged.entry(key).or_default(), piece);
    }
    merged
        .into_iter()
        .map(|(key, mesh)| (key.0 as usize, mesh))
        .collect()
}

/// Index of an edge in the `[E, N, W, S]` array for a lattice direction.
fn edge_index(dir: (i32, i32)) -> usize {
    match dir {
        (1, 0) => 0,
        (0, 1) => 1,
        (-1, 0) => 2,
        _ => 3,
    }
}

/// Appends `src` to `target`, rebasing its indices.
fn merge_into(target: &mut ChunkMeshData, src: ChunkMeshData) {
    let base = target.positions.len() as u32;
    target.positions.extend(src.positions);
    target.normals.extend(src.normals);
    target.uvs.extend(src.uvs);
    target.colors.extend(src.colors);
    target
        .indices
        .extend(src.indices.into_iter().map(|i| i + base));
}

/// Emits the feather band of a material that fades over the cell it borders:
/// full strength on the shared lattice edge, zero `feather` meters inside the
/// neighbouring cell, [`SEAM_LIFT`] above that cell's pavement.
fn emit_seam_band(
    mesh: &mut ChunkMeshData,
    grid: &BrushGrid,
    center: Vec2,
    half: f32,
    normal: Vec2,
    group: &PavingGroup,
    scale: f32,
) {
    let feather = group.feather.min(half * 2.0);
    if feather <= MIN_RADIUS {
        return;
    }
    // The shared edge runs along the tangent of `normal`; walk it from one
    // cell corner to the other and extrude inward.
    let tangent = Vec2::new(-normal.y, normal.x);
    let mid = center + normal * half;
    let a = mid - tangent * half;
    let b = mid + tangent * half;
    let lift = group.lift + SEAM_LIFT;
    let mut prev = [
        push_vertex(mesh, grid, a.x, a.y, lift, scale, 1.0, 1.0),
        push_vertex(mesh, grid, b.x, b.y, lift, scale, 1.0, 1.0),
    ];
    for ring in 1..=FEATHER_RINGS {
        let t = ring as f32 / FEATHER_RINGS as f32;
        let alpha = 1.0 - smootherstep01(t);
        let wobble = group.noise * feather * t;
        let mut cur = [0u32; 2];
        for (idx, p) in [a, b].into_iter().enumerate() {
            let w = if wobble > 0.0 {
                (rim_wobble(group.seed, p.x, p.y) * wobble).max(-feather * 0.5)
            } else {
                0.0
            };
            let q = p - normal * (feather * t + w);
            cur[idx] = push_vertex(mesh, grid, q.x, q.y, lift, scale, 1.0, alpha);
        }
        strip(mesh, prev, cur);
        prev = cur;
    }
}

/// Emits the cell polygon (fan from the cell centre) and its feather band.
fn emit_cell(
    mesh: &mut ChunkMeshData,
    grid: &BrushGrid,
    outline: &Outline,
    group: &PavingGroup,
    scale: f32,
    center: Vec2,
) {
    let nodes = &outline.nodes;
    if nodes.len() < 3 {
        return;
    }
    let lift = group.lift;
    let reverse = signed_area(nodes) > 0.0;
    let hub = push_vertex(
        mesh, grid, center.x, center.y, lift, scale, 1.0, 1.0,
    );
    let mut indices = Vec::with_capacity(nodes.len());
    for p in nodes {
        indices.push(push_vertex(
            mesh, grid, p.x, p.y, lift, scale, 1.0, 1.0,
        ));
    }
    for k in 0..nodes.len() {
        let a = indices[k];
        let c = indices[(k + 1) % nodes.len()];
        if reverse {
            mesh.indices.extend_from_slice(&[hub, c, a]);
        } else {
            mesh.indices.extend_from_slice(&[hub, a, c]);
        }
    }

    // Feather band: one strip per paved rim segment, extruded away from the
    // cell centre so the ring never folds back on itself.
    let feather = group.feather;
    if feather <= MIN_RADIUS {
        return;
    }
    for k in 0..nodes.len() {
        if !outline.wall.get(k).copied().unwrap_or(false) {
            continue;
        }
        let a = nodes[k];
        let b = nodes[(k + 1) % nodes.len()];
        let mid = (a + b) * 0.5;
        let dir = b - a;
        let len = dir.length();
        if len <= WELD {
            continue;
        }
        let mut n = Vec2::new(dir.y, -dir.x) / len;
        if n.dot(mid - center) < 0.0 {
            n = -n;
        }
        // Ring 0 shares the paved edge exactly; rings 1..=FEATHER_RINGS walk
        // outward with alpha 1 → 0.
        let inner = [
            push_vertex(mesh, grid, a.x, a.y, lift, scale, 1.0, 1.0),
            push_vertex(mesh, grid, b.x, b.y, lift, scale, 1.0, 1.0),
        ];
        let mut prev = inner;
        for ring in 1..=FEATHER_RINGS {
            let t = ring as f32 / FEATHER_RINGS as f32;
            let alpha = 1.0 - smootherstep01(t);
            // The outer rim wobbles; the ring just inside it only a little, so
            // the band stays a smooth ramp.
            let wobble = group.noise * feather * t;
            let mut cur = [0u32; 2];
            for (s, p) in [a, b].into_iter().enumerate() {
                let w = if wobble > 0.0 {
                    (rim_wobble(group.seed, p.x, p.y) * wobble).max(-feather * 0.5)
                } else {
                    0.0
                };
                let q = p + n * (feather * t + w);
                cur[s] = push_vertex(mesh, grid, q.x, q.y, lift, scale, 1.0, alpha);
            }
            strip(mesh, prev, cur);
            prev = cur;
        }
    }
}

/// Appends the two triangles of one band quad, wound so both face `+Y`.
fn strip(mesh: &mut ChunkMeshData, prev: [u32; 2], cur: [u32; 2]) {
    let p = |i: u32| {
        let v = mesh.positions[i as usize];
        Vec2::new(v[0], v[2])
    };
    let up = up_normal(p(prev[0]), p(prev[1]), p(cur[1])) > 0.0;
    if up {
        mesh.indices
            .extend_from_slice(&[prev[0], prev[1], cur[1], prev[0], cur[1], cur[0]]);
    } else {
        mesh.indices
            .extend_from_slice(&[prev[0], cur[1], prev[1], prev[0], cur[0], cur[1]]);
    }
}

/// Emits a concave-corner fillet: the notch filler bounded by the cell corner
/// and the arc, all of it in unpaved space.
fn emit_fillet(
    mesh: &mut ChunkMeshData,
    grid: &BrushGrid,
    patch: &[Vec2],
    group: &PavingGroup,
    scale: f32,
) {
    if patch.len() < 3 {
        return;
    }
    let lift = group.lift;
    let hub = patch[0];
    let hub_idx = push_vertex(mesh, grid, hub.x, hub.y, lift, scale, 1.0, 1.0);
    let idx: Vec<u32> = patch[1..]
        .iter()
        .map(|p| push_vertex(mesh, grid, p.x, p.y, lift, scale, 1.0, 1.0))
        .collect();
    for k in 0..idx.len().saturating_sub(1) {
        let (a, b) = (idx[k], idx[k + 1]);
        let pa = Vec2::new(
            mesh.positions[a as usize][0],
            mesh.positions[a as usize][2],
        );
        let pb = Vec2::new(
            mesh.positions[b as usize][0],
            mesh.positions[b as usize][2],
        );
        if up_normal(hub, pa, pb) > 0.0 {
            mesh.indices.extend_from_slice(&[hub_idx, a, b]);
        } else {
            mesh.indices.extend_from_slice(&[hub_idx, b, a]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 128×128 grid over 128 m with a rolling hill, so anything that fails to
    /// drape shows up as a vertical error.
    fn test_grid() -> BrushGrid {
        let mut grid =
            BrushGrid::new(vec![0; 128 * 128], 128, 128, 128.0, 50.0, 0.0).expect("grid");
        grid.begin_stroke("base");
        for z in 0..128 {
            for x in 0..128 {
                let p = grid.cell_center(x, z);
                let h = 6.0 + 10.0 * (-(p.y * p.y) / 500.0).exp() + 2.0 * (p.x * 0.02).sin();
                grid.set_cell_height(x, z, h);
            }
        }
        grid.commit_stroke();
        grid
    }

    fn rect_spec(at: Vec2, size: Vec2, priority: i32) -> PavingSpec {
        PavingSpec {
            shapes: vec![PavingShape::Rect { at, size }],
            style: PavingStyle {
                priority,
                ..PavingStyle::default()
            },
            ..PavingSpec::default()
        }
    }

    #[test]
    fn rect_snaps_outward_to_the_lattice() {
        let grid = PavingGrid::build(&[rect_spec(Vec2::ZERO, Vec2::splat(34.0), 0)]);
        assert_eq!(grid.module(), 2.0);
        // 34 m centred on 0 → −17..17, snapped out to −18..18 = 18 cells.
        assert_eq!(grid.len(), 18 * 18);
        let (min, max) = grid.bounds().expect("bounds");
        assert_eq!((min, max), (Vec2::splat(-18.0), Vec2::splat(18.0)));
    }

    #[test]
    fn occupancy_is_exclusive_and_priority_wins() {
        // A tall rectangle and a short one crossing it at a different
        // priority: every cell ends up owned by exactly one group.
        let low = PavingSpec {
            base_color: [0.1, 0.1, 0.1],
            shapes: vec![PavingShape::Rect {
                at: Vec2::ZERO,
                size: Vec2::splat(10.0),
            }],
            style: PavingStyle {
                priority: 1,
                ..PavingStyle::default()
            },
            ..PavingSpec::default()
        };
        let high = PavingSpec {
            base_color: [0.9, 0.9, 0.9],
            shapes: vec![PavingShape::Rect {
                at: Vec2::ZERO,
                size: Vec2::splat(4.0),
            }],
            style: PavingStyle {
                priority: 9,
                ..PavingStyle::default()
            },
            ..PavingSpec::default()
        };
        let grid = PavingGrid::build(&[low, high]);
        assert_eq!(grid.groups().len(), 2, "duas cores, dois grupos");
        // The contested middle belongs to the high-priority group and nowhere
        // else: `covers` can only ever name one owner.
        let owner = grid.group_at(Vec2::ZERO).expect("owned");
        assert_eq!(grid.groups()[owner as usize].base_color[0], 0.9);
        // Outside the small rect the low-priority group still owns ground.
        let outer = grid.group_at(Vec2::new(4.5, 4.5)).expect("owned");
        assert_eq!(grid.groups()[outer as usize].base_color[0], 0.1);
    }

    #[test]
    fn touching_shapes_leave_no_gap() {
        // The two authored rects meet at exactly x = 0 but neither is on the
        // lattice; the outward snap has to make them overlap by a cell.
        let a = rect_spec(Vec2::new(-3.5, 0.0), Vec2::new(7.0, 20.0), 0);
        let b = rect_spec(Vec2::new(3.5, 0.0), Vec2::new(7.0, 20.0), 0);
        let grid = PavingGrid::build(&[a, b]);
        for j in -4..4 {
            assert!(
                grid.group_at_cell(-1, j).is_some() || grid.group_at_cell(0, j).is_some(),
                "a célula {j} junto à costura tem de estar pavimentada"
            );
        }
        // And the seam is contiguous: walking the row, no unowned cell sits
        // between two owned ones.
        let owned: Vec<i32> = (-12..12)
            .filter(|i| grid.group_at_cell(*i, 0).is_some())
            .collect();
        assert_eq!(
            owned.last().unwrap() - owned.first().unwrap(),
            owned.len() as i32 - 1,
            "a faixa tem de ser contígua: {owned:?}"
        );
    }

    #[test]
    fn straight_run_has_no_corner_arcs() {
        // A one-cell-high horizontal slab: the top and bottom edges are
        // straight, only the two ends have convex corners.
        let mut spec = rect_spec(Vec2::ZERO, Vec2::new(10.0, 2.0), 0);
        spec.style.corner = 0.0;
        spec.feather = 0.0;
        let grid = PavingGrid::build(&[spec]);
        let meshes = paving_meshes(&test_grid(), &grid);
        let mesh = &meshes[0].1;
        // Without a chamfer a cell is its square: 4 corners, plus one
        // subdivision point per edge (MAX_EDGE = 1 m on a 2 m cell), plus the
        // hub the fan pivots on. No arc vertices at all.
        let cells = grid.len();
        assert_eq!(cells, 12, "10 × 2 m em módulo 2 = 6 × 2 células");
        assert_eq!(mesh.positions.len(), cells * 9, "8 nós + hub por célula");
        assert_eq!(mesh.indices.len(), cells * 8 * 3, "fan de 8 triângulos");
    }

    #[test]
    fn corner_radius_turns_a_step_into_a_chamfer() {
        // An L: the convex corner has to gain arc vertices, and no boundary
        // turn may exceed 45° at half a module.
        let mut leaf = rect_spec(Vec2::new(30.0, 0.0), Vec2::new(4.0, 30.0), 0);
        leaf.style.cell = 2.0;
        leaf.style.corner = 1.0;
        leaf.style.inner_corner = 0.7;
        let stem = rect_spec(Vec2::new(38.0, -13.0), Vec2::new(20.0, 4.0), 0);
        let mut stem = stem;
        stem.style.cell = 2.0;
        stem.style.corner = 1.0;
        stem.style.inner_corner = 0.7;
        let grid = PavingGrid::build(&[leaf, stem]);
        let meshes = paving_meshes(&test_grid(), &grid);
        assert_eq!(meshes.len(), 1, "uma cor, um grupo");
        let mesh = &meshes[0].1;
        let plain = grid.len() * 5;
        assert!(
            mesh.positions.len() > plain,
            "os cantos convexos têm de acrescentar vértices de arco: {} vs {plain}",
            mesh.positions.len()
        );
        // Max turn between consecutive outline segments, over every cell.
        let max_turn = max_boundary_turn(&grid);
        assert!(
            max_turn <= 45.0 + 1e-3,
            "nenhum canto de contorno pode virar mais de 45°: {max_turn:.2}°"
        );
    }

    /// Largest direction change between two consecutive **rim** segments of
    /// any cell outline, in degrees. Interior edges (shared with a cell of the
    /// same group) turn 90° by construction and are not part of the visible
    /// boundary, so they are excluded.
    fn max_boundary_turn(grid: &PavingGrid) -> f32 {
        let module = grid.module();
        let half = module * 0.5;
        let mut worst: f32 = 0.0;
        for (&(i, j), &gid) in &grid.cells {
            let group = &grid.groups()[gid as usize];
            let edge = [
                grid.edge(i, j, gid, (1, 0)),
                grid.edge(i, j, gid, (0, 1)),
                grid.edge(i, j, gid, (-1, 0)),
                grid.edge(i, j, gid, (0, -1)),
            ];
            let diag = [
                grid.group_at_cell(i + 1, j - 1),
                grid.group_at_cell(i + 1, j + 1),
                grid.group_at_cell(i - 1, j + 1),
                grid.group_at_cell(i - 1, j - 1),
            ];
            let (outline, _) = cell_outline(
                grid.cell_center(i, j),
                half,
                edge,
                diag,
                group.corner,
                group.inner_corner,
            );
            let n = outline.nodes.len();
            // The turn *at* vertex `v` uses the segments `v-1 → v` and
            // `v → v+1`, so `wall[v-1]` and `wall[v]` gate it.
            for v in 0..n {
                if !outline.wall[(v + n - 1) % n] || !outline.wall[v] {
                    continue;
                }
                let prev = outline.nodes[(v + n - 1) % n];
                let here = outline.nodes[v];
                let next = outline.nodes[(v + 1) % n];
                let d0 = (here - prev).normalize_or_zero();
                let d1 = (next - here).normalize_or_zero();
                if d0 == Vec2::ZERO || d1 == Vec2::ZERO {
                    continue;
                }
                let angle = d0.perp_dot(d1).atan2(d0.dot(d1)).abs().to_degrees();
                worst = worst.max(angle);
            }
        }
        worst
    }

    #[test]
    fn adjacent_cells_share_the_boundary() {
        // Interior edges must land on identical positions, or the merged
        // surface cracks.
        let grid = PavingGrid::build(&[rect_spec(Vec2::ZERO, Vec2::splat(12.0), 0)]);
        let module = grid.module();
        // Cell (0,0) and (1,0) are neighbours (12 m rect covers -6..6).
        let (a, _) = cell_outline(
            grid.cell_center(0, 0),
            module * 0.5,
            [
                grid.edge(0, 0, 0, (1, 0)),
                grid.edge(0, 0, 0, (0, 1)),
                grid.edge(0, 0, 0, (-1, 0)),
                grid.edge(0, 0, 0, (0, -1)),
            ],
            [None; 4],
            0.0,
            0.0,
        );
        let (b, _) = cell_outline(
            grid.cell_center(1, 0),
            module * 0.5,
            [
                grid.edge(1, 0, 0, (1, 0)),
                grid.edge(1, 0, 0, (0, 1)),
                grid.edge(1, 0, 0, (-1, 0)),
                grid.edge(1, 0, 0, (0, -1)),
            ],
            [None; 4],
            0.0,
            0.0,
        );
        // The shared edge is x = module; both outlines carry it, at the same
        // subdivided positions.
        let on_edge = |o: &Outline| -> Vec<f32> {
            let mut v: Vec<f32> = o
                .nodes
                .iter()
                .filter(|p| (p.x - module).abs() < 1e-4)
                .map(|p| p.y)
                .collect();
            v.sort_by(f32::total_cmp);
            v
        };
        assert_eq!(on_edge(&a), on_edge(&b), "aresta interior partilhada");
    }

    #[test]
    fn drapes_the_terrain() {
        let terrain = test_grid();
        let grid = PavingGrid::build(&[rect_spec(Vec2::new(30.0, 0.0), Vec2::splat(20.0), 0)]);
        let meshes = paving_meshes(&terrain, &grid);
        let mesh = &meshes[0].1;
        let mut worst: f32 = 0.0;
        for p in &mesh.positions {
            let want = terrain.sample(p[0], p[2]) + PAVING_LIFT;
            worst = worst.max((p[1] - want).abs());
        }
        assert!(worst < 1.0e-3, "drape error {worst}");
        // And the surface is not a single plane over the slope: a cell's rim
        // and hub sample different heights, which is the whole point of
        // subdividing the outline and fanning from the hub.
        let rim: Vec<f32> = mesh
            .positions
            .iter()
            .map(|p| p[1])
            .collect();
        let spread = rim.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - rim.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(spread > 0.05, "a malha tem de seguir a colina: {spread}");
    }

    #[test]
    fn feather_reaches_zero_alpha() {
        let terrain = test_grid();
        let mut spec = rect_spec(Vec2::new(30.0, 0.0), Vec2::splat(12.0), 0);
        spec.feather = 1.5;
        let grid = PavingGrid::build(&[spec]);
        let meshes = paving_meshes(&terrain, &grid);
        let mesh = &meshes[0].1;
        assert!(
            mesh.colors.iter().any(|c| c[3] < 1.0e-3),
            "a feather tem de chegar a alpha 0"
        );
        assert!(
            mesh.colors.iter().any(|c| c[3] > 0.999),
            "e o miolo tem de ser opaco"
        );
    }

    #[test]
    fn uvs_are_world_space() {
        let terrain = test_grid();
        let mut spec = rect_spec(Vec2::new(30.0, 0.0), Vec2::splat(8.0), 0);
        spec.texture_scale = 4.0;
        let grid = PavingGrid::build(&[spec]);
        let meshes = paving_meshes(&terrain, &grid);
        let mesh = &meshes[0].1;
        for (k, uv) in mesh.uvs.iter().enumerate() {
            let p = mesh.positions[k];
            assert!((uv[0] - p[0] / 4.0).abs() < 1e-5);
            assert!((uv[1] - p[2] / 4.0).abs() < 1e-5);
        }
    }

    #[test]
    fn every_triangle_faces_up() {
        let terrain = test_grid();
        let mut spec = rect_spec(Vec2::new(30.0, 0.0), Vec2::new(14.0, 6.0), 0);
        spec.style.corner = 1.0;
        spec.style.inner_corner = 0.7;
        spec.feather = 1.0;
        let grid = PavingGrid::build(&[spec]);
        let meshes = paving_meshes(&terrain, &grid);
        for (_, mesh) in &meshes {
            for tri in mesh.indices.as_chunks::<3>().0 {
                let p = |i: u32| {
                    let v = mesh.positions[i as usize];
                    Vec2::new(v[0], v[2])
                };
                let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
                assert!(
                    up_normal(a, b, c) > -1e-6,
                    "triângulo invertido: {a:?} {b:?} {c:?} → {}",
                    up_normal(a, b, c)
                );
            }
        }
    }

    #[test]
    fn deterministic_across_runs() {
        let terrain = test_grid();
        let mut spec = rect_spec(Vec2::new(30.0, 0.0), Vec2::splat(12.0), 0);
        spec.style.corner = 0.9;
        spec.style.inner_corner = 0.6;
        spec.feather = 1.2;
        spec.noise = 0.3;
        let hash = || {
            let grid = PavingGrid::build(&[spec.clone()]);
            let meshes = paving_meshes(&terrain, &grid);
            let m = &meshes[0].1;
            let mut h = 0u64;
            for p in &m.positions {
                for v in p {
                    h = h.wrapping_mul(31).wrapping_add(v.to_bits() as u64);
                }
            }
            for i in &m.indices {
                h = h.wrapping_mul(31).wrapping_add(*i as u64);
            }
            h
        };
        assert_eq!(hash(), hash(), "mesma spec ⇒ mesma malha");
    }

    #[test]
    fn one_mesh_per_group_and_chunk() {
        let cobble = PavingSpec {
            base_color: [0.5, 0.5, 0.5],
            shapes: vec![PavingShape::Rect {
                at: Vec2::ZERO,
                size: Vec2::splat(20.0),
            }],
            style: PavingStyle {
                chunk: 8.0,
                ..PavingStyle::default()
            },
            ..PavingSpec::default()
        };
        let dirt = PavingSpec {
            base_color: [0.4, 0.3, 0.2],
            shapes: vec![PavingShape::Rect {
                at: Vec2::new(40.0, 0.0),
                size: Vec2::splat(20.0),
            }],
            style: PavingStyle {
                chunk: 8.0,
                ..PavingStyle::default()
            },
            ..PavingSpec::default()
        };
        let grid = PavingGrid::build(&[cobble, dirt]);
        let meshes = paving_meshes(&test_grid(), &grid);
        assert_eq!(grid.groups().len(), 2, "dois materiais distintos");
        // 20 m at an 8 m bucket spans at least 3 buckets per axis per group,
        // so the merge is chunked rather than one giant mesh.
        assert!(meshes.len() >= 6, "meshes = {}", meshes.len());
        assert!(meshes.iter().all(|(_, m)| !m.indices.is_empty()));
    }

    #[test]
    fn run_corridor_keeps_the_authored_width() {
        // A 4 m road centred on the lattice line x = 0 covers exactly two
        // cell columns: the tangent boxes on either side are excluded.
        let spec = PavingSpec {
            shapes: vec![PavingShape::Run {
                stations: (-20..=20).map(|k| Vec2::new(0.0, k as f32)).collect(),
                half_width: vec![2.0; 41],
            }],
            ..PavingSpec::default()
        };
        let grid = PavingGrid::build(&[spec]);
        let width = (-8..8)
            .filter(|i| grid.group_at_cell(*i, 0).is_some())
            .count();
        assert_eq!(width, 2, "4 m de estrada = 2 células de 2 m");
        // No pinholes: every row inside the corridor is paved.
        for j in -9..9 {
            assert!(
                grid.group_at_cell(-1, j).is_some() && grid.group_at_cell(0, j).is_some(),
                "linha {j} com buraco"
            );
        }
    }

    #[test]
    fn fillet_fills_the_notch_without_overlap() {
        // Three cells in an L leave the diagonal (1,1) empty: cell (0,0) has
        // both its E and N edges open onto the same group and an unowned
        // diagonal, which is exactly a concave corner.
        let spec = PavingSpec {
            shapes: vec![
                PavingShape::Rect {
                    at: Vec2::new(2.0, 1.0),
                    size: Vec2::new(4.0, 2.0),
                },
                PavingShape::Rect {
                    at: Vec2::new(1.0, 3.0),
                    size: Vec2::new(2.0, 2.0),
                },
            ],
            style: PavingStyle {
                inner_corner: 0.7,
                ..PavingStyle::default()
            },
            ..PavingSpec::default()
        };
        let grid = PavingGrid::build(&[spec]);
        assert!(grid.group_at_cell(1, 1).is_none(), "a diagonal fica vazia");
        let (_, fillets) = cell_outline(
            grid.cell_center(0, 0),
            grid.module() * 0.5,
            [
                grid.edge(0, 0, 0, (1, 0)),
                grid.edge(0, 0, 0, (0, 1)),
                grid.edge(0, 0, 0, (-1, 0)),
                grid.edge(0, 0, 0, (0, -1)),
            ],
            [
                grid.group_at_cell(1, -1),
                grid.group_at_cell(1, 1),
                grid.group_at_cell(-1, 1),
                grid.group_at_cell(-1, -1),
            ],
            0.0,
            0.7,
        );
        assert_eq!(fillets.len(), 1, "um canto côncavo, um filete");
        assert_eq!(fillets[0].len(), 1 + CORNER_SEGS + 1);

        // And the patch stays in the unpaved diagonal: no vertex of the whole
        // merged surface may land inside a cell owned by a *different* group
        // or stray far from the notch.
        let meshes = paving_meshes(&test_grid(), &grid);
        let module = grid.module();
        let mut spill = 0;
        for (_, mesh) in &meshes {
            for p in &mesh.positions {
                let q = Vec2::new(p[0], p[2]);
                let (i, j) = grid.cell_index(q);
                if grid.group_at_cell(i, j).is_none() {
                    spill += 1;
                    let (min, max) = (
                        Vec2::new(i as f32, j as f32) * module,
                        Vec2::new(i as f32 + 1.0, j as f32 + 1.0) * module,
                    );
                    assert!(
                        point_box_distance(q, min, max) <= 0.7,
                        "vértice {q:?} caiu longe do canto, isso não é um filete"
                    );
                }
            }
        }
        assert!(spill > 0, "o canto côncavo tem de produzir um filete");
    }

    /// A material declared later fades over the one it borders: the band is
    /// drawn with the upper material, inside the lower material's cells, a
    /// couple of millimetres above them. Without it a dirt street cuts a
    /// cobbled square with a razor line.
    #[test]
    fn the_later_material_feathers_over_the_earlier_one() {
        let cobble = PavingSpec {
            base_color: [0.6, 0.6, 0.6],
            feather: 1.0,
            shapes: vec![PavingShape::Rect {
                at: Vec2::ZERO,
                size: Vec2::splat(20.0),
            }],
            ..PavingSpec::default()
        };
        // Declared after → paints on top. Sits at z ∈ [4, 12] on the cobbles.
        let dirt = PavingSpec {
            base_color: [0.4, 0.3, 0.2],
            feather: 1.0,
            shapes: vec![PavingShape::Rect {
                at: Vec2::new(0.0, 8.0),
                size: Vec2::new(6.0, 8.0),
            }],
            ..PavingSpec::default()
        };
        let grid = PavingGrid::build(&[cobble, dirt]);
        let terrain = test_grid();
        let meshes = paving_meshes(&terrain, &grid);
        assert_eq!(grid.groups().len(), 2);

        let of = |colour: [f32; 3]| -> &ChunkMeshData {
            let gid = grid
                .groups()
                .iter()
                .position(|g| g.base_color == colour)
                .expect("grupo");
            let (_, mesh) = meshes
                .iter()
                .find(|(g, _)| *g == gid)
                .expect("malha do grupo");
            mesh
        };

        // The dirt band lives inside the cobble cells, on the seam at z = 4,
        // and its alpha ramps.
        let dirt_mesh = of([0.4, 0.3, 0.2]);
        let ramp = dirt_mesh
            .positions
            .iter()
            .zip(&dirt_mesh.colors)
            .filter(|(p, c)| p[2] < 3.99 && p[2] > 2.0 && c[3] > 0.001 && c[3] < 0.999)
            .count();
        assert!(ramp > 0, "a terra tem de esbater-se por cima da calçada");
        // And it is *above* the pavement it covers.
        let lifted = dirt_mesh
            .positions
            .iter()
            .zip(&dirt_mesh.colors)
            .any(|(p, c)| {
                c[3] > 0.5
                    && p[2] < 3.99
                    && (p[1] - terrain.sample(p[0], p[2]) - PAVING_LIFT - SEAM_LIFT).abs() < 1e-3
            });
        assert!(lifted, "a banda sobe o suficiente para ganhar o depth test");

        // The cobbles do not feather into the dirt: the older material stops
        // at the lattice line.
        let cobble_mesh = of([0.6, 0.6, 0.6]);
        let leak = cobble_mesh
            .positions
            .iter()
            .zip(&cobble_mesh.colors)
            .filter(|(p, c)| p[2] > 4.01 && p[2] < 6.0 && p[0].abs() < 3.0 && c[3] > 0.001 && c[3] < 0.999)
            .count();
        assert_eq!(leak, 0, "a calçada não deve esbater-se para dentro da terra");
    }

    #[test]
    fn empty_paving_is_cheap() {
        let grid = PavingGrid::build(&[]);
        assert!(grid.is_empty());
        assert_eq!(grid.module(), 0.0);
        assert!(paving_meshes(&test_grid(), &grid).is_empty());
        assert!(grid.bounds().is_none());
    }

    /// The cell walk order and the diagonal indexing have to agree, or the
    /// chamfer lands on the wrong corner.
    #[test]
    fn corner_kinds_follow_the_neighbour_mask() {
        // A rect that fits exactly in one cell: all four corners convex → all
        // chamfered. At half a module the arcs weld at the edge midpoints and
        // the cell becomes a disc.
        let spec = rect_spec(Vec2::new(31.0, 1.0), Vec2::splat(2.0), 0);
        let grid = PavingGrid::build(&[spec]);
        assert_eq!(grid.len(), 1, "2 m em módulo 2 = uma célula");
        assert!(grid.group_at_cell(15, 0).is_some(), "célula isolada");
        let (outline, _) = cell_outline(
            grid.cell_center(15, 0),
            grid.module() * 0.5,
            [Edge::Wall; 4],
            [None; 4],
            1.0,
            0.7,
        );
        // Four chamfer arcs of CORNER_SEGS + 1 points, welded at the edge
        // midpoints: the outline becomes a ring of 4 × 4 points.
        assert_eq!(outline.nodes.len(), 4 * (CORNER_SEGS + 1) - 4);
        assert!(outline.wall.iter().all(|w| *w), "célula isolada só tem rim");
    }
}
