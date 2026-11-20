//! Proprioception & environmental sensing: what the puppet "feels" and "sees"
//! every physics step.
//!
//! Combines internal whole-body dynamics — centre of mass, velocity, vestibular
//! acceleration, **centroidal angular momentum** (Macchietto et al. 2009 /
//! Geijtenbeek et al. 2012), contact stance state, real impulse-weighted
//! **Center of Pressure (CoP)**, support polygon, constant-leg-length IPM
//! **capture point** & **flywheel capture region** (Pratt et al. 2006 / Coros
//! et al. 2010) — with external Avian contact manifolds (normals, points,
//! impact impulses & speeds) and `SpatialQuery` probes (true ground clearance
//! & slope normal, step-ahead terrain elevation, and nearby walls/obstacles for
//! [`crate::behaviour::WallBrace`]).

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::part::{PART_COUNT, PuppetPart, Side};
use crate::rig::{PuppetBody, PuppetConfig, PuppetRig};
use crate::state::{PuppetMode, PuppetState};

/// Which feet are currently supporting the body (Geijtenbeek et al. 2012, eq. 8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StanceState {
    #[default]
    Double,
    Left,
    Right,
    Flight,
}

/// A world surface detected by spatial probing (ground ahead, drop-off, wall, obstacle).
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceProbe {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    pub entity: Option<Entity>,
}

/// Kinematic state of one body.
#[derive(Clone, Copy, Debug)]
pub struct BodyState {
    pub pos: Vec3,
    pub rot: Quat,
    pub lin_vel: Vec3,
    pub ang_vel: Vec3,
    /// World centre of mass.
    pub com: Vec3,
    pub mass: f32,
    /// World inertia tensor about the centre of mass.
    pub inertia: Mat3,
}

impl Default for BodyState {
    fn default() -> Self {
        Self {
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            lin_vel: Vec3::ZERO,
            ang_vel: Vec3::ZERO,
            com: Vec3::ZERO,
            mass: 0.0,
            inertia: Mat3::ZERO,
        }
    }
}

/// What the puppet senses this step.
#[derive(Component, Clone, Debug)]
pub struct PuppetSense {
    pub bodies: [BodyState; PART_COUNT],
    /// Whether each part touches a solid (non-sensor) surface other than its own puppet.
    pub contacts: [bool; PART_COUNT],
    /// Outward contact normal pointing from the world surface into each contacting part.
    pub contact_normals: [Vec3; PART_COUNT],
    /// Impulse-weighted world contact point for each contacting part.
    pub contact_points: [Vec3; PART_COUNT],
    /// Total normal impulse (N·s) received by each part this physics step.
    pub contact_impulses: [f32; PART_COUNT],
    /// Peak approach speed (m/s, >= 0) along the contact normal for each part this step.
    pub contact_speeds: [f32; PART_COUNT],
    /// Hardest-impacting part this step (filtered to actual impacts with approach speed > 0.5 m/s).
    pub peak_impact_part: Option<PuppetPart>,
    /// Normal impulse (N·s) of `peak_impact_part`.
    pub peak_impact_impulse: f32,
    /// Approach speed (m/s) of `peak_impact_part`.
    pub peak_impact_speed: f32,
    pub com: Vec3,
    pub com_vel: Vec3,
    /// Smoothed CoM linear acceleration (m/s², vestibular otolith signal).
    pub com_accel: Vec3,
    /// Smoothed Head linear acceleration (m/s², vestibular jolt).
    pub head_accel: Vec3,
    /// Centroidal angular momentum `L = Σ [m_i (c_i - c) × (v_i - v) + I_i ω_i]`.
    pub angular_momentum: Vec3,
    /// Composite inertia tensor of the whole body about `com`.
    pub com_inertia: Mat3,
    /// Effective whole-body angular velocity `I_com⁻¹ L` (rad/s).
    pub centroidal_ang_vel: Vec3,
    pub total_mass: f32,
    /// Unit up (against gravity) and gravity magnitude.
    pub up: Vec3,
    pub g: f32,
    /// Ground height under the body (lowest contacting sole, or true raycast surface when airborne).
    pub ground: f32,
    /// Ground surface normal beneath the puppet (`up` on flat ground).
    pub ground_normal: Vec3,
    /// Ground normal beneath each foot `[left, right]` for slope-conforming planting.
    pub foot_normals: [Vec3; 2],
    /// True vertical clearance (m) from the lowest foot sole to the raycast surface below.
    pub ground_clearance: f32,
    /// Terrain surface probed ahead at the predicted capture/step landing location.
    pub ahead_ground: Option<SurfaceProbe>,
    /// Nearby steep wall or obstacle ahead in velocity/fall direction (for [`crate::behaviour::WallBrace`]).
    pub nearby_wall: Option<SurfaceProbe>,
    /// Real Center of Pressure (horizontal) weighted by foot normal impulses.
    pub cop: Vec2,
    /// Current foot contact configuration.
    pub stance: StanceState,
    /// Angle between each foot's sole normal and the local ground normal (rad), `[left, right]`.
    pub foot_tilts: [f32; 2],
    /// Angular speed of each foot (rad/s), `[left, right]`.
    pub foot_ang_speeds: [f32; 2],
    /// Support polygon (convex, counter-clockwise) on the ground plane, in
    /// horizontal coordinates (world X/Z for Y-up gravity). Empty when
    /// no foot touches anything.
    pub support: Vec<Vec2>,
    /// Horizontal centre of the support polygon (or between the feet).
    pub support_centre: Vec2,
    /// Capture point (horizontal), computed from the constant-leg-length IPM
    /// (Pratt & Tedrake 2006 / Coros et al. 2010 §3.2).
    pub capture_point: Vec2,
    /// Signed distance of the capture point outside the support polygon
    /// (negative inside; +inf with no support).
    pub capture_margin: f32,
    /// Extra capture-region radius achievable via angular-momentum (hip/arm
    /// flywheel) strategy without stepping (Pratt et al. 2006).
    pub capture_region_radius: f32,
    /// Angle between the trunk and the vertical (rad).
    pub trunk_tilt: f32,
    /// Pelvis height above `ground`.
    pub pelvis_height: f32,
    /// Horizontal forward of the pelvis.
    pub heading: Vec3,
    /// Chest facing dot `up`: `+1` = supine (face-up), `-1` = prone (face-down), `0` = upright/side.
    pub face_up_dot: f32,
    /// Continuous seconds spent completely airborne (`feet_down == 0 && !body_touching`).
    pub free_fall_time: f32,
    /// Exact ballistic time-to-impact (s) of the torso onto `ground` (`+inf` in zero-g).
    pub predicted_impact_time: f32,
    /// Time step of the sense.
    pub dt: f32,
    pub time: f32,
    /// Number of physics sensing ticks completed for this rig.
    pub steps: u32,
    #[doc(hidden)]
    pub prev_com_vel: Vec3,
    #[doc(hidden)]
    pub prev_head_vel: Vec3,
}

impl Default for PuppetSense {
    fn default() -> Self {
        Self {
            bodies: [BodyState::default(); PART_COUNT],
            contacts: [false; PART_COUNT],
            contact_normals: [Vec3::ZERO; PART_COUNT],
            contact_points: [Vec3::ZERO; PART_COUNT],
            contact_impulses: [0.0; PART_COUNT],
            contact_speeds: [0.0; PART_COUNT],
            peak_impact_part: None,
            peak_impact_impulse: 0.0,
            peak_impact_speed: 0.0,
            com: Vec3::ZERO,
            com_vel: Vec3::ZERO,
            com_accel: Vec3::ZERO,
            head_accel: Vec3::ZERO,
            angular_momentum: Vec3::ZERO,
            com_inertia: Mat3::IDENTITY,
            centroidal_ang_vel: Vec3::ZERO,
            total_mass: 0.0,
            up: Vec3::Y,
            g: 9.81,
            ground: 0.0,
            ground_normal: Vec3::Y,
            foot_normals: [Vec3::Y; 2],
            ground_clearance: 0.0,
            ahead_ground: None,
            nearby_wall: None,
            cop: Vec2::ZERO,
            stance: StanceState::Flight,
            foot_tilts: [0.0; 2],
            foot_ang_speeds: [0.0; 2],
            support: Vec::new(),
            support_centre: Vec2::ZERO,
            capture_point: Vec2::ZERO,
            capture_margin: f32::INFINITY,
            capture_region_radius: 0.0,
            trunk_tilt: 0.0,
            pelvis_height: 0.0,
            heading: Vec3::NEG_Z,
            face_up_dot: 0.0,
            free_fall_time: 0.0,
            predicted_impact_time: f32::INFINITY,
            dt: 1.0 / 60.0,
            time: 0.0,
            steps: 0,
            prev_com_vel: Vec3::ZERO,
            prev_head_vel: Vec3::ZERO,
        }
    }
}

impl PuppetSense {
    #[inline]
    pub fn body(&self, part: PuppetPart) -> &BodyState {
        &self.bodies[part.index()]
    }

    #[inline]
    pub fn touching(&self, part: PuppetPart) -> bool {
        self.contacts[part.index()]
    }

    #[inline]
    pub fn contact_normal(&self, part: PuppetPart) -> Vec3 {
        self.contact_normals[part.index()]
    }

    #[inline]
    pub fn contact_impulse(&self, part: PuppetPart) -> f32 {
        self.contact_impulses[part.index()]
    }

    #[inline]
    pub fn contact_speed(&self, part: PuppetPart) -> f32 {
        self.contact_speeds[part.index()]
    }

    pub fn foot_down(&self, side: Side) -> bool {
        self.touching(PuppetPart::foot(side))
    }

    /// Number of feet on something.
    pub fn feet_down(&self) -> usize {
        [Side::Left, Side::Right].iter().filter(|s| self.foot_down(**s)).count()
    }

    /// Whether the puppet is in sustained airborne free-fall well above the ground.
    pub fn is_high_fall(&self, rig_height: f32) -> bool {
        if self.g <= 1e-3 || self.feet_down() > 0 || self.body_touching() {
            return false;
        }
        let falling_down = self.com_vel.dot(self.up) < -0.6;
        self.ground_clearance > 0.85 * rig_height
            || (falling_down
                && self.free_fall_time > 0.18
                && self.predicted_impact_time.is_finite()
                && self.predicted_impact_time > 0.42)
    }

    /// How well planted `side` is in `[0, 1]`: 1 when flat and still on the
    /// ground, smoothly dropping to 0 as the foot rolls or spins (Coros et al.
    /// 2010 §3.3 foot-tipping guard for ankle virtual torques).
    pub fn foot_plant_quality(&self, side: Side) -> f32 {
        if !self.foot_down(side) {
            return 0.0;
        }
        let i = side.index();
        let tilt_q = 1.0 - ((self.foot_tilts[i] - 0.12) / 0.28).clamp(0.0, 1.0);
        let spin_q = 1.0 - ((self.foot_ang_speeds[i] - 1.5) / 3.5).clamp(0.0, 1.0);
        tilt_q * spin_q
    }

    /// Axes of the plane perpendicular to gravity. For Y-up they are X and Z.
    fn ground_axes(&self) -> (Vec3, Vec3) {
        if self.up == Vec3::Y {
            return (Vec3::X, Vec3::Z);
        }
        let z = if self.up.x.abs() > 0.9 {
            Vec3::Z.reject_from(self.up).normalize()
        } else {
            Vec3::X.cross(self.up).normalize()
        };
        (self.up.cross(z), z)
    }

    /// Horizontal coordinates of a world point.
    #[inline]
    pub fn flat(&self, p: Vec3) -> Vec2 {
        let (x, z) = self.ground_axes();
        Vec2::new(p.dot(x), p.dot(z))
    }

    /// World point at ground height from horizontal coordinates.
    #[inline]
    pub fn lift(&self, p: Vec2, height: f32) -> Vec3 {
        let (x, z) = self.ground_axes();
        x * p.x + z * p.y + self.up * height
    }

    /// Speed of the centre of mass in the horizontal plane.
    pub fn horizontal_speed(&self) -> f32 {
        self.com_vel.reject_from(self.up).length()
    }

    /// Whether any part other than the feet (and the calves) touches something:
    /// the body is down or leaning on something.
    pub fn body_touching(&self) -> bool {
        PuppetPart::ALL.iter().any(|p| {
            self.contacts[p.index()]
                && !matches!(
                    p,
                    PuppetPart::FootL | PuppetPart::FootR | PuppetPart::CalfL | PuppetPart::CalfR
                )
        })
    }
}

/// Constant-leg-length inverted pendulum stopping distance (Pratt & Tedrake
/// 2006; Coros, Beaudoin & van de Panne, SIGGRAPH 2010 §3.2):
/// `d = v · sqrt(h / g + v² / (4 g²))`.
#[inline]
pub fn ipm_capture_offset(v_horiz: Vec2, com_height: f32, g: f32) -> Vec2 {
    let speed = v_horiz.length();
    if speed < 1e-6 {
        return Vec2::ZERO;
    }
    let h = com_height.max(0.05);
    let g = g.max(1e-3);
    let d = speed * (h / g + (speed * speed) / (4.0 * g * g)).sqrt();
    (v_horiz / speed) * d
}

#[inline]
fn outer(a: Vec3, b: Vec3) -> Mat3 {
    Mat3::from_cols(a * b.x, a * b.y, a * b.z)
}

/// Clamp a surface normal so its angle to `up` never exceeds `max_tilt` (rad).
#[inline]
pub fn clamp_normal_tilt(normal: Vec3, up: Vec3, max_tilt: f32) -> Vec3 {
    let n = normal.normalize_or(up);
    let angle = n.angle_between(up);
    if angle <= max_tilt || angle < 1e-5 {
        return n;
    }
    let axis = up.cross(n).normalize_or_zero();
    if axis.length_squared() < 0.5 {
        return up;
    }
    (Quat::from_axis_angle(axis, max_tilt) * up).normalize()
}

/// Convex hull (Andrew's monotone chain), counter-clockwise.
pub fn convex_hull(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| (*a - *b).length_squared() < 1e-10);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut lower: Vec<Vec2> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Vec2> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Signed distance from `p` to a convex CCW polygon (negative inside).
pub fn polygon_distance(poly: &[Vec2], p: Vec2) -> f32 {
    match poly.len() {
        0 => f32::INFINITY,
        1 => (p - poly[0]).length(),
        2 => segment_distance(poly[0], poly[1], p),
        n => {
            let mut inside = true;
            let mut best = f32::INFINITY;
            for i in 0..n {
                let a = poly[i];
                let b = poly[(i + 1) % n];
                if (b - a).perp_dot(p - a) < 0.0 {
                    inside = false;
                }
                best = best.min(segment_distance(a, b, p));
            }
            if inside { -best } else { best }
        }
    }
}

fn segment_distance(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-12 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t - p).length()
}

/// Closest point of a convex polygon to `p` (`p` itself if inside).
pub fn polygon_closest(poly: &[Vec2], p: Vec2) -> Vec2 {
    if poly.is_empty() {
        return p;
    }
    if poly.len() >= 3 && polygon_distance(poly, p) <= 0.0 {
        return p;
    }
    let n = poly.len();
    let mut best = poly[0];
    let mut best_d = f32::INFINITY;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let ab = b - a;
        let t = if ab.length_squared() > 1e-12 {
            ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = a + ab * t;
        let d = (q - p).length_squared();
        if d < best_d {
            best_d = d;
            best = q;
        }
    }
    best
}

/// Computes [`PuppetSense`] for every built puppet.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sense_puppets(
    time: Res<Time>,
    gravity: Option<Res<Gravity>>,
    collisions: Collisions,
    spatial: SpatialQuery,
    mut puppets: Query<(Entity, &PuppetRig, &PuppetConfig, &PuppetState, &mut PuppetSense)>,
    bodies: Query<(
        &Position,
        &Rotation,
        &LinearVelocity,
        &AngularVelocity,
        &ComputedMass,
        &ComputedCenterOfMass,
        &ComputedAngularInertia,
        Option<&CollidingEntities>,
    )>,
    puppet_bodies: Query<&PuppetBody>,
    sensors: Query<(), With<Sensor>>,
) {
    let gv = gravity.map(|g| g.0).unwrap_or(Vec3::NEG_Y * 9.81);
    let g = gv.length();
    let up = (-gv).normalize_or(Vec3::Y);
    for (puppet, rig, config, state, mut sense) in &mut puppets {
        let s = &mut *sense;
        let owner = Some(puppet);
        s.up = up;
        s.g = g;
        s.dt = time.delta_secs().max(1e-4);
        s.time = time.elapsed_secs();

        let mut com = Vec3::ZERO;
        let mut com_vel = Vec3::ZERO;
        let mut mass = 0.0;
        let mut peak_part = None;
        let mut peak_imp = 0.0_f32;
        let mut peak_spd = 0.0_f32;

        // Per-foot impulse-weighted CoP accumulators.
        let mut cop_acc = Vec3::ZERO;
        let mut cop_w = 0.0_f32;

        for part in PuppetPart::ALL {
            let i = part.index();
            let body_e = rig.parts[i].body;
            let Ok((pos, rot, lv, av, m, c, ai, colliding)) = bodies.get(body_e) else {
                continue;
            };
            let m = m.value();
            let world_com = pos.0 + rot.0 * c.0;
            let r = Mat3::from_quat(rot.0);
            let inertia = r * ai.tensor().to_mat3() * r.transpose();
            s.bodies[i] = BodyState {
                inertia,
                pos: pos.0,
                rot: rot.0,
                lin_vel: lv.0,
                ang_vel: av.0,
                com: world_com,
                mass: m,
            };
            com += world_com * m;
            com_vel += lv.0 * m;
            mass += m;

            // Gather full contact manifold data (normals, points, impulses, approach speeds)
            // excluding self-parts and trigger `Sensor`s.
            let mut touching = false;
            let mut imp_sum = 0.0_f32;
            let mut max_speed = 0.0_f32;
            let mut n_acc = Vec3::ZERO;
            let mut p_acc = Vec3::ZERO;
            let mut w_sum = 0.0_f32;

            for pair in collisions.collisions_with(body_e) {
                if !pair.is_touching() {
                    continue;
                }
                let other = if pair.collider1 == body_e {
                    pair.collider2
                } else {
                    pair.collider1
                };
                if sensors.contains(other) {
                    continue;
                }
                if puppet_bodies.get(other).is_ok_and(|b| Some(b.puppet) == owner) {
                    continue;
                }
                let n_sign = if pair.collider1 == body_e { -1.0 } else { 1.0 };
                for manifold in &pair.manifolds {
                    let n = manifold.normal * n_sign;
                    for pt in &manifold.points {
                        // Ignore speculative air-gap contacts with zero impulse so
                        // swing feet never report premature touchdown in mid-air.
                        if pt.penetration < -0.003 && pt.normal_impulse <= 1e-3 {
                            continue;
                        }
                        touching = true;
                        let imp = pt.normal_impulse.max(0.0);
                        let approach = (-pt.normal_speed).max(0.0);
                        imp_sum += imp;
                        max_speed = max_speed.max(approach);
                        let w = imp.max(0.01);
                        n_acc += n * w;
                        p_acc += pt.point * w;
                        w_sum += w;
                        if matches!(part, PuppetPart::FootL | PuppetPart::FootR) && n.dot(up) > 0.25 {
                            cop_acc += pt.point * w;
                            cop_w += w;
                        }
                    }
                }
            }
            if !touching
                && w_sum == 0.0
                && collisions.collisions_with(body_e).next().is_none()
                && let Some(c) = colliding
            {
                // Fallback to `CollidingEntities` only when no manifold exists yet.
                touching = c.iter().any(|e| {
                    !sensors.contains(*e)
                        && puppet_bodies
                            .get(*e)
                            .map_or(true, |b| Some(b.puppet) != owner)
                });
            }

            s.contacts[i] = touching;
            s.contact_impulses[i] = imp_sum;
            s.contact_speeds[i] = max_speed;
            if w_sum > 0.0 {
                s.contact_normals[i] = n_acc.normalize_or(up);
                s.contact_points[i] = p_acc / w_sum;
            } else {
                s.contact_normals[i] = if touching { up } else { Vec3::ZERO };
                s.contact_points[i] = pos.0;
            }

            // Track peak genuine impact (filters out steady resting foot weight).
            let is_foot = matches!(part, PuppetPart::FootL | PuppetPart::FootR);
            let min_speed = if is_foot { 1.2 } else { 0.45 };
            if touching && max_speed >= min_speed && imp_sum > peak_imp {
                peak_imp = imp_sum;
                peak_spd = max_speed;
                peak_part = Some(part);
            }
        }
        if mass <= 0.0 {
            continue;
        }
        s.total_mass = mass;
        s.com = com / mass;
        s.com_vel = com_vel / mass;
        s.peak_impact_part = peak_part;
        s.peak_impact_impulse = peak_imp;
        s.peak_impact_speed = peak_spd;

        // Vestibular acceleration (smoothed finite difference of CoM & Head velocity).
        let head_vel = s.bodies[PuppetPart::Head.index()].lin_vel;
        if s.steps > 0 {
            let raw_com_a = (s.com_vel - s.prev_com_vel) / s.dt;
            let raw_head_a = (head_vel - s.prev_head_vel) / s.dt;
            let alpha = (s.dt / 0.04).clamp(0.1, 1.0);
            s.com_accel = s.com_accel.lerp(raw_com_a.clamp_length_max(120.0), alpha);
            s.head_accel = s.head_accel.lerp(raw_head_a.clamp_length_max(180.0), alpha);
        } else {
            s.com_accel = Vec3::ZERO;
            s.head_accel = Vec3::ZERO;
        }
        s.prev_com_vel = s.com_vel;
        s.prev_head_vel = head_vel;

        // Centroidal angular momentum and composite inertia (Macchietto et al.
        // 2009; Geijtenbeek et al. 2012, eq. 14).
        let mut ang_mom = Vec3::ZERO;
        let mut com_inertia = Mat3::ZERO;
        for part in PuppetPart::ALL {
            let b = s.bodies[part.index()];
            let r = b.com - s.com;
            let v_rel = b.lin_vel - s.com_vel;
            ang_mom += r.cross(v_rel) * b.mass + b.inertia * b.ang_vel;
            com_inertia += b.inertia + (Mat3::IDENTITY * r.length_squared() - outer(r, r)) * b.mass;
        }
        s.angular_momentum = ang_mom;
        s.com_inertia = com_inertia;
        let det = com_inertia.determinant();
        s.centroidal_ang_vel = if det.abs() > 1e-8 {
            com_inertia.inverse() * ang_mom
        } else {
            Vec3::ZERO
        };

        // Support polygon, foot normals, and foot stability diagnostics.
        let mut pts = Vec::new();
        let mut lowest = f32::INFINITY;
        let mut lowest_contact = f32::INFINITY;
        let mut floor_normal_acc = Vec3::ZERO;
        for side in [Side::Left, Side::Right] {
            let foot = PuppetPart::foot(side);
            let fi = foot.index();
            let b = s.bodies[fi];
            let local_n = if s.contacts[fi] && s.contact_normals[fi].dot(up) > 0.35 {
                let n = clamp_normal_tilt(s.contact_normals[fi], up, 0.62);
                floor_normal_acc += n;
                n
            } else {
                up
            };
            s.foot_normals[side.index()] = local_n;
            let sole_up = (b.rot * rig.part(foot).up_local).normalize_or(up);
            s.foot_tilts[side.index()] = sole_up.angle_between(local_n);
            s.foot_ang_speeds[side.index()] = b.ang_vel.length();
            for corner in rig.part(foot).sole_local {
                let w = b.pos + b.rot * corner;
                let h = w.dot(up);
                lowest = lowest.min(h);
                if s.contacts[fi] {
                    lowest_contact = lowest_contact.min(h);
                    pts.push(s.flat(w));
                }
            }
        }
        s.stance = match (s.foot_down(Side::Left), s.foot_down(Side::Right)) {
            (true, true) => StanceState::Double,
            (true, false) => StanceState::Left,
            (false, true) => StanceState::Right,
            (false, false) => StanceState::Flight,
        };

        // ---- Environmental spatial probes (ground, slopes, walls) -----------
        let pelvis = s.bodies[PuppetPart::Pelvis.index()];
        let chest = s.bodies[PuppetPart::SpineUpper.index()];
        let pelvis_h = pelvis.pos.dot(up);
        let filter = SpatialQueryFilter::from_mask(config.layers().filters);
        let not_self_or_sensor = |e: Entity| {
            !sensors.contains(e)
                && puppet_bodies
                    .get(e)
                    .map_or(true, |b| Some(b.puppet) != owner)
        };

        // Downward ground raycast (skip for calm standing Animated puppets).
        let need_ground_ray = state.mode != PuppetMode::Animated || s.feet_down() < 2;
        let down_hit = if need_ground_ray && g > 1e-3 {
            Dir3::new(-up).ok().and_then(|down_dir| {
                let origin = pelvis.pos + up * 0.12;
                spatial
                    .cast_ray_predicate(origin, down_dir, 14.0, true, &filter, &not_self_or_sensor)
                    .map(|hit| SurfaceProbe {
                        point: origin + *down_dir * hit.distance,
                        normal: hit.normal,
                        distance: hit.distance,
                        entity: Some(hit.entity),
                    })
            })
        } else {
            None
        };

        let mut ground_normal = if floor_normal_acc.length_squared() > 0.25 {
            floor_normal_acc.normalize()
        } else {
            up
        };
        if let Some(hit) = down_hit
            && hit.normal.dot(up) > 0.35
        {
            let hit_h = hit.point.dot(up);
            if lowest_contact.is_finite() {
                s.ground = lowest_contact;
                if floor_normal_acc.length_squared() < 0.25 {
                    ground_normal = clamp_normal_tilt(hit.normal, up, 0.65);
                }
                s.ground_clearance = 0.0;
            } else {
                // Airborne / falling off a ledge: trust the true raycast surface!
                s.ground = hit_h;
                ground_normal = clamp_normal_tilt(hit.normal, up, 0.65);
                s.ground_clearance = (lowest - hit_h).max(0.0);
            }
        } else {
            let min_plausible_ground = pelvis_h - rig.height.max(0.5) * 1.35;
            let max_plausible_ground = pelvis_h + 0.15;
            let raw_ground = if lowest_contact.is_finite() { lowest_contact } else { lowest };
            s.ground = raw_ground.clamp(min_plausible_ground, max_plausible_ground);
            s.ground_clearance = if lowest_contact.is_finite() {
                0.0
            } else {
                (lowest - s.ground).max(0.0)
            };
        }
        s.ground_normal = ground_normal;

        // Free-fall timer and exact ballistic time-to-impact.
        let airborne_now = s.feet_down() == 0 && !s.body_touching();
        s.free_fall_time = if airborne_now { s.free_fall_time + s.dt } else { 0.0 };
        if g > 1e-3 {
            let torso_h = (s.com.dot(up) - s.ground - 0.24 * rig.height).max(0.0);
            let vy = s.com_vel.dot(up);
            if torso_h <= 1e-3 && vy <= 0.0 {
                s.predicted_impact_time = 0.0;
            } else {
                s.predicted_impact_time =
                    ((vy + (vy * vy + 2.0 * g * torso_h).sqrt()) / g).max(0.0);
            }
        } else {
            s.predicted_impact_time = f32::INFINITY;
        }

        s.steps = s.steps.saturating_add(1);
        s.support = convex_hull(pts);
        s.support_centre = if s.support.is_empty() {
            let l = s.flat(s.bodies[PuppetPart::FootL.index()].pos);
            let r = s.flat(s.bodies[PuppetPart::FootR.index()].pos);
            (l + r) * 0.5
        } else {
            s.support.iter().copied().sum::<Vec2>() / s.support.len() as f32
        };
        s.cop = if cop_w > 0.0 {
            s.flat(cop_acc / cop_w)
        } else {
            s.support_centre
        };

        // Constant-leg-length IPM capture point (Pratt & Tedrake 2006 / Coros 2010 §3.2).
        let h = (s.com.dot(up) - s.ground).max(0.05);
        s.capture_point = s.flat(s.com) + ipm_capture_offset(s.flat(s.com_vel), h, g);
        s.capture_margin = polygon_distance(&s.support, s.capture_point);

        // Approximate flywheel capture-region radius (Pratt et al. 2006).
        let effective_g = g.max(1e-3);
        let omega = (effective_g / h).sqrt();
        let flywheel_tau = 0.25 * rig.torque_unit(g);
        let lunge_time = 0.22;
        s.capture_region_radius =
            ((flywheel_tau / (mass * effective_g)) * (1.0 - (-omega * lunge_time).exp()))
                .min(0.09 * rig.height);

        // Trunk tilt, chest facing, and heading.
        let trunk_up = (chest.pos - pelvis.pos).normalize_or(up);
        s.trunk_tilt = trunk_up.angle_between(up);
        s.pelvis_height = pelvis_h - s.ground;
        let chest_fwd = (chest.rot * rig.part(PuppetPart::SpineUpper).forward_local).normalize_or(Vec3::NEG_Z);
        s.face_up_dot = chest_fwd.dot(up);
        let fwd = (pelvis.rot * rig.part(PuppetPart::Pelvis).forward_local).reject_from(up);
        if fwd.length_squared() > 1e-4 {
            s.heading = fwd.normalize();
        }

        // Ahead step-ground probe (for slopes, stairs, curbs during active balance).
        s.ahead_ground = None;
        if state.mode == PuppetMode::Active && s.capture_margin > -0.02 * rig.height && g > 1e-3
            && let Ok(down_dir) = Dir3::new(-up)
        {
            let origin = s.lift(s.capture_point, pelvis_h + 0.35);
            if let Some(hit) = spatial.cast_ray_predicate(
                origin,
                down_dir,
                rig.height * 1.4,
                true,
                &filter,
                &not_self_or_sensor,
            ) && hit.normal.dot(up) > 0.45
            {
                s.ahead_ground = Some(SurfaceProbe {
                    point: origin + *down_dir * hit.distance,
                    normal: clamp_normal_tilt(hit.normal, up, 0.65),
                    distance: hit.distance,
                    entity: Some(hit.entity),
                });
            }
        }

        // Wall / obstacle brace probe (for `WallBrace` during active stumble or fall).
        s.nearby_wall = None;
        if config.wall_brace && matches!(state.mode, PuppetMode::Active | PuppetMode::Falling) {
            let v_h = s.com_vel.reject_from(up);
            let lean_h = (chest.pos - pelvis.pos).reject_from(up);
            let probe_dir = if v_h.length_squared() > 0.09 {
                v_h.normalize()
            } else if lean_h.length_squared() > 0.01 {
                lean_h.normalize()
            } else {
                Vec3::ZERO
            };
            if let Ok(dir) = Dir3::new(probe_dir) {
                let max_dist = (0.75 + 0.25 * v_h.length()).clamp(0.75, 1.45);
                let origins = [chest.pos, s.bodies[PuppetPart::SpineLower.index()].pos];
                for origin in origins {
                    if let Some(hit) = spatial.cast_ray_predicate(
                        origin,
                        dir,
                        max_dist,
                        true,
                        &filter,
                        &not_self_or_sensor,
                    ) {
                        // Accept steep wall/obstacle facing our approach (not flat floor).
                        if hit.normal.dot(probe_dir) < -0.30 && hit.normal.dot(up).abs() < 0.65 {
                            s.nearby_wall = Some(SurfaceProbe {
                                point: origin + *dir * hit.distance,
                                normal: hit.normal,
                                distance: hit.distance,
                                entity: Some(hit.entity),
                            });
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_of_two_feet_is_ccw_and_contains_the_middle() {
        let pts = vec![
            Vec2::new(-0.2, -0.1),
            Vec2::new(-0.1, -0.1),
            Vec2::new(-0.2, 0.15),
            Vec2::new(-0.1, 0.15),
            Vec2::new(0.1, -0.1),
            Vec2::new(0.2, -0.1),
            Vec2::new(0.1, 0.15),
            Vec2::new(0.2, 0.15),
        ];
        let hull = convex_hull(pts);
        assert_eq!(hull.len(), 4);
        assert!(polygon_distance(&hull, Vec2::ZERO) < 0.0);
        let d = polygon_distance(&hull, Vec2::new(0.0, 0.5));
        assert!((d - 0.35).abs() < 1e-4, "{d}");
        assert_eq!(polygon_closest(&hull, Vec2::new(0.0, 0.5)), Vec2::new(0.0, 0.15));
    }

    #[test]
    fn empty_support_is_infinitely_far() {
        assert!(polygon_distance(&[], Vec2::ZERO).is_infinite());
    }

    #[test]
    fn ipm_capture_matches_lipm_at_low_speed_and_exceeds_at_high_speed() {
        let h = 0.95_f32;
        let g = 9.81_f32;
        let omega = (g / h).sqrt();
        let slow = ipm_capture_offset(Vec2::new(0.0, 0.1), h, g);
        assert!((slow.y - 0.1 / omega).abs() < 1e-4);

        let fast = ipm_capture_offset(Vec2::new(0.0, 2.0), h, g);
        assert!(fast.y > 2.0 / omega);
    }

    #[test]
    fn clamp_normal_tilt_preserves_flat_and_caps_steep() {
        assert!((clamp_normal_tilt(Vec3::Y, Vec3::Y, 0.5) - Vec3::Y).length() < 1e-5);
        let steep = Vec3::new(0.95, 0.31, 0.0).normalize();
        let clamped = clamp_normal_tilt(steep, Vec3::Y, 0.5);
        assert!((clamped.angle_between(Vec3::Y) - 0.5).abs() < 1e-3);
    }
}
