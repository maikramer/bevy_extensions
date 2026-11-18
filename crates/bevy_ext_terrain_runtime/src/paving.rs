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

use std::collections::{BTreeMap, BTreeSet};

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
/// Target arc length between two samples of a corner arc (meters). The
/// segment count follows the radius, so a 4 m kerb radius is a real curve and
/// a 20 cm one is not 24 triangles.
const ARC_STEP: f32 = 0.55;
/// Bounds on the corner arc segments.
const MIN_ARC_SEGS: usize = 3;
const MAX_ARC_SEGS: usize = 24;
/// Auto rim wobble is **off**. A paved kerb is a kerb: sliding it by nearly
/// half a module (the old default) turned a 4 m street into a wavy stain.
/// An authored `paving-wobble` still works, capped so the cell fan stays valid.
const DEFAULT_WOBBLE: f32 = 0.0;
const MAX_WOBBLE: f32 = 0.35;
/// Rings across the feather band. The alpha ramp is piecewise-linear per
/// triangle, so too few rings makes every quad's diagonal readable as a facet
/// — five keeps the dissolve smooth without adding meaningful cost (the band
/// is a couple of hundred quads).
const FEATHER_RINGS: usize = 5;
/// A resolved radius under this is treated as "off" (meters).
const MIN_RADIUS: f32 = 0.02;
/// How hard the shared rim is relaxed toward its neighbours (0 = the raw
/// lattice, 1 = collapse). A staircase rim becomes a curve and a 90° corner
/// rounds; a straight run is untouched because its neighbours are collinear.
const RIM_RELAX: f32 = 0.5;
/// Relaxation passes. Each one reaches about one sample further, so three
/// passes smooth across ~3 m — enough to erase a one-cell step, not enough to
/// eat a piece of the square.
const RIM_RELAX_PASSES: usize = 8;
/// Quantisation of a rim position used to key the shared smoothing (1 cm).
const RIM_SNAP: f32 = 100.0;
/// Length under which two outline points collapse into one (meters).
const WELD: f32 = 1e-4;

/// Grid tuning of a paved node (`paving-cell`, `paving-corner`, …).
///
/// Deliberately free of material and shape: the owning
/// [`super::decal::GroundDecalSpec`] / [`super::roads::RoadSpec`] keeps its
/// `texture`, `texture-scale`, `base-color`, `roughness`, `feather`, `noise`,
/// `seed` and `lift`, and [`PavingSpec`] copies them in. One vocabulary, one
/// place.
#[derive(Debug, Clone, PartialEq, Reflect)]
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
    /// Feather band width (meters, `paving-feather`). Negative = derive from
    /// the owning node: a road's `edge-feather` is a fraction of its half
    /// width, a decal's `feather` is already meters.
    pub feather: f32,
    /// Rim wobble amplitude (meters, `paving-wobble`). Negative = auto (`0`,
    /// a crisp kerb). The fade band still breathes with `noise`; the opaque
    /// edge does not, or a street stops reading as paving.
    pub wobble: f32,
}

impl Default for PavingStyle {
    /// Every field starts at its "inherit" sentinel. A derived `Default` would
    /// zero them, and `0` means "no feather" and "no chamfer" rather than
    /// "pick for me" — that footgun already swallowed one test round.
    fn default() -> Self {
        Self {
            cell: 0.0,
            corner: -1.0,
            inner_corner: -1.0,
            chunk: -1.0,
            priority: 0,
            round: false,
            feather: -1.0,
            wobble: -1.0,
        }
    }
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
                feather: -1.0,
                wobble: -1.0,
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
    /// Rim wobble amplitude (meters) — see [`PavingStyle::wobble`].
    pub wobble: f32,
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
            && self.lift == other.lift
            && self.corner == other.corner
            && self.inner_corner == other.inner_corner
            && self.chunk == other.chunk
            && self.wobble == other.wobble
    }
}

/// A rasterized road corridor: the smoothed centreline plus the half width
/// per station. Kept so the road's **rim can follow the road's own curve**
/// instead of the lattice staircase the rasterization produces.
#[derive(Debug, Clone, Reflect)]
#[type_path = "viber::terrain::paving"]
pub struct PavingCorridor {
    stations: Vec<Vec2>,
    half_width: Vec<f32>,
    group: u16,
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
    corridors: Vec<PavingCorridor>,
    /// Which run corridors claimed each cell. A rim vertex may only follow a
    /// road it actually belongs to: otherwise a plaza vertex near a passing
    /// artery is dragged onto the artery's edge, and the square's rim turns
    /// into saw teeth.
    cell_runs: BTreeMap<(i32, i32), Vec<usize>>,
    /// Cells claimed by a rect or a disc (plaza, street, apron). A corridor
    /// draws a ribbon everywhere else; it does not paint over these, and it
    /// does not drag their outline onto the road.
    area_cells: BTreeSet<(i32, i32)>,
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
                feather: if spec.style.feather >= 0.0 {
                    spec.style.feather
                } else {
                    spec.feather.max(0.0)
                },
                noise: spec.noise.clamp(0.0, 1.0),
                seed: spec.seed,
                lift: spec.lift(),
                corner: resolve_radius(spec.style.corner, module * 0.5),
                inner_corner: resolve_radius(spec.style.inner_corner, module * 0.35),
                chunk: spec.style.chunk(),
                priority: spec.style.priority,
                wobble: resolve_wobble(spec.style.wobble, module),
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
                let run = if let PavingShape::Run {
                    stations,
                    half_width,
                } = shape
                    && stations.len() >= 2
                {
                    grid.corridors.push(PavingCorridor {
                        stations: stations.clone(),
                        half_width: half_width.clone(),
                        group: gid,
                    });
                    Some((grid.corridors.len() - 1) as usize)
                } else {
                    None
                };
                grid.rasterize(shape, gid, run);
            }
        }
        grid
    }

    /// Claims every cell the shape touches. A cell's square only has to
    /// *intersect* the shape, so touching shapes never leave a one-cell gap;
    /// the cost is that a shape grows by at most one module per side
    /// (authoring in multiples of the module keeps it exact).
    fn rasterize(&mut self, shape: &PavingShape, gid: u16, run: Option<usize>) {
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
                        self.claim((i, j), gid, true);
                    }
                }
            }
            PavingShape::Disc { at, radius } => {
                if *radius <= 0.0 {
                    return;
                }
                self.stamp(*at, *radius, gid, None, true);
            }
            PavingShape::Run {
                stations,
                half_width,
            } => {
                let Some(run) = run else {
                    return;
                };
                for (k, a) in stations.iter().enumerate() {
                    let hw = half_width.get(k).copied().unwrap_or(1.0).max(0.0);
                    if hw <= 0.0 {
                        continue;
                    }
                    match stations.get(k + 1) {
                        Some(b) => self.stamp_segment(*a, *b, hw, gid, run),
                        // The last station of an open path: a disc, so a road
                        // tip is not a hairline. Still a corridor, not an area.
                        None => self.stamp(*a, hw, gid, Some(run), false),
                    }
                }
            }
        }
    }

    /// Claims the cells within `radius` of `at`.
    fn stamp(&mut self, at: Vec2, radius: f32, gid: u16, run: Option<usize>, area: bool) {
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
                    self.claim((i, j), gid, area);
                    if let Some(r) = run
                        && self.cells.get(&(i, j)) == Some(&gid)
                    {
                        let entry = self.cell_runs.entry((i, j)).or_default();
                        if entry.last() != Some(&r) {
                            entry.push(r);
                        }
                    }
                }
            }
        }
    }

    /// Claims the cells within `half_width` of the segment `a → b`.
    fn stamp_segment(&mut self, a: Vec2, b: Vec2, half_width: f32, gid: u16, run: usize) {
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
                        self.claim((i, j), gid, false);
                        if self.cells.get(&(i, j)) == Some(&gid) {
                            let entry = self.cell_runs.entry((i, j)).or_default();
                            if entry.last() != Some(&run) {
                                entry.push(run);
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    fn claim(&mut self, key: (i32, i32), gid: u16, area: bool) {
        let prev = self.cells.get(&key).copied();
        let replace = match prev {
            None => true,
            // `>=` on purpose: iterating in declaration order, a tie lets the
            // later node win, which is the reading order of the file.
            Some(cur) => self.groups[gid as usize].priority >= self.groups[cur as usize].priority,
        };
        if !replace {
            return;
        }
        // A different material taking the cell drops the previous owner's
        // area flag and run list. The same material re-claiming (a road
        // crossing its own plaza) keeps the area flag: the rect still owns
        // the ground, the road just shares the group.
        if prev.is_some() && prev != Some(gid) {
            self.area_cells.remove(&key);
            self.cell_runs.remove(&key);
        }
        self.cells.insert(key, gid);
        if area {
            self.area_cells.insert(key);
        }
    }

    /// Direction the pavement rim moves along at a **lattice** position, or
    /// `None` when that node must stay put.
    ///
    /// This has to come from the lattice and not from a cell's own outline:
    /// two cells that meet at a node each build their own outline, and if each
    /// derived the direction locally they would slide the shared vertex two
    /// different ways and tear the surface. Classifying by the cells around
    /// the point gives every owner the same answer, so a straight rim wobbles
    /// continuously across cell boundaries with no scalloping and no cracks.
    ///
    /// Only an **unpaved** neighbour lets a node move. A cell of another
    /// material is a seam: it computes the opposite direction for the same
    /// point, and the material boundary has to stay on the lattice anyway.
    fn rim_dir(&self, gid: u16, p: Vec2) -> Option<Vec2> {
        let m = self.module;
        if m <= 0.0 {
            return None;
        }
        /// `0` unpaved, `1` ours, `2` another material.
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Side {
            Empty,
            Ours,
            Other,
        }
        const TOL: f32 = 1.0e-3;
        let fi = p.x / m;
        let fj = p.y / m;
        let (i, j) = (fi.floor() as i32, fj.floor() as i32);
        let (u, v) = (fi - i as f32, fj - j as f32);
        let ox = if u < TOL {
            -1
        } else if u > 1.0 - TOL {
            1
        } else {
            0
        };
        let oy = if v < TOL {
            -1
        } else if v > 1.0 - TOL {
            1
        } else {
            0
        };
        let side = |di: i32, dj: i32| match self.cells.get(&(i + di, j + dj)) {
            None => Side::Empty,
            Some(&g) if g == gid => Side::Ours,
            Some(_) => Side::Other,
        };
        // `line` answers a two-cell boundary: move only from ours into empty.
        let line = |a: Side, b: Side, dir: Vec2| match (a, b) {
            (Side::Ours, Side::Empty) => Some(dir),
            (Side::Empty, Side::Ours) => Some(-dir),
            _ => None,
        };
        match (ox, oy) {
            // Strictly inside a cell: only one cell sees this node.
            (0, 0) => None,
            // On a vertical lattice line: the two cells either side.
            (sx, 0) => {
                let left = if sx < 0 { -1 } else { 0 };
                line(side(left, 0), side(left + 1, 0), Vec2::new(1.0, 0.0))
            }
            (0, sy) => {
                let below = if sy < 0 { -1 } else { 0 };
                line(side(0, below), side(0, below + 1), Vec2::new(0.0, 1.0))
            }
            // Lattice corner: four cells. Only a rim that runs *straight*
            // through the corner may move; a convex corner (one cell), a
            // notch (three) or a pinch (two diagonal) stays crisp.
            (sx, sy) => {
                let (x0, x1) = if sx < 0 { (-1, 0) } else { (0, 1) };
                let (y0, y1) = if sy < 0 { (-1, 0) } else { (0, 1) };
                let q = [side(x0, y0), side(x1, y0), side(x0, y1), side(x1, y1)];
                let ours = q.iter().filter(|s| **s == Side::Ours).count();
                let empty = q.iter().filter(|s| **s == Side::Empty).count();
                // One cell and nothing else: a convex corner of the region
                // that no other paved cell touches, so it can slide along the
                // diagonal and round the corner off. (Any other material in
                // the three remaining quadrants makes it a seam — crisp.)
                if ours == 1 && empty == 3 {
                    // Away from whichever quadrant owns the point.
                    let x = if q[0] == Side::Ours || q[2] == Side::Ours {
                        1.0
                    } else {
                        -1.0
                    };
                    let y = if q[0] == Side::Ours || q[1] == Side::Ours {
                        1.0
                    } else {
                        -1.0
                    };
                    return Some(Vec2::new(x, y).normalize());
                }
                if ours != 2 || empty != 2 {
                    return None;
                }
                // Both owned cells in one column → a vertical rim through the
                // corner; both in one row → a horizontal one.
                if q[0] == Side::Ours && q[2] == Side::Ours {
                    return Some(Vec2::new(1.0, 0.0));
                }
                if q[1] == Side::Ours && q[3] == Side::Ours {
                    return Some(Vec2::new(-1.0, 0.0));
                }
                if q[0] == Side::Ours && q[1] == Side::Ours {
                    return Some(Vec2::new(0.0, 1.0));
                }
                if q[2] == Side::Ours && q[3] == Side::Ours {
                    return Some(Vec2::new(0.0, -1.0));
                }
                // Diagonal pair: a pinch, leave it alone.
                None
            }
        }
    }

    fn cell_box(&self, i: i32, j: i32) -> (Vec2, Vec2) {
        let m = self.module;
        (
            Vec2::new(i as f32, j as f32) * m,
            Vec2::new(i as f32 + 1.0, j as f32 + 1.0) * m,
        )
    }

    /// Moves a rim position onto the edge of the corridor it belongs to, when
    /// it is close to one.
    ///
    /// The lattice rasterizes a road into a staircase of cells, and no amount
    /// of relaxing the cell outlines can erase it: a cell only sees its own
    /// corner, and the fan from its centre caps how far a vertex may slide.
    /// The road already *has* a smooth curve — its own centreline — so a rim
    /// vertex within one module of a corridor's edge is placed at exactly
    /// `half_width` from it.
    ///
    /// This has to be a pure function of position (like [`rim_dir`]): two
    /// cells that share a vertex must compute the same answer or the merged
    /// surface tears. Interior vertices are far from the edge and stay on the
    /// lattice, so a square keeps its straight sides.
    pub fn corridor_snap(&self, gid: u16, runs: &[usize], p: Vec2) -> Vec2 {
        if runs.is_empty() || self.module <= 0.0 {
            return p;
        }
        // Wide enough to reach every rim vertex of the rasterized cells: a
        // cell square that touches the corridor edge has corners up to
        // half + module·√2/2 away, and those corners are the staircase.
        let band = self.module * 1.6;
        let mut best: Option<(f32, Vec2)> = None;
        for &r in runs {
            let corridor = &self.corridors[r];
            // Only the road's own surface follows the road. A plaza rim near a
            // passing artery must stay on the square, or the square turns into
            // saw teeth — that was the serration.
            if corridor.group != gid {
                continue;
            }
            let stations = &corridor.stations;
            if stations.len() < 2 {
                continue;
            }
            // Nearest segment (stations are ~1 m apart; a linear scan over a
            // few hundred of them is nothing at world-build time).
            let mut hit = (Vec2::ZERO, 0usize, 0.0f32, f32::INFINITY);
            for k in 0..stations.len() - 1 {
                let (a, b) = (stations[k], stations[k + 1]);
                let d = b - a;
                let len2 = d.length_squared();
                if len2 <= 1e-9 {
                    continue;
                }
                let t = ((p - a).dot(d) / len2).clamp(0.0, 1.0);
                let q = a + d * t;
                let dist2 = (p - q).length_squared();
                if dist2 < hit.3 {
                    hit = (q, k, t, dist2);
                }
            }
            let (q, k, t, _) = hit;
            let dir = stations[k + 1] - stations[k];
            let len = dir.length();
            if len <= 1e-6 {
                continue;
            }
            let normal = Vec2::new(dir.y, -dir.x) / len;
            let lateral = (p - q).dot(normal);
            let half = corridor
                .half_width
                .get(k)
                .copied()
                .unwrap_or(corridor.half_width[0]);
            // Measure against the TRUE distance to the segment, not just its
            // lateral component. Past a segment end (`t` clamped) the lateral
            // offset says nothing about how far the point is: a node 200 m out
            // along a road's extension has `lateral ≈ half` and used to be
            // snapped onto that road's end cap, stretching a cell into a
            // 300 m sheet (the "walls" and stair-steps across the city).
            let dist = (p - q).length();
            let off = (dist - half).abs();
            if off > band {
                continue;
            }
            let target = if t > 0.0 && t < 1.0 {
                q + normal * lateral.signum() * half
            } else if dist > 1e-6 {
                // Round cap at a corridor end / joint.
                q + (p - q) / dist * half
            } else {
                continue;
            };
            // Prefer the corridor this vertex is actually on the rim of.
            let score = off;
            if best.is_none_or(|(b, _)| score < b) {
                best = Some((score, target));
            }
        }
        best.map(|(_, q)| q).unwrap_or(p)
    }

    /// True when the lattice position sits on the boundary of the paved
    /// union — some cell around it is not owned by the same group as the
    /// rest. A pure function of position, like [`rim_dir`], so two cells that
    /// share a node agree on whether it may be snapped onto a corridor edge
    /// (interior nodes must stay put or one cell would move and the other
    /// not, tearing the surface).
    fn is_boundary(&self, p: Vec2) -> bool {
        let m = self.module;
        if m <= 0.0 {
            return false;
        }
        const TOL: f32 = 1.0e-3;
        let fi = p.x / m;
        let fj = p.y / m;
        let (i, j) = (fi.floor() as i32, fj.floor() as i32);
        let (u, v) = (fi - i as f32, fj - j as f32);
        let ox = if u < TOL {
            -1
        } else if u > 1.0 - TOL {
            1
        } else {
            0
        };
        let oy = if v < TOL {
            -1
        } else if v > 1.0 - TOL {
            1
        } else {
            0
        };
        let owner = |di: i32, dj: i32| self.cells.get(&(i + di, j + dj)).copied();
        match (ox, oy) {
            // Strictly inside a cell there are no lattice lines — the only
            // nodes there are a corner arc's samples, and a chamfer only ever
            // happens on the rim.
            (0, 0) => true,
            (sx, 0) => {
                let left = if sx < 0 { -1 } else { 0 };
                owner(left, 0) != owner(left + 1, 0)
            }
            (0, sy) => {
                let below = if sy < 0 { -1 } else { 0 };
                owner(0, below) != owner(0, below + 1)
            }
            (sx, sy) => {
                let (x0, x1) = if sx < 0 { (-1, 0) } else { (0, 1) };
                let (y0, y1) = if sy < 0 { (-1, 0) } else { (0, 1) };
                let q = [owner(x0, y0), owner(x1, y0), owner(x0, y1), owner(x1, y1)];
                // All four the same group — or all empty — means no boundary
                // here: an interior vertex (or ground outside the pavement).
                !(q[0] == q[1] && q[1] == q[2] && q[2] == q[3])
            }
        }
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

/// Rim wobble amplitude, clamped so the fan from the cell centre stays valid.
fn resolve_wobble(value: f32, module: f32) -> f32 {
    let w = if value < 0.0 {
        module * DEFAULT_WOBBLE
    } else {
        value
    };
    w.clamp(0.0, module * MAX_WOBBLE)
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

/// Ordered outline of one cell plus the frame each node needs to move and to
/// extrude a band from it.
struct Outline {
    nodes: Vec<Vec2>,
    /// `wall[i]` applies to the segment `nodes[i] → nodes[i+1]`.
    wall: Vec<bool>,
    /// `seam[i]`: the segment borders another material. Both `wall` and `seam`
    /// segments are boundary, and the *shared* rim smoothing treats them the
    /// same way — a diagonal material edge has to become a curve too, not a
    /// lattice staircase.
    seam: Vec<bool>,
    /// Nodes pulled onto a road's own curve. They are already smooth by
    /// construction, so the shared relaxation leaves them alone — relaxing
    /// them again just pushes them off the curve.
    snapped: Vec<bool>,
    /// Unit direction each node may slide along for the organic rim; zero
    /// where the node is welded to something that must stay put (an interior
    /// edge, a material seam, or a corner another cell could disagree about).
    move_dir: Vec<Vec2>,
    /// Mitred band direction per node, already divided by `cos(θ/2)` so the
    /// fan's outer rings keep a constant width across a corner instead of
    /// opening a wedge between two segments.
    band: Vec<Vec2>,
}

struct OutlineBuilder {
    nodes: Vec<Vec2>,
    wall: Vec<bool>,
    seam: Vec<bool>,
}

impl OutlineBuilder {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            wall: Vec::new(),
            seam: Vec::new(),
        }
    }

    /// Appends `p`; the flags describe the segment from the previous node and
    /// which of the cell's four edges produced it.
    fn push(&mut self, p: Vec2, wall: bool, seam: bool) {
        if let Some(&last) = self.nodes.last()
            && (p - last).length_squared() <= WELD * WELD
        {
            return;
        }
        if !self.nodes.is_empty() {
            self.wall.push(wall);
            self.seam.push(seam);
        }
        self.nodes.push(p);
    }

    /// Appends the interior subdivision of `from → to` (both exclusive).
    fn run(&mut self, from: Vec2, to: Vec2, wall: bool, seam: bool) {
        let len = (to - from).length();
        let steps = (len / MAX_EDGE).ceil() as usize;
        for k in 1..steps {
            let t = k as f32 / steps as f32;
            self.push(from + (to - from) * t, wall, seam);
        }
    }

    /// Closes the loop and derives each node's rim frame.
    ///
    /// `rim_dir` answers "which way may a node at this lattice position slide",
    /// from the occupancy grid; it returns `None` for anything that is not a
    /// straight rim crossing that point.
    fn finish(
        mut self,
        centre: Vec2,
        closing: (bool, bool),
        rim_dir: &dyn Fn(Vec2) -> Option<Vec2>,
        snap: &dyn Fn(Vec2) -> Vec2,
        is_boundary: &dyn Fn(Vec2) -> bool,
    ) -> Outline {
        let closing_wall = closing.0;
        let n0 = self.nodes.len();
        // Roads: pull the rim onto the corridor's own curve *before* any frame
        // is derived, so the mitre, the band and the smoothing all see the
        // same geometry the polygon will be emitted with. Boundary nodes only,
        // decided by a pure *position* test: the flags of this cell's own
        // segments miss the concave corner of a staircase step, where both
        // adjacent segments are interior yet the node is on the union boundary.
        // An interior vertex must stay exactly where the lattice put it.
        let mut snapped = vec![false; n0];
        for (k, p) in self.nodes.iter_mut().enumerate() {
            if is_boundary(*p) {
                let q = snap(*p);
                snapped[k] = q != *p;
                *p = q;
            }
        }
        // A cell whose chamfers weld at the edge midpoints (every corner at
        // half a module) walks back onto its first point. Drop the duplicate
        // so the ring has one node per position; the segment that brought us
        // there is the closing one.
        if self.nodes.len() > 2
            && (self.nodes[self.nodes.len() - 1] - self.nodes[0]).length_squared() <= WELD * WELD
        {
            self.nodes.pop();
            let closing = self.wall.pop().unwrap_or(closing_wall);
            self.wall.push(closing);
        } else {
            self.wall.push(closing_wall);
        }
        self.seam.push(closing.1);

        let n = self.nodes.len();
        // Per-segment outward normal, oriented away from the cell centre.
        let mut segn = Vec::with_capacity(n);
        for i in 0..n {
            let a = self.nodes[i];
            let b = self.nodes[(i + 1) % n];
            let d = b - a;
            let len = d.length();
            let mut nrm = if len > WELD {
                Vec2::new(d.y, -d.x) / len
            } else {
                Vec2::ZERO
            };
            if nrm.dot((a + b) * 0.5 - centre) < 0.0 {
                nrm = -nrm;
            }
            segn.push(nrm);
        }

        let mut move_dir = Vec::with_capacity(n);
        let mut band = Vec::with_capacity(n);
        for i in 0..n {
            let prev = segn[(i + n - 1) % n];
            let next = segn[i];
            let sum = prev + next;
            let mitre = sum.normalize_or_zero();
            let mitre = if mitre == Vec2::ZERO { next } else { mitre };
            let prev_rim = self.wall[(i + n - 1) % n] || self.seam[(i + n - 1) % n];
            let next_rim = self.wall[i] || self.seam[i];
            // The band only extrudes from *rim* segments, so it must only take
            // its direction from rim segments too. Mitring against a cell's
            // interior edge would flare the band diagonally at every cell
            // corner mid-rim — that is the 2 m staircase that the feather puts
            // along an otherwise straight kerb.
            band.push(match (prev_rim, next_rim) {
                (true, true) => {
                    // Mitre stretch 1/cos(θ/2), capped so a hairpin does not
                    // shoot the band to infinity (as the road ribbon does).
                    let cos_half = mitre.dot(next).clamp(0.5, 1.0);
                    mitre * (1.0 / cos_half)
                }
                (true, false) => prev,
                (false, true) => next,
                (false, false) => Vec2::ZERO,
            });

            // Where the node may go:
            //  * on the lattice, the direction comes from the occupancy grid so
            //    every cell that touches the node agrees (see `rim_dir`);
            //  * inside a cell (a corner arc, or its tangent point) only this
            //    cell sees it, so the local mitre is safe — as long as the
            //    corner is actually *smooth* (a 90° un-chamfered corner is
            //    shared with the diagonal cell and must not move).
            let both_rim = self.wall[(i + n - 1) % n] && self.wall[i];
            let smooth = prev != Vec2::ZERO && next != Vec2::ZERO && prev.dot(next) > 0.5;
            move_dir.push(match rim_dir(self.nodes[i]) {
                Some(d) => d,
                None if both_rim && smooth => mitre,
                None => Vec2::ZERO,
            });
        }

        Outline {
            nodes: self.nodes,
            wall: self.wall,
            seam: self.seam,
            snapped,
            move_dir,
            band,
        }
    }
}

/// Segments a corner arc of radius `r` and angular span `sweep` is sampled
/// with: one per [`ARC_STEP`] of arc length, clamped. A 4 m radius becomes a
/// curve; a 20 cm nick stays cheap.
fn arc_segments(r: f32, sweep: f32) -> usize {
    let len = r * sweep.abs();
    ((len / ARC_STEP).ceil() as usize).clamp(MIN_ARC_SEGS, MAX_ARC_SEGS)
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
    rim_dir: &dyn Fn(Vec2) -> Option<Vec2>,
    snap: &dyn Fn(Vec2) -> Vec2,
    is_boundary: &dyn Fn(Vec2) -> bool,
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
                pts.push(arc(
                    arc_center,
                    start,
                    end,
                    p,
                    arc_segments(corner, std::f32::consts::FRAC_PI_2),
                ));
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
                patch.extend(arc(
                    arc_center,
                    start,
                    end,
                    p,
                    arc_segments(inner_corner, std::f32::consts::FRAC_PI_2),
                ));
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
        let in_seam = edge[(k + 3) % 4] == Edge::Seam;
        for (n, p) in pts[k].iter().enumerate() {
            b.push(
                *p,
                if n == 0 { in_wall } else { corner_wall[k] },
                if n == 0 { in_seam } else { false },
            );
        }
        // Straight run along edge k to the first point of the next corner.
        let from = *pts[k].last().expect("corner has points");
        let to = pts[(k + 1) % 4][0];
        b.run(from, to, edge[k] == Edge::Wall, edge[k] == Edge::Seam);
        if k == 3 {
            closing_wall = edge[3] == Edge::Wall;
        }
    }
    (
        b.finish(
            center,
            (closing_wall, edge[3] == Edge::Seam),
            rim_dir,
            snap,
            is_boundary,
        ),
        fillets,
    )
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
    // Three harmonics whose amplitudes sum to 1: long lobes (~13 m and ~8 m)
    // with a smaller cross term so the edge never repeats on a recognisable
    // period. Pure sin of the world position, so two cells meeting at a rim
    // vertex compute exactly the same offset.
    // Wavelengths of ~22 m, ~14 m and ~9 m. Below ~6 m the 1 m rim sampling
    // reads the wobble as jitter instead of a lobe (a 4 m harmonic made the
    // kerb serrated); above ~30 m a 36 m square shows a single straight
    // stretch and reads as a ruler again.
    0.55 * (x * 0.28 + s).sin()
        + 0.30 * (z * 0.45 + s * 1.7).sin()
        + 0.15 * ((x + z) * 0.70 + s * 2.3).sin()
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

/// Local curvature radius at each outline node, from the turn between its two
/// segments. `INFINITY` on a straight run, small on a tight corner — the
/// feather band clamps its width to it so it can never turn inside out.
fn curvature_radius(nodes: &[Vec2], dir: &[Vec2], wall: &[bool], seam: &[bool]) -> Vec<f32> {
    let n = nodes.len();
    let mut out = vec![f32::INFINITY; n];
    let boundary =
        |i: usize| wall.get(i).copied().unwrap_or(false) || seam.get(i).copied().unwrap_or(false);
    for k in 0..n {
        // A node on the pavement rim is often also the corner of the *cell*,
        // where the next segment is an interior edge shared with the
        // neighbour. That 90° is not curvature of the boundary — taking it as
        // such pinches the feather band once per cell.
        if dir[k] == Vec2::ZERO || !boundary((k + n - 1) % n) || !boundary(k) {
            continue;
        }
        let prev = nodes[(k + n - 1) % n];
        let here = nodes[k];
        let next = nodes[(k + 1) % n];
        let (a, b) = (here - prev, next - here);
        let (la, lb) = (a.length(), b.length());
        if la <= WELD || lb <= WELD {
            continue;
        }
        let cos = (a / la).dot(b / lb).clamp(-1.0, 1.0);
        let turn = cos.acos();
        if turn <= 1.0e-3 {
            continue;
        }
        // A polygon inscribed in a circle of radius R turns by `turn` over a
        // chord of ~(la+lb)/2, i.e. R ≈ chord / (2 sin(turn/2)).
        out[k] = ((la + lb) * 0.5) / (2.0 * (turn * 0.5).sin()).max(1.0e-4);
    }
    out
}

/// Smooths the **shared** rim, in world space.
///
/// The outline of one cell cannot be smooth on its own: a diagonal rim is a
/// staircase of cells, and each cell only sees its own corner, so every cell
/// contributes its own little arc and the result is a row of fish scales. The
/// fix is to treat all the rim segments of a group as one polyline — each rim
/// node knows its two neighbours *along the boundary*, whoever owns it — and
/// relax it. Two cells that share a node look up the same smoothed position,
/// so the merged surface stays watertight.
///
/// A straight run is a fixed point (its neighbours are collinear), a 90°
/// corner rounds, a staircase becomes a curve. Nodes with anything other than
/// exactly two rim neighbours are pinned: a dead end, a pinch and the end of a
/// rim against a material seam all have to stay where they are.
fn smooth_rims<'a>(
    outlines: impl Iterator<Item = &'a Outline>,
    module: f32,
) -> BTreeMap<(i64, i64), Vec2> {
    let key = |p: Vec2| -> (i64, i64) {
        (
            (p.x * RIM_SNAP).round() as i64,
            (p.y * RIM_SNAP).round() as i64,
        )
    };
    let mut pos: BTreeMap<(i64, i64), Vec2> = BTreeMap::new();
    let mut adj: BTreeMap<(i64, i64), Vec<Vec2>> = BTreeMap::new();
    let mut pinned: BTreeMap<(i64, i64), bool> = BTreeMap::new();
    let mut mark = |p: Vec2, on_curve: bool, pinned: &mut BTreeMap<(i64, i64), bool>| {
        pinned.insert(
            (
                (p.x * RIM_SNAP).round() as i64,
                (p.y * RIM_SNAP).round() as i64,
            ),
            on_curve,
        );
    };
    for outline in outlines {
        let n = outline.nodes.len();
        for k in 0..n {
            let wall = outline.wall.get(k).copied().unwrap_or(false);
            let seam = outline.seam.get(k).copied().unwrap_or(false);
            if !wall && !seam {
                continue;
            }
            let a = outline.nodes[k];
            let b = outline.nodes[(k + 1) % n];
            if (b - a).length_squared() <= WELD * WELD {
                continue;
            }
            let (ka, kb) = (key(a), key(b));
            pos.insert(ka, a);
            pos.insert(kb, b);
            // A node on a road's curve stays put: it is already where it
            // should be, and relaxing it pushes it off.
            let sa = outline.snapped.get(k).copied().unwrap_or(false);
            let sb = outline.snapped.get((k + 1) % n).copied().unwrap_or(false);
            if sa {
                mark(a, true, &mut pinned);
            }
            if sb {
                mark(b, true, &mut pinned);
            }
            adj.entry(ka).or_default().push(b);
            adj.entry(kb).or_default().push(a);
        }
    }
    // The budget is on the *total* displacement, not per pass: relaxing eight
    // times with a per-pass cap lets a vertex migrate eight caps away and fold
    // the outline back over itself.
    let limit = module * 0.45;
    let orig = pos.clone();
    for _ in 0..RIM_RELAX_PASSES {
        let mut next = pos.clone();
        for (k, neighbours) in &adj {
            if neighbours.len() != 2 || pinned.get(k).copied().unwrap_or(false) {
                continue;
            }
            let here = pos[k];
            let mid = (neighbours[0] + neighbours[1]) * 0.5;
            let mut moved = here + (mid - here) * RIM_RELAX;
            // Never let the smoothing drag a vertex far from where the lattice
            // put it: the fan from the cell centre has to stay valid.
            let d = moved - orig[k];
            if d.length() > limit {
                moved = orig[k] + d.normalize() * limit;
            }
            next.insert(*k, moved);
        }
        pos = next;
    }
    // Return the *offset* per key so the emit pass can add it to whatever the
    // node ended up at.
    let mut offset = BTreeMap::new();
    for (k, p) in pos {
        let raw = Vec2::new(k.0 as f32, k.1 as f32) / RIM_SNAP;
        offset.insert(k, p - raw);
    }
    offset
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

    // ── Pass 1: every cell's outline. ────────────────────────────────────
    // The rim is smoothed as **one polyline per group** before anything is
    // emitted (see `smooth_rims`), which is the only way a diagonal rim stops
    // being a row of per-cell fish scales: a cell cannot see the cells its rim
    // continues into.
    struct Built {
        gid: u16,
        center: Vec2,
        outline: Outline,
        fillets: Vec<Vec<Vec2>>,
    }
    let mut built: Vec<Built> = Vec::with_capacity(paving.cells.len());
    for (&(i, j), &gid) in &paving.cells {
        // Corridor cells are a ribbon, not a staircase of chamfered squares.
        // Area cells (plaza, street rect, apron) stay on the lattice.
        if !paving.area_cells.contains(&(i, j)) && paving.cell_runs.contains_key(&(i, j)) {
            continue;
        }
        let group = &paving.groups[gid as usize];
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
        let rim_dir = |p: Vec2| paving.rim_dir(gid, p);
        // An area keeps the outline it was authored with. Snapping it onto a
        // road that shares the material pinches a 7 m street down to the
        // artery and saws the rim.
        let runs = if paving.area_cells.contains(&(i, j)) {
            Vec::new()
        } else {
            paving.cell_runs.get(&(i, j)).cloned().unwrap_or_default()
        };
        let snap = move |p: Vec2| paving.corridor_snap(gid, &runs, p);
        let is_boundary = |p: Vec2| paving.is_boundary(p);
        let (outline, fillets) = cell_outline(
            paving.cell_center(i, j),
            half,
            edge,
            diag,
            group.corner,
            group.inner_corner,
            &rim_dir,
            &snap,
            &is_boundary,
        );
        built.push(Built {
            gid,
            center: paving.cell_center(i, j),
            outline,
            fillets,
        });
    }
    let smooth = smooth_rims(built.iter().map(|b| &b.outline), module);

    // ── Pass 2: emit. ────────────────────────────────────────────────────
    let mut pieces: Vec<((u16, i32, i32), ChunkMeshData)> = Vec::new();
    for cell in &built {
        let gid = cell.gid;
        let group = &paving.groups[gid as usize];
        let scale = if group.texture_scale > 0.0 {
            group.texture_scale
        } else {
            1.0
        };
        let mut piece = ChunkMeshData::default();
        emit_cell(&mut piece, terrain, &cell.outline, &smooth, group, scale);
        for patch in &cell.fillets {
            emit_fillet(&mut piece, terrain, patch, group, scale);
        }
        pieces.push((bucket(gid, cell.center), piece));
    }

    emit_corridors(terrain, paving, &mut pieces);

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

/// One cross-section of a road ribbon: the centreline sample and the two kerb
/// points at `half_width` (mitered, so the paved width stays constant).
#[derive(Clone, Copy)]
struct Section {
    center: Vec2,
    left: Vec2,
    right: Vec2,
}

fn lerp_section(a: &Section, b: &Section, t: f32) -> Section {
    Section {
        center: a.center.lerp(b.center, t),
        left: a.left.lerp(b.left, t),
        right: a.right.lerp(b.right, t),
    }
}

/// Mitered kerb samples along a corridor. Stations are already ~1 m apart.
fn ribbon_sections(c: &PavingCorridor) -> Vec<Section> {
    let n = c.stations.len();
    if n < 2 {
        return Vec::new();
    }
    let mut seg_dir = Vec::with_capacity(n - 1);
    for w in c.stations.windows(2) {
        seg_dir.push((w[1] - w[0]).normalize_or_zero());
    }
    let perp = |d: Vec2| Vec2::new(-d.y, d.x);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let half = c.half_width.get(i).copied().unwrap_or(1.0).max(0.05);
        let in_d = if i > 0 { seg_dir[i - 1] } else { Vec2::ZERO };
        let out_d = if i + 1 < n { seg_dir[i] } else { Vec2::ZERO };
        let in_n = if in_d == Vec2::ZERO {
            Vec2::ZERO
        } else {
            perp(in_d)
        };
        let out_n = if out_d == Vec2::ZERO {
            Vec2::ZERO
        } else {
            perp(out_d)
        };
        let (normal, seg_n) = if in_n == Vec2::ZERO {
            (out_n, out_n)
        } else if out_n == Vec2::ZERO {
            (in_n, in_n)
        } else {
            let bis = in_n + out_n;
            if bis.length_squared() > 1e-8 {
                (bis.normalize(), in_n)
            } else {
                (in_n, in_n)
            }
        };
        if normal == Vec2::ZERO {
            continue;
        }
        let cos_half = normal.dot(seg_n).abs().max(0.25);
        let miter = (1.0 / cos_half).min(3.0);
        let off = normal * (half * miter);
        out.push(Section {
            center: c.stations[i],
            left: c.stations[i] - off,
            right: c.stations[i] + off,
        });
    }
    out
}

/// Distance from `p` to a corridor and the half-width there.
fn corridor_query(c: &PavingCorridor, p: Vec2) -> (f32, f32) {
    let mut best_d2 = f32::INFINITY;
    let mut best_half = c.half_width.first().copied().unwrap_or(1.0);
    let open = c.stations.first() != c.stations.last();
    let mut beyond_tip = false;
    for k in 0..c.stations.len().saturating_sub(1) {
        let a = c.stations[k];
        let b = c.stations[k + 1];
        let reach = c.half_width[k].max(c.half_width.get(k + 1).copied().unwrap_or(0.0)) + 0.75;
        if p.x < a.x.min(b.x) - reach || p.x > a.x.max(b.x) + reach {
            continue;
        }
        if p.y < a.y.min(b.y) - reach || p.y > a.y.max(b.y) + reach {
            continue;
        }
        let d = b - a;
        let len2 = d.length_squared();
        if len2 <= 1e-9 {
            continue;
        }
        let along = (p - a).dot(d) / len2;
        let t = along.clamp(0.0, 1.0);
        let dist2 = (p - (a + d * t)).length_squared();
        if dist2 < best_d2 {
            beyond_tip =
                open && ((k == 0 && along < 0.0) || (k + 2 == c.stations.len() && along > 1.0));
            best_d2 = dist2;
            let hb = c.half_width.get(k + 1).copied().unwrap_or(c.half_width[k]);
            best_half = c.half_width[k] + (hb - c.half_width[k]) * t;
        }
    }
    (
        if beyond_tip {
            f32::INFINITY
        } else {
            best_d2.sqrt()
        },
        best_half.max(0.0),
    )
}

/// The corridor that owns `p`: wider wins, later declaration breaks a tie.
/// `None` when `p` sits in none of this group's corridors.
fn winning_run(paving: &PavingGrid, gid: u16, p: Vec2) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (ri, c) in paving.corridors.iter().enumerate() {
        if c.group != gid {
            continue;
        }
        let (dist, half) = corridor_query(c, p);
        if dist > half + 1.0e-3 {
            continue;
        }
        let take = match best {
            None => true,
            Some((bh, bi)) => half > bh + 1.0e-3 || ((half - bh).abs() <= 1.0e-3 && ri > bi),
        };
        if take {
            best = Some((half, ri));
        }
    }
    best.map(|(_, ri)| ri)
}

fn ribbon_owns(paving: &PavingGrid, run: usize, gid: u16, p: Vec2) -> bool {
    let (i, j) = paving.cell_index(p);
    if paving.cells.get(&(i, j)) != Some(&gid) || paving.area_cells.contains(&(i, j)) {
        return false;
    }
    winning_run(paving, gid, p) == Some(run)
}

/// A kerb sample that would land on a plaza, on another material, or inside a
/// road that wins the junction.
fn edge_blocked(paving: &PavingGrid, run: usize, gid: u16, p: Vec2) -> bool {
    let (i, j) = paving.cell_index(p);
    if paving.area_cells.contains(&(i, j)) {
        return true;
    }
    match paving.cells.get(&(i, j)) {
        Some(&g) if g != gid => return true,
        Some(_) => {}
        // Unpaved ground: the kerb and its feather belong here.
        None => return false,
    }
    let contested = paving
        .cell_runs
        .get(&(i, j))
        .is_some_and(|rs| rs.iter().any(|&r| r != run));
    contested && winning_run(paving, gid, p).is_some_and(|w| w != run)
}

fn pull_free(origin: Vec2, edge: Vec2, blocked: impl Fn(Vec2) -> bool) -> Vec2 {
    if !blocked(edge) {
        return edge;
    }
    if blocked(origin) {
        return origin;
    }
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..16 {
        let mid = (lo + hi) * 0.5;
        if blocked(origin.lerp(edge, mid)) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    origin.lerp(edge, lo)
}

fn clip_section(from: &Section, to: &Section, owns: &impl Fn(Vec2) -> bool) -> Section {
    let from_owned = owns(from.center);
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..16 {
        let mid = (lo + hi) * 0.5;
        if owns(from.center.lerp(to.center, mid)) == from_owned {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = if from_owned { lo } else { hi };
    lerp_section(from, to, t)
}

fn feather_width(group: &PavingGroup, p: Vec2) -> f32 {
    let breathe = rim_wobble(group.seed ^ 0x5bf0_3635, p.x, p.y);
    (group.feather * (1.0 + group.noise * breathe))
        .clamp(group.feather * 0.55, group.feather * 1.45)
}

fn chunk_key(group: &PavingGroup, gid: u16, at: Vec2) -> (u16, i32, i32) {
    let chunk = group.chunk.max(0.0);
    if chunk > 0.0 {
        (
            gid,
            (at.x / chunk).floor() as i32,
            (at.y / chunk).floor() as i32,
        )
    } else {
        (gid, 0, 0)
    }
}

fn emit_side_feather(
    mesh: &mut ChunkMeshData,
    terrain: &BrushGrid,
    group: &PavingGroup,
    scale: f32,
    a: Vec2,
    b: Vec2,
    out_a: Vec2,
    out_b: Vec2,
    mut prev: [u32; 2],
) {
    if group.feather <= MIN_RADIUS || out_a == Vec2::ZERO || out_b == Vec2::ZERO {
        return;
    }
    let wa = feather_width(group, a);
    let wb = feather_width(group, b);
    for ring in 1..=FEATHER_RINGS {
        let t = ring as f32 / FEATHER_RINGS as f32;
        let alpha = 1.0 - smootherstep01(t);
        let pa = a + out_a * (wa * t);
        let pb = b + out_b * (wb * t);
        let cur = [
            push_vertex(mesh, terrain, pa.x, pa.y, group.lift, scale, 1.0, alpha),
            push_vertex(mesh, terrain, pb.x, pb.y, group.lift, scale, 1.0, alpha),
        ];
        strip(mesh, prev, cur);
        prev = cur;
    }
}

fn emit_ribbon_quad(
    mesh: &mut ChunkMeshData,
    terrain: &BrushGrid,
    group: &PavingGroup,
    scale: f32,
    a: Section,
    b: Section,
    feather_l: bool,
    feather_r: bool,
) {
    let lift = group.lift;
    let l0 = push_vertex(mesh, terrain, a.left.x, a.left.y, lift, scale, 1.0, 1.0);
    let r0 = push_vertex(mesh, terrain, a.right.x, a.right.y, lift, scale, 1.0, 1.0);
    let l1 = push_vertex(mesh, terrain, b.left.x, b.left.y, lift, scale, 1.0, 1.0);
    let r1 = push_vertex(mesh, terrain, b.right.x, b.right.y, lift, scale, 1.0, 1.0);
    strip(mesh, [l0, r0], [l1, r1]);
    if feather_l {
        emit_side_feather(
            mesh,
            terrain,
            group,
            scale,
            a.left,
            b.left,
            (a.left - a.center).normalize_or_zero(),
            (b.left - b.center).normalize_or_zero(),
            [l0, l1],
        );
    }
    if feather_r {
        emit_side_feather(
            mesh,
            terrain,
            group,
            scale,
            a.right,
            b.right,
            (a.right - a.center).normalize_or_zero(),
            (b.right - b.center).normalize_or_zero(),
            [r0, r1],
        );
    }
}

/// Roads are ribbons. The lattice still decides who owns the ground — a plaza
/// or a winning junction keeps the cell, and the ribbon stops on that edge —
/// but the paved surface follows the centreline at the authored width instead
/// of the staircase the raster leaves behind.
fn emit_corridors(
    terrain: &BrushGrid,
    paving: &PavingGrid,
    pieces: &mut Vec<((u16, i32, i32), ChunkMeshData)>,
) {
    for (ri, corridor) in paving.corridors.iter().enumerate() {
        let secs = ribbon_sections(corridor);
        if secs.len() < 2 {
            continue;
        }
        let gid = corridor.group;
        let group = &paving.groups[gid as usize];
        let scale = if group.texture_scale > 0.0 {
            group.texture_scale
        } else {
            1.0
        };
        let owns = |p: Vec2| ribbon_owns(paving, ri, gid, p);
        let mut i = 0;
        while i < secs.len() {
            if !owns(secs[i].center) {
                i += 1;
                continue;
            }
            let mut span: Vec<Section> = Vec::new();
            if i > 0 && !owns(secs[i - 1].center) {
                span.push(clip_section(&secs[i - 1], &secs[i], &owns));
            }
            while i < secs.len() && owns(secs[i].center) {
                span.push(secs[i]);
                i += 1;
            }
            if i < secs.len()
                && let Some(prev) = span.last().copied()
            {
                span.push(clip_section(&prev, &secs[i], &owns));
            }
            for sec in &mut span {
                sec.left = pull_free(sec.center, sec.left, |p| edge_blocked(paving, ri, gid, p));
                sec.right = pull_free(sec.center, sec.right, |p| edge_blocked(paving, ri, gid, p));
            }
            for w in span.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a.center.distance(b.center) < 0.01 {
                    continue;
                }
                if a.left.distance(a.right) < 0.02 && b.left.distance(b.right) < 0.02 {
                    continue;
                }
                let probe = 0.05;
                let out_l = (a.left - a.center).normalize_or_zero();
                let out_r = (a.right - a.center).normalize_or_zero();
                let feather_l =
                    out_l != Vec2::ZERO && !edge_blocked(paving, ri, gid, a.left + out_l * probe);
                let feather_r =
                    out_r != Vec2::ZERO && !edge_blocked(paving, ri, gid, a.right + out_r * probe);
                let mut mesh = ChunkMeshData::default();
                emit_ribbon_quad(&mut mesh, terrain, group, scale, a, b, feather_l, feather_r);
                let at = (a.center + b.center) * 0.5;
                pieces.push((chunk_key(group, gid, at), mesh));
            }
        }
    }
}

/// Emits the cell polygon (fan from the cell centre) and its feather band.
#[allow(clippy::too_many_arguments)]
fn emit_cell(
    mesh: &mut ChunkMeshData,
    grid: &BrushGrid,
    outline: &Outline,
    smooth: &BTreeMap<(i64, i64), Vec2>,
    group: &PavingGroup,
    scale: f32,
) {
    if outline.nodes.len() < 3 {
        return;
    }
    let lift = group.lift;

    // The opaque kerb is the relaxed lattice (and, for a corridor that still
    // goes through here, the snapped curve). It does not wobble: `noise`
    // only breathes the feather width, so two cells that share a vertex stay
    // on the same point.
    let n = outline.nodes.len();
    let mut base: Vec<Vec2> = Vec::with_capacity(n);
    for p in &outline.nodes {
        let key = (
            (p.x * RIM_SNAP).round() as i64,
            (p.y * RIM_SNAP).round() as i64,
        );
        let relax = smooth.get(&key).copied().unwrap_or(Vec2::ZERO);
        // An explicit `paving-wobble` still slides a free rim. Auto is zero.
        let slide =
            outline.move_dir[base.len()] * (group.wobble * rim_wobble(group.seed, p.x, p.y));
        base.push(*p + relax + slide);
    }
    let nodes = base.clone();

    let reverse = signed_area(&nodes) > 0.0;
    // The hub is the centroid of the outline, not the raw cell centre: a cell
    // whose rim was snapped onto a road's curve can leave its own centre
    // outside the polygon, and a fan from outside inverts.
    let hub_at = nodes.iter().copied().fold(Vec2::ZERO, |a, p| a + p) / nodes.len() as f32;
    let hub = push_vertex(mesh, grid, hub_at.x, hub_at.y, lift, scale, 1.0, 1.0);
    let mut indices = Vec::with_capacity(nodes.len());
    for p in &nodes {
        indices.push(push_vertex(mesh, grid, p.x, p.y, lift, scale, 1.0, 1.0));
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

    let feather = group.feather;
    if feather <= MIN_RADIUS {
        return;
    }
    // The band's offset is per **node**, not per segment: the direction is the
    // mitred rim normal (so the strip keeps its width across a corner instead
    // of opening a wedge). `noise` breathes the width; the kerb (t = 0) stays
    // put. The width is also capped by the local curvature radius, so a tight
    // corner gets a narrow band instead of an inverted one.
    let radius = curvature_radius(&base, &outline.move_dir, &outline.wall, &outline.seam);
    let widths: Vec<f32> = (0..n)
        .map(|k| {
            let breathe = rim_wobble(group.seed ^ 0x5bf0_3635, base[k].x, base[k].y);
            (group.feather * (1.0 + group.noise * breathe))
                .clamp(group.feather * 0.55, group.feather * 1.45)
                .min(radius[k] * 0.8)
        })
        .collect();
    let band_at = |k: usize, t: f32| -> Vec2 { base[k] + outline.band[k] * (widths[k] * t) };
    // Bands exist **only** where the pavement meets unpaved ground. Across a
    // material boundary there is nothing: the two surfaces meet on the lattice
    // line, and that line is the kerb. The alternative — the later material
    // fading a translucent copy of itself a millimetre above the other — is
    // what produced the grey wedges across the dirt streets and the flicker:
    // two surfaces 1.5 mm apart read as one z-fighting plane at the far plane,
    // and the band's geometry followed the *street's* cell edges, so the wedge
    // ran at the street's rotation instead of the road's.
    for k in 0..n {
        if !outline.wall.get(k).copied().unwrap_or(false) {
            continue;
        }
        let kb = (k + 1) % n;
        let mut prev = [indices[k], indices[kb]];
        for ring in 1..=FEATHER_RINGS {
            let t = ring as f32 / FEATHER_RINGS as f32;
            let alpha = 1.0 - smootherstep01(t);
            let a = band_at(k, t);
            let b = band_at(kb, t);
            let cur = [
                push_vertex(mesh, grid, a.x, a.y, lift, scale, 1.0, alpha),
                push_vertex(mesh, grid, b.x, b.y, lift, scale, 1.0, alpha),
            ];
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
        let pa = Vec2::new(mesh.positions[a as usize][0], mesh.positions[a as usize][2]);
        let pb = Vec2::new(mesh.positions[b as usize][0], mesh.positions[b as usize][2]);
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

    /// Test probe: the lattice never lets a node slide, so the outlines the
    /// geometry tests inspect are the plain lattice shapes.
    fn no_move(_: Vec2) -> Option<Vec2> {
        None
    }

    fn identity(p: Vec2) -> Vec2 {
        p
    }

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
                chunk: 0.0,
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
                chunk: 0.0,
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
                chunk: 0.0,
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
                &no_move,
                &identity,
                &|_| true,
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
            &no_move,
            &identity,
            &|_| true,
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
            &no_move,
            &identity,
            &|_| true,
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
        let rim: Vec<f32> = mesh.positions.iter().map(|p| p[1]).collect();
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
    fn corridor_snap_ignores_points_far_past_a_road_end() {
        // A point far out along a road's extension has a lateral offset of
        // about `half`; it must not be snapped onto the road's end cap (that
        // stretched paving cells into 300 m sheets across the city).
        let spec = PavingSpec {
            shapes: vec![PavingShape::Run {
                stations: (0..=20).map(|k| Vec2::new(0.0, k as f32)).collect(),
                half_width: vec![2.0; 21],
            }],
            ..PavingSpec::default()
        };
        let grid = PavingGrid::build(&[spec]);
        let runs: Vec<usize> = (0..grid.corridors.len()).collect();
        let far = Vec2::new(2.0, 200.0);
        assert_eq!(grid.corridor_snap(0, &runs, far), far);
        // A node on the rim beside the road still snaps to its edge.
        let near = Vec2::new(2.4, 10.0);
        let snapped = grid.corridor_snap(0, &runs, near);
        assert!(
            (snapped - Vec2::new(2.0, 10.0)).length() < 1e-4,
            "{snapped:?}"
        );
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
                chunk: 0.0,
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
            &no_move,
            &identity,
            &|_| true,
        );
        assert_eq!(fillets.len(), 1, "um canto côncavo, um filete");
        let segs = arc_segments(0.7, std::f32::consts::FRAC_PI_2);
        assert_eq!(fillets[0].len(), 1 + segs + 1);

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

    /// Two materials meet on the lattice line and **nothing is drawn over
    /// either of them**. The first version of this feathered the later
    /// material a millimetre above the other one's paving — a translucent copy
    /// that followed the *street's* cell edges, so a cobbled artery left grey
    /// wedges across a dirt street, and the 1.5 mm offset z-fought the street
    /// itself into flicker.
    #[test]
    fn a_material_boundary_is_a_clean_kerb() {
        let cobble = PavingSpec {
            base_color: [0.6, 0.6, 0.6],
            feather: 1.0,
            shapes: vec![PavingShape::Rect {
                at: Vec2::ZERO,
                size: Vec2::splat(20.0),
            }],
            ..PavingSpec::default()
        };
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
        let of = |colour: [f32; 3]| -> Vec<(f32, f32, f32)> {
            let gid = grid
                .groups()
                .iter()
                .position(|g| g.base_color == colour)
                .expect("grupo");
            meshes
                .iter()
                .filter(|(g, _)| *g == gid)
                .flat_map(|(_, m)| {
                    m.positions
                        .iter()
                        .zip(&m.colors)
                        .map(|(p, c)| (p[0], p[2], c[3]))
                })
                .collect()
        };
        // The dirt stops at z = 4 and nothing of it is drawn over the cobbles:
        // no translucent dirt vertex above the seam line.
        let dirt_over = of([0.4, 0.3, 0.2])
            .into_iter()
            .filter(|(x, z, a)| *z < 3.99 && x.abs() < 3.0 && *a > 0.001 && *a < 0.999)
            .count();
        assert_eq!(dirt_over, 0, "a terra não deve invadir a calçada");
        // And the cobbles do not fade into the dirt either.
        let cobble_over = of([0.6, 0.6, 0.6])
            .into_iter()
            .filter(|(x, z, a)| *z > 4.01 && x.abs() < 3.0 && *a > 0.001 && *a < 0.999)
            .count();
        assert_eq!(cobble_over, 0, "a calçada não deve esbater-se para a terra");
        // Both still reach the seam at full strength, so there is no terrain
        // bleed along the kerb.
        for colour in [[0.6, 0.6, 0.6], [0.4, 0.3, 0.2]] {
            let at_seam = of(colour)
                .into_iter()
                .filter(|(x, z, a)| (*z - 4.0).abs() < 0.05 && *a > 0.999)
                .count();
            assert!(at_seam > 0, "a costura tem de ser opaca dos dois lados");
        }
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
            &no_move,
            &identity,
            &|_| true,
        );
        // Four chamfer arcs of CORNER_SEGS + 1 points, welded at the edge
        // midpoints: the outline becomes a ring of 4 × 4 points.
        let segs = arc_segments(1.0, std::f32::consts::FRAC_PI_2);
        assert_eq!(outline.nodes.len(), 4 * (segs + 1) - 4);
        assert!(outline.wall.iter().all(|w| *w), "célula isolada só tem rim");
    }

    fn run_spec(stations: Vec<Vec2>, half: f32) -> PavingSpec {
        PavingSpec {
            feather: 0.0,
            shapes: vec![PavingShape::Run {
                half_width: vec![half; stations.len()],
                stations,
            }],
            style: PavingStyle {
                chunk: 0.0,
                wobble: 0.0,
                corner: 0.0,
                inner_corner: 0.0,
                ..PavingStyle::default()
            },
            ..PavingSpec::default()
        }
    }

    fn opaque_xz(mesh: &ChunkMeshData) -> Vec<Vec2> {
        mesh.positions
            .iter()
            .zip(&mesh.colors)
            .filter(|(_, c)| c[3] > 0.999)
            .map(|(p, _)| Vec2::new(p[0], p[2]))
            .collect()
    }

    #[test]
    fn diagonal_road_keeps_the_authored_width() {
        // 45° so a cell staircase would throw the kerb out by up to a module.
        // The ribbon stays on the offset curve.
        let stations: Vec<Vec2> = (0..=24)
            .map(|k| Vec2::new(20.0 + k as f32, 18.0 + k as f32))
            .collect();
        let grid = PavingGrid::build(&[run_spec(stations, 2.0)]);
        let meshes = paving_meshes(&test_grid(), &grid);
        assert_eq!(meshes.len(), 1);
        let pts = opaque_xz(&meshes[0].1);
        let dir = Vec2::new(1.0, 1.0).normalize();
        let normal = Vec2::new(-dir.y, dir.x);
        let origin = Vec2::new(20.0, 18.0);
        let mut kerb = 0;
        for p in pts {
            let along = (p - origin).dot(dir);
            if !(1.0..32.0).contains(&along) {
                continue;
            }
            let lat = (p - origin).dot(normal).abs();
            assert!(
                (lat - 2.0).abs() < 0.08,
                "lancil fora da meia-largura: {p:?} lat {lat}"
            );
            kerb += 1;
        }
        assert!(kerb > 20, "a fita não desenhou o lancil");
    }

    #[test]
    fn collinear_roads_own_their_side_of_the_joint() {
        let road = |start: i32| {
            run_spec(
                (start..=start + 5)
                    .map(|x| Vec2::new(x as f32, 20.0))
                    .collect(),
                2.0,
            )
        };
        let grid = PavingGrid::build(&[road(10), road(15)]);
        assert_eq!(winning_run(&grid, 0, Vec2::new(14.8, 20.7)), Some(0));
        assert_eq!(winning_run(&grid, 0, Vec2::new(15.2, 20.7)), Some(1));
    }

    #[test]
    fn a_crossing_does_not_draw_the_side_road_through_the_main() {
        let main = run_spec(
            (0..=20).map(|k| Vec2::new(40.0, 20.0 + k as f32)).collect(),
            3.0,
        );
        let side = run_spec(
            (0..=20).map(|k| Vec2::new(30.0 + k as f32, 30.0)).collect(),
            1.5,
        );
        let grid = PavingGrid::build(&[side, main]);
        let meshes = paving_meshes(&test_grid(), &grid);
        let pts: Vec<Vec2> = meshes.iter().flat_map(|(_, m)| opaque_xz(m)).collect();
        for p in pts {
            let inside_main = (p.x - 40.0).abs() < 2.6 && (p.y - 30.0).abs() < 8.0;
            let on_main_kerb =
                ((p.x - 40.0).abs() - 3.0).abs() < 0.25 && (20.0..=40.0).contains(&p.y);
            let on_side_kerb = ((p.y - 30.0).abs() - 1.5).abs() < 0.25
                && (p.x - 40.0).abs() >= 2.7
                && (30.0..=50.0).contains(&p.x);
            assert!(
                !inside_main || on_main_kerb || on_side_kerb,
                "a secundária entrou na principal: {p:?}"
            );
            assert!(
                on_main_kerb
                    || on_side_kerb
                    || (p.y - 20.0).abs() < 0.4
                    || (p.y - 40.0).abs() < 0.4
                    || (p.x - 30.0).abs() < 0.4
                    || (p.x - 50.0).abs() < 0.4,
                "vértice fora das duas fitas: {p:?}"
            );
        }
    }

    #[test]
    fn a_street_rect_is_not_sucked_onto_the_artery() {
        // The old snap pulled every rim within 3.2 m of a same-material road
        // onto that road. A 12 m street crossing a 4 m artery collapsed.
        let road = run_spec(
            (0..=30).map(|k| Vec2::new(40.0, 15.0 + k as f32)).collect(),
            2.0,
        );
        let mut street = rect_spec(Vec2::new(40.0, 30.0), Vec2::new(12.0, 14.0), 0);
        street.feather = 0.0;
        street.style.wobble = 0.0;
        street.style.corner = 0.0;
        street.style.inner_corner = 0.0;
        street.style.chunk = 0.0;
        let grid = PavingGrid::build(&[street, road]);
        assert!(
            grid.area_cells.contains(&(17, 14)) || grid.area_cells.len() > 10,
            "o rectângulo tem de continuar a ser área"
        );
        let meshes = paving_meshes(&test_grid(), &grid);
        let pts: Vec<Vec2> = meshes.iter().flat_map(|(_, m)| opaque_xz(m)).collect();
        let street_edge = pts
            .iter()
            .any(|p| (p.x - 34.0).abs() < 0.15 && (p.y - 30.0).abs() < 6.0);
        assert!(street_edge, "a borda da rua a x=34 desapareceu");
        let pinched = pts.iter().any(|p| {
            (p.y - 30.0).abs() < 4.0 && (p.x - 40.0).abs() > 6.3 && (p.x - 40.0).abs() < 9.0
        });
        assert!(!pinched, "a rua foi puxada para fora do rectângulo");
        let ribbon = pts
            .iter()
            .any(|p| (p.y - 16.5).abs() < 0.6 && (p.x - 38.0).abs() < 0.15);
        assert!(ribbon, "a fita fora da rua não ficou na meia-largura");
    }

    #[test]
    fn road_ribbon_drapes_and_faces_up() {
        let mut spec = run_spec(
            (0..=16)
                .map(|k| Vec2::new(24.0 + k as f32 * 0.6, 30.0 + k as f32))
                .collect(),
            2.2,
        );
        spec.feather = 0.8;
        spec.noise = 0.1;
        let terrain = test_grid();
        let grid = PavingGrid::build(&[spec]);
        let meshes = paving_meshes(&terrain, &grid);
        assert!(!meshes.is_empty());
        let mut saw_clear = false;
        for (_, mesh) in &meshes {
            for p in &mesh.positions {
                let want = terrain.sample(p[0], p[2]) + PAVING_LIFT;
                assert!(
                    (p[1] - want).abs() < 1.0e-3,
                    "drape {}",
                    (p[1] - want).abs()
                );
            }
            for tri in mesh.indices.as_chunks::<3>().0 {
                let p = |i: u32| {
                    let v = mesh.positions[i as usize];
                    Vec2::new(v[0], v[2])
                };
                let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
                let area = up_normal(a, b, c);
                if area.abs() < 1e-8 {
                    continue;
                }
                assert!(area > 0.0, "triângulo invertido {a:?} {b:?} {c:?}");
            }
            if mesh.colors.iter().any(|c| c[3] < 1.0e-3) {
                saw_clear = true;
            }
        }
        assert!(saw_clear, "a feather da estrada tem de chegar a alpha 0");
    }
}
