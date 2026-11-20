//! Building the physical body ("puppet rig") from a skeleton.
//!
//! The game marks a character with [`Puppet`]. Once its skeleton is in the
//! hierarchy (a glTF scene can take a few frames), [`build_puppet_rigs`] finds
//! the 16 bones of [`PuppetPart`], sizes one rigid body per part from the bone
//! geometry, weights it with the de Leva mass fractions and joints it to its
//! parent with Avian joints (ball-and-socket, or hinges for knees/elbows).
//!
//! Every body starts exactly at its bone's global pose, so a body's frame *is*
//! the bone's frame: writing the physical pose back is a copy, with no offsets.

use avian3d::prelude::*;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::behaviour::{PuppetBehaviours, PuppetRequests};
use crate::neuro::PuppetNeuroState;
use crate::sense::PuppetSense;
use crate::part::{
    Flexion, JointKind, PART_COUNT, PartShape, PuppetPart, Side, bone_rank, is_head_bone,
    is_toe_bone,
};
use crate::pose::{PuppetBlend, PuppetTargets};
use crate::state::{PuppetInbox, PuppetState};

/// Collision layer bit used by puppet bodies.
pub const PUPPET_LAYER_BIT: u32 = 1 << 30;

/// Marks a character to be simulated as a puppet. Requires the skeleton to be
/// a descendant of this entity.
#[derive(Component, Reflect, Default, Clone, Copy)]
#[reflect(Component, Default)]
#[require(
    PuppetConfig,
    PuppetState,
    PuppetBlend,
    PuppetBehaviours,
    PuppetRequests,
    PuppetInbox,
    PuppetSense,
    PuppetNeuroState,
    Transform
)]
pub struct Puppet;

/// Tuning of one puppet. Every field has a sane default for a 1.8 m, 70 kg
/// humanoid; most are relative so they scale with the rig.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default)]
pub struct PuppetConfig {
    /// Total body mass (kg).
    pub total_mass: f32,
    /// Global muscle strength multiplier (1 = healthy adult).
    pub strength: f32,
    /// Muscle damping ratio (1 = critical).
    pub damping_ratio: f32,
    /// Pin servo frequency (Hz) at pin weight 1 (pins pull bodies towards the
    /// animated pose in world space: the "hand of the animator").
    pub pin_frequency: f32,
    /// Pin weight on the pelvis while balancing (0 = honest physics). Pins
    /// pull towards the animated *position* too, so this fights stepping.
    pub balance_assist: f32,
    /// External torque keeping the trunk upright while balancing, as a
    /// fraction of `M·g·H` (0 = honest physics). The "helper" of game
    /// balancers: small pushes are absorbed, big ones still need steps.
    pub upright_assist: f32,
    /// Whether parts of the same puppet collide with each other (and with other puppets).
    pub self_collision: bool,
    /// Collision layer memberships of the bodies.
    pub collision_memberships: u32,
    /// Collision layer filters of the bodies (the puppet layer is removed
    /// unless `self_collision`).
    pub collision_filters: u32,
    /// Friction coefficient of the bodies.
    pub friction: f32,
    /// Start [`crate::behaviour::BodyBalance`] automatically in the active state.
    pub auto_balance: bool,
    /// Let impulses knock the puppet out of the animated state.
    pub react_to_hits: bool,
    /// Impulse (N·s) under which a hit only flops the hit limb ("partial ragdoll").
    pub partial_hit_impulse: f32,
    /// Maximum recovery steps before giving up and falling.
    pub max_steps: u32,
    /// Seconds lying still before getting up (negative = never).
    pub getup_delay: f32,
    /// Seconds the get-up blend lasts.
    pub getup_duration: f32,
    /// Move the puppet entity to the pelvis when getting up, so the animation
    /// (and the game's controller) continue from where the body lies.
    pub move_root_on_getup: bool,
    /// Seconds to blend between animation and physics when entering/leaving the active state.
    pub blend_time: f32,
    /// Writhe on the ground (`BodyWrithe`) while down.
    pub writhe_when_down: bool,
    /// Mass multiplier of the feet (taken from the thighs, the total is kept).
    /// A 1 kg foot carrying 70 kg through solver joints lets the body sink
    /// into the ground; heavier feet keep the mass ratio sane.
    pub foot_mass_scale: f32,
    /// Neuromuscular activation rise time constant (s, ~0.045 s = human muscle
    /// recruitment lag; 0 = instantaneous servo).
    pub muscle_activation_time: f32,
    /// Sensorimotor reflex latency on unexpected hits and bumps (s, ~0.065 s =
    /// passive inertial absorption before active flinch/guard kicks in).
    pub reaction_delay: f32,
    /// Automatically reach out to brace against nearby walls and obstacles when stumbling/falling.
    pub wall_brace: bool,
    /// Automatically reach toward heavy wound locations (`ReachForWound`) when struck.
    pub reach_for_wound: bool,
    /// Drive `GettingUp` through a biomechanical multi-stage push-up/stand-up motor program
    /// instead of pure world-space pin levitation.
    pub physical_getup: bool,
    /// Seconds of progressive anatomical neuromuscular collapse on `Kill` (legs buckle
    /// first, spine follows, arms/head trail; 0 = instant limp ragdoll).
    pub dying_collapse_time: f32,
    /// Passive viscoelastic soft-tissue damping scale (stays active when limp/dead so
    /// limbs settle with biological weight instead of rubbery pendulum bouncing).
    pub passive_joint_damping: f32,
    /// Minimum contact impulse (N·s) on a body impact to emit [`crate::state::PuppetEventKind::BodyImpact`].
    pub impact_event_threshold: f32,
}

impl Default for PuppetConfig {
    fn default() -> Self {
        Self {
            total_mass: 70.0,
            strength: 1.0,
            damping_ratio: 1.0,
            pin_frequency: 6.0,
            balance_assist: 0.0,
            upright_assist: 0.15,
            self_collision: false,
            collision_memberships: PUPPET_LAYER_BIT,
            collision_filters: u32::MAX,
            friction: 0.9,
            auto_balance: true,
            react_to_hits: true,
            partial_hit_impulse: 25.0,
            max_steps: 4,
            getup_delay: 1.5,
            getup_duration: 1.2,
            move_root_on_getup: true,
            blend_time: 0.25,
            writhe_when_down: false,
            foot_mass_scale: 4.0,
            muscle_activation_time: 0.045,
            reaction_delay: 0.065,
            wall_brace: true,
            reach_for_wound: true,
            physical_getup: true,
            dying_collapse_time: 0.30,
            passive_joint_damping: 0.22,
            impact_event_threshold: 10.0,
        }
    }
}

impl PuppetConfig {
    pub fn layers(&self) -> CollisionLayers {
        let mut filters = self.collision_filters;
        if !self.self_collision {
            filters &= !PUPPET_LAYER_BIT;
        }
        CollisionLayers::from_bits(self.collision_memberships | PUPPET_LAYER_BIT, filters)
    }
}

/// A physical body of a puppet.
#[derive(Component, Clone, Copy, Debug)]
pub struct PuppetBody {
    pub puppet: Entity,
    pub part: PuppetPart,
}

/// Bodies and joints belong to their puppet; despawning the puppet despawns them.
#[derive(Component, Clone, Copy, Debug)]
#[relationship(relationship_target = PuppetParts)]
pub struct PuppetPartOf(pub Entity);

/// The bodies and joints of a puppet (see [`PuppetPartOf`]).
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = PuppetPartOf, linked_spawn)]
pub struct PuppetParts(Vec<Entity>);

impl PuppetParts {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

/// Static description of one part, measured when the rig was built.
#[derive(Clone, Debug)]
pub struct PartRig {
    pub part: PuppetPart,
    pub body: Entity,
    pub bone: Entity,
    pub joint: Option<Entity>,
    /// The [`MuscleJoint`](crate::drive::MuscleJoint) driving the joint.
    pub muscle: Option<Entity>,
    /// Bones from just below the puppet root down to `bone` (inclusive), each
    /// with the part it drives, if any.
    pub path: Vec<(Entity, Option<PuppetPart>)>,
    pub mass: f32,
    /// Segment length (m).
    pub length: f32,
    pub radius: f32,
    /// Unit segment direction (bone → end) in the body frame.
    pub axis_local: Vec3,
    /// World up at build time, in the body frame (the sole normal for feet).
    pub up_local: Vec3,
    /// Character forward at build time, in the body frame.
    pub forward_local: Vec3,
    /// End of the segment in the body frame (`axis_local * length`).
    pub end_local: Vec3,
    /// Sole corners (feet only), body frame.
    pub sole_local: [Vec3; 4],
    /// Joint anchor in the parent body frame (the joint sits at this body's origin).
    pub anchor_in_parent: Vec3,
}

/// The built rig of a puppet.
#[derive(Component, Clone, Debug)]
pub struct PuppetRig {
    pub parts: Vec<PartRig>,
    /// Standing height (m).
    pub height: f32,
    /// Pelvis height above the soles in the build pose.
    pub pelvis_height: f32,
    /// Centre-of-mass height above the soles in the build pose.
    pub com_height: f32,
    /// Ankle height above the sole.
    pub ankle_height: f32,
    pub total_mass: f32,
    /// Static anchor body for world-space constraints (foot planting).
    pub anchor: Entity,
    /// Planting constraints of the left and right foot (see [`crate::drive`]).
    pub plants: [Entity; 2],
}

impl PuppetRig {
    #[inline]
    pub fn part(&self, part: PuppetPart) -> &PartRig {
        &self.parts[part.index()]
    }

    /// `M·g·H` (standard gravity), the unit of muscle stiffness/torque.
    /// Muscles are anatomical: they do not weaken in low gravity.
    pub fn torque_unit(&self, _g: f32) -> f32 {
        self.total_mass * 9.81 * self.height
    }
}

/// Set when the skeleton could not be mapped (missing bones). Remove it to retry.
#[derive(Component, Debug, Clone)]
pub struct PuppetRigError(pub String);

/// Frames spent waiting for a skeleton.
#[derive(Component, Default)]
pub struct PuppetRigPending(u32);

const RIG_WAIT_FRAMES: u32 = 600;

/// Finds the part bones among the descendants of `root`.
///
/// Every part must be found, with each part's bone a descendant of its parent
/// part's bone. Returns the missing parts otherwise.
pub fn find_part_bones(
    root: Entity,
    children: &Query<&Children>,
    names: &Query<&Name>,
    parents: &Query<&ChildOf>,
) -> Result<[Entity; PART_COUNT], Vec<PuppetPart>> {
    let mut bones: Vec<(Entity, String)> = Vec::new();
    for e in children.iter_descendants(root) {
        if let Ok(n) = names.get(e) {
            bones.push((e, n.as_str().to_string()));
        }
    }
    let mut out = [Entity::PLACEHOLDER; PART_COUNT];
    let mut missing = Vec::new();
    for part in PuppetPart::ALL {
        let parent_bone = part.parent().map(|p| out[p.index()]);
        let best = bones
            .iter()
            .filter(|(e, _)| !out.contains(e))
            .filter(|(e, _)| match parent_bone {
                Some(pb) if pb != Entity::PLACEHOLDER => is_descendant(*e, pb, parents),
                _ => true,
            })
            .filter_map(|(e, n)| bone_rank(part, n).map(|r| (r, *e)))
            .min_by_key(|(r, _)| *r);
        match best {
            Some((_, e)) => out[part.index()] = e,
            None => missing.push(part),
        }
    }
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(missing)
    }
}

fn is_descendant(e: Entity, ancestor: Entity, parents: &Query<&ChildOf>) -> bool {
    let mut cur = e;
    while let Ok(p) = parents.get(cur) {
        cur = p.parent();
        if cur == ancestor {
            return true;
        }
    }
    false
}

/// First descendant of `bone` matching `pred` (breadth-first), with its global position.
fn find_child_bone(
    bone: Entity,
    children: &Query<&Children>,
    names: &Query<&Name>,
    pred: impl Fn(&str) -> bool,
) -> Option<Entity> {
    children
        .iter_descendants(bone)
        .find(|e| names.get(*e).is_ok_and(|n| pred(n.as_str())))
}

/// Volume of a capsule (cylinder `length` + two hemispheres).
pub fn capsule_volume(radius: f32, length: f32) -> f32 {
    use std::f32::consts::PI;
    PI * radius * radius * length + 4.0 / 3.0 * PI * radius.powi(3)
}

/// Orthonormal rotation whose `Y` is `y` and whose `X` is as close as possible to `x_hint`.
pub fn basis_from_y(y: Vec3, x_hint: Vec3) -> Quat {
    let y = y.normalize_or(Vec3::Y);
    let mut x = x_hint - y * x_hint.dot(y);
    if x.length_squared() < 1e-6 {
        x = y.any_orthonormal_vector();
    }
    let x = x.normalize();
    let z = x.cross(y);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// Orthonormal basis for Avian 0.8's [`SphericalJoint`]: Avian's XPBD solver
/// uses `swing_axis = DEFAULT_TWIST_AXIS.any_orthonormal_vector()` as the
/// longitudinal cone/twist axis and `DEFAULT_TWIST_AXIS` as the perpendicular
/// twist-reference axis. Mapping `swing_axis → bone_dir` ensures `swing_limit`
/// genuinely constrains the cone around `bone_dir` and `twist_limit` genuinely
/// constrains axial spin around `bone_dir`.
fn spherical_joint_basis(bone_dir: Vec3, ref_hint: Vec3) -> Quat {
    let a_loc = SphericalJoint::DEFAULT_TWIST_AXIS.any_orthonormal_vector().normalize();
    let b_loc = SphericalJoint::DEFAULT_TWIST_AXIS.normalize();
    let c_loc = a_loc.cross(b_loc);
    let a_w = bone_dir.normalize_or(Vec3::NEG_Y);
    let mut b_w = ref_hint - a_w * ref_hint.dot(a_w);
    if b_w.length_squared() < 1e-6 {
        b_w = a_w.any_orthonormal_vector();
    }
    let b_w = b_w.normalize();
    let c_w = a_w.cross(b_w);
    let m_loc = Mat3::from_cols(a_loc, b_loc, c_loc);
    let m_w = Mat3::from_cols(a_w, b_w, c_w);
    Quat::from_mat3(&(m_w * m_loc.transpose())).normalize()
}

/// Right-handed rotation of `from` to `to` around `up`.
///
/// Unlike [`Quat::from_rotation_arc`], this always rotates strictly around `up`
/// even at 180° (`from ≈ -to`), never flipping a character upside down.
pub fn yaw_between(from: Vec3, to: Vec3, up: Vec3) -> Quat {
    let up = up.normalize_or(Vec3::Y);
    Quat::from_axis_angle(up, signed_angle(from, to, up))
}

/// Geometry measured from the skeleton, before spawning anything.
struct Measured {
    pos: [Vec3; PART_COUNT],
    rot: [Quat; PART_COUNT],
    scale: [Vec3; PART_COUNT],
    end: [Vec3; PART_COUNT],
    up: Vec3,
    forward: Vec3,
    right: Vec3,
    ground_y: f32,
    height: f32,
    ankle_height: f32,
    toe: [Option<Vec3>; 2],
}

fn measure(
    bones: &[Entity; PART_COUNT],
    children: &Query<&Children>,
    names: &Query<&Name>,
    world: &dyn Fn(Entity) -> Option<Affine3A>,
    up: Vec3,
) -> Measured {
    use PuppetPart::*;
    let mut pos = [Vec3::ZERO; PART_COUNT];
    let mut rot = [Quat::IDENTITY; PART_COUNT];
    let mut scale = [Vec3::ONE; PART_COUNT];
    for part in PuppetPart::ALL {
        let aff = world(bones[part.index()]).unwrap_or_default();
        let (s, r, t) = aff.to_scale_rotation_translation();
        pos[part.index()] = t;
        rot[part.index()] = r.normalize();
        scale[part.index()] = s;
    }
    let p = |part: PuppetPart| pos[part.index()];
    let right = (p(ThighR) - p(ThighL)).reject_from(up).normalize_or(Vec3::X);
    let forward = up.cross(right).normalize_or(Vec3::NEG_Z);

    let head_bone = if bones[Head.index()] != Entity::PLACEHOLDER
        && names.get(bones[Head.index()]).is_ok_and(|n| is_head_bone(n.as_str()))
    {
        Some(bones[Head.index()])
    } else {
        find_child_bone(bones[Head.index()], children, names, is_head_bone)
    };
    let head_pos = head_bone
        .and_then(world)
        .map(|a| Vec3::from(a.translation))
        .unwrap_or(p(Head) + up * 0.1);

    let toe = [FootL, FootR].map(|f| {
        find_child_bone(bones[f.index()], children, names, is_toe_bone)
            .and_then(world)
            .map(|a| Vec3::from(a.translation))
    });

    let foot_y = p(FootL).dot(up).min(p(FootR).dot(up));
    let span = (head_pos.dot(up) - foot_y).max(0.2);
    // Head bone ≈ 0.87 H, ankle ≈ 0.039 H above the sole.
    let height = span / 0.83;
    let mut ankle_height = 0.039 * height;
    if let Some(t) = toe[0].or(toe[1]) {
        // The ball of the foot sits ~0.02 H above the sole.
        let from_toe = foot_y - (t.dot(up) - 0.02 * height);
        if from_toe > 0.01 {
            ankle_height = from_toe;
        }
    }
    let ground_y = foot_y - ankle_height;

    let mut end = [Vec3::ZERO; PART_COUNT];
    for part in PuppetPart::ALL {
        let i = part.index();
        end[i] = if let Some(c) = part.segment_child() {
            p(c)
        } else {
            match part {
                Pelvis => (p(ThighL) + p(ThighR)) * 0.5,
                Head => {
                    // From the neck through the skull to the top of the head.
                    let dir = (head_pos - p(Head)).normalize_or(up);
                    let base = if head_bone == Some(bones[Head.index()]) { p(Head) } else { head_pos };
                    base + dir * 0.13 * height
                }
                HandL | HandR => {
                    let fore = if part == HandL { LowerArmL } else { LowerArmR };
                    p(part) + (p(part) - p(fore)).normalize_or(-up) * 0.08 * height
                }
                FootL | FootR => {
                    let k = if part == FootL { 0 } else { 1 };
                    toe[k].unwrap_or(p(part) + forward * 0.10 * height)
                }
                _ => p(part) - up * 0.1,
            }
        };
    }
    Measured {
        pos,
        rot,
        scale,
        end,
        up,
        forward,
        right,
        ground_y,
        height,
        ankle_height,
        toe,
    }
}

/// Collider (in the body frame), its volume and the rig data of one part.
struct PartGeometry {
    collider: Collider,
    volume: f32,
    length: f32,
    radius: f32,
    axis_local: Vec3,
    end_local: Vec3,
    sole_local: [Vec3; 4],
}

fn part_geometry(part: PuppetPart, m: &Measured) -> PartGeometry {
    use PuppetPart::*;
    let i = part.index();
    let h = m.height;
    let body_pos = m.pos[i];
    let inv = m.rot[i].inverse();
    let to_local = |w: Vec3| inv * (w - body_pos);
    let dir_local = |w: Vec3| (inv * w).normalize_or(Vec3::Y);
    let mut r = part.radius_fraction() * h;
    let seg = m.end[i] - body_pos;
    let seg_len = seg.length().max(0.02);
    let axis_local = dir_local(seg);

    match part.shape() {
        PartShape::Segment => {
            r = r.min(0.45 * seg_len);
            let cyl = (seg_len - 2.0 * r).max(0.01);
            let mid = to_local(body_pos + seg * 0.5);
            let rot = Quat::from_rotation_arc(Vec3::Y, axis_local);
            PartGeometry {
                collider: Collider::compound(vec![(mid, rot, Collider::capsule(r, cyl))]),
                volume: capsule_volume(r, cyl),
                length: seg_len,
                radius: r,
                axis_local,
                end_local: axis_local * seg_len,
                sole_local: [Vec3::ZERO; 4],
            }
        }
        PartShape::Across => {
            let (a, b, centre) = if part == Pelvis {
                let a = m.pos[ThighL.index()];
                let b = m.pos[ThighR.index()];
                (a, b, ((a + b) * 0.5).lerp(body_pos, 0.5))
            } else {
                let a = m.pos[UpperArmL.index()];
                let b = m.pos[UpperArmR.index()];
                let mid_v = body_pos.lerp(m.end[i], 0.45);
                // Shoulder line, centred over the spine.
                let c = mid_v + ((a + b) * 0.5 - mid_v).reject_from(m.up) * 0.0;
                (a, b, c)
            };
            let across = b - a;
            let w = across.length().max(0.05);
            let span = if part == Pelvis { w } else { 0.75 * w };
            r = r.min(0.5 * span);
            let cyl = (span - 2.0 * r).max(0.01);
            let rot = Quat::from_rotation_arc(Vec3::Y, dir_local(across));
            PartGeometry {
                collider: Collider::compound(vec![(to_local(centre), rot, Collider::capsule(r, cyl))]),
                volume: capsule_volume(r, cyl),
                length: seg_len,
                radius: r,
                axis_local,
                end_local: axis_local * seg_len,
                sole_local: [Vec3::ZERO; 4],
            }
        }
        PartShape::FootBox => {
            let k = if part == FootL { 0 } else { 1 };
            let toe = m.toe[k];
            let fwd = toe
                .map(|t| (t - body_pos).reject_from(m.up))
                .filter(|v| v.length() > 0.02)
                .unwrap_or(m.forward)
                .normalize();
            let toe_dist = toe.map(|t| (t - body_pos).reject_from(m.up).length()).unwrap_or(0.10 * h);
            let back = 0.045 * h;
            let front = toe_dist + 0.035 * h;
            let length = back + front;
            let height = (body_pos.dot(m.up) - m.ground_y).max(0.02);
            let width = 0.055 * h;
            let side = m.up.cross(fwd).normalize();
            let box_rot_w = Quat::from_mat3(&Mat3::from_cols(side, m.up, fwd)).normalize();
            let centre = body_pos + fwd * ((front - back) * 0.5) - m.up * (height * 0.5);
            let local_rot = inv * box_rot_w;
            let he = Vec3::new(width * 0.5, height * 0.5, length * 0.5);
            let sole_w = [
                centre + side * he.x - m.up * he.y + fwd * he.z,
                centre - side * he.x - m.up * he.y + fwd * he.z,
                centre + side * he.x - m.up * he.y - fwd * he.z,
                centre - side * he.x - m.up * he.y - fwd * he.z,
            ];
            PartGeometry {
                collider: Collider::compound(vec![(
                    to_local(centre),
                    local_rot,
                    Collider::cuboid(width, height, length),
                )]),
                volume: width * height * length,
                length: toe_dist,
                radius: height * 0.5,
                axis_local: dir_local(fwd),
                end_local: dir_local(fwd) * toe_dist,
                sole_local: sole_w.map(to_local),
            }
        }
    }
}

/// Spawns the bodies and joints of puppets whose skeleton is ready.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn build_puppet_rigs(
    mut commands: Commands,
    mut puppets: Query<
        (Entity, &PuppetConfig, Option<&mut PuppetRigPending>),
        (With<Puppet>, Without<PuppetRig>, Without<PuppetRigError>),
    >,
    children: Query<&Children>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    gts: Query<&GlobalTransform>,
    transforms: Query<&Transform>,
    gravity: Option<Res<Gravity>>,
    skins: crate::skin::PuppetSkins,
) {
    let g = gravity.map(|g| g.0).unwrap_or(Vec3::NEG_Y * 9.81);
    let up = (-g).normalize_or(Vec3::Y);
    for (puppet, config, pending) in &mut puppets {
        let bones = match find_part_bones(puppet, &children, &names, &parents) {
            Ok(b) => b,
            Err(missing) => {
                // The glTF scene may still be spawning; wait, then give up loudly.
                let waited = pending.map(|mut p| {
                    p.0 += 1;
                    p.0
                });
                match waited {
                    None => {
                        commands.entity(puppet).insert(PuppetRigPending::default());
                    }
                    Some(n) if n >= RIG_WAIT_FRAMES => {
                        let msg = format!("skeleton bones not found for {missing:?}");
                        warn!("puppet {puppet}: {msg}");
                        commands.entity(puppet).insert(PuppetRigError(msg));
                    }
                    _ => {}
                }
                continue;
            }
        };
        // Compose the bone globals from the locals: a skeleton spawned this
        // frame has no propagated `GlobalTransform` yet.
        let root_local = transforms.get(puppet).map(|t| t.compute_affine()).unwrap_or_default();
        let root_aff = match parents.get(puppet).ok().and_then(|p| gts.get(p.parent()).ok()) {
            Some(parent) => parent.affine() * root_local,
            None => root_local,
        };
        let world = |e: Entity| -> Option<Affine3A> {
            let mut chain = Vec::new();
            let mut cur = e;
            while cur != puppet {
                chain.push(cur);
                cur = parents.get(cur).ok()?.parent();
            }
            let mut a = root_aff;
            for c in chain.iter().rev() {
                a *= transforms.get(*c).ok()?.compute_affine();
            }
            Some(a)
        };
        let m = measure(&bones, &children, &names, &world, up);
        let fall_colliders = skins.fit_fall_colliders(puppet, &bones, m.height, &children, &parents, world);
        let rig = spawn_rig(&mut commands, puppet, config, &bones, &m, &parents, fall_colliders);
        let mut targets = PuppetTargets::default();
        for part in PuppetPart::ALL {
            let i = part.index();
            targets.anim_local[i] = transforms.get(bones[i]).copied().unwrap_or_default();
            targets.world_pos[i] = m.pos[i];
            targets.world_rot[i] = m.rot[i];
            targets.prev_pos[i] = m.pos[i];
            targets.prev_rot[i] = m.rot[i];
            targets.global_scale[i] = m.scale[i];
        }
        for part in PuppetPart::ALL {
            if let Some(p) = part.parent() {
                targets.local[part.index()] =
                    (m.rot[p.index()].inverse() * m.rot[part.index()]).normalize();
            }
        }
        targets.initialized = true;
        commands
            .entity(puppet)
            .remove::<PuppetRigPending>()
            .insert((rig, targets));
    }
}

fn spawn_rig(
    commands: &mut Commands,
    puppet: Entity,
    config: &PuppetConfig,
    bones: &[Entity; PART_COUNT],
    m: &Measured,
    parents: &Query<&ChildOf>,
    mut fall_colliders: [Option<crate::skin::FallCollider>; PART_COUNT],
) -> PuppetRig {
    let layers = config.layers();
    let mut parts: Vec<PartRig> = Vec::with_capacity(PART_COUNT);
    for part in PuppetPart::ALL {
        let i = part.index();
        let geo = part_geometry(part, m);
        let mass = config.total_mass * part_mass_fraction(part, config.foot_mass_scale);
        let density = mass / geo.volume.max(1e-6);
        // The contact shape can change during a fall. Preserve the anatomical
        // mass, centre of mass and inertia used to tune the muscles.
        let fall = fall_colliders[i].take().map(|shape| {
            (shape, MassPropertiesBundle::from_shape(&geo.collider, density))
        });
        let body = commands
            .spawn((
                Name::new(format!("puppet {part:?}")),
                RigidBody::Dynamic,
                Transform::from_translation(m.pos[i]).with_rotation(m.rot[i]),
                geo.collider,
                ColliderDensity(density),
                Friction::new(config.friction),
                Restitution::ZERO,
                LinearDamping(0.05),
                AngularDamping(0.3),
                MaxAngularSpeed(40.0),
                SleepingDisabled,
                CollidingEntities::default(),
                layers,
                PuppetBody { puppet, part },
                PuppetPartOf(puppet),
            ))
            .id();

        if let Some(bundle) = fall {
            commands.entity(body).insert(bundle);
        }

        // Path from just below the puppet root down to the bone.
        let mut path = Vec::new();
        let mut cur = bones[i];
        loop {
            let tag = bones.iter().position(|b| *b == cur).map(PuppetPart::from_index);
            path.push((cur, tag));
            match parents.get(cur) {
                Ok(p) if p.parent() != puppet => cur = p.parent(),
                _ => break,
            }
        }
        path.reverse();

        let inv = m.rot[i].inverse();
        let anchor_in_parent = part
            .parent()
            .map(|p| m.rot[p.index()].inverse() * (m.pos[i] - m.pos[p.index()]))
            .unwrap_or(Vec3::ZERO);
        parts.push(PartRig {
            part,
            body,
            bone: bones[i],
            joint: None,
            muscle: None,
            path,
            mass,
            length: geo.length,
            radius: geo.radius,
            axis_local: geo.axis_local,
            up_local: inv * m.up,
            forward_local: inv * m.forward,
            end_local: geo.end_local,
            sole_local: geo.sole_local,
            anchor_in_parent,
        });
    }

    // Joints: the anchor is the child's origin (its bone position).
    for part in PuppetPart::ALL {
        let Some(parent) = part.parent() else { continue };
        let Some(kind) = part.joint_kind() else { continue };
        let i = part.index();
        let p = parent.index();
        let body1 = parts[p].body;
        let body2 = parts[i].body;
        let dir = (m.rot[i] * parts[i].axis_local).normalize_or(-m.up);
        let anchor1 = parts[i].anchor_in_parent;
        let joint = match kind {
            JointKind::Ball {
                swing,
                twist,
                bias_forward,
                bias_outward,
            } => {
                let child_basis = spherical_joint_basis(dir, m.right);
                // Bias the parent swing-cone center anatomically in the character's
                // rest frame while guaranteeing the build-time pose sits comfortably
                // inside the cone (important when puppets spawn on demand mid-stride
                // or mid-attack!).
                let base_dir = match part {
                    PuppetPart::SpineLower | PuppetPart::SpineUpper | PuppetPart::Head => m.up,
                    // Use the skeleton's actual standing ankle→toe axis so rigs with
                    // elevated ankle bones (e.g. 20° slope to ball) keep symmetric
                    // plantarflexion/dorsiflexion margins instead of pinching plantarflexion!
                    PuppetPart::FootL | PuppetPart::FootR => dir,
                    _ => -m.up,
                };
                let outward = match part.side() {
                    Some(Side::Left) => -m.right,
                    Some(Side::Right) => m.right,
                    None => Vec3::ZERO,
                };
                let mut cone_dir = base_dir;
                if bias_forward.abs() > 1e-4 {
                    let pitch_target = match part {
                        PuppetPart::FootL | PuppetPart::FootR => m.up,
                        _ => m.forward,
                    };
                    let pitch_axis = base_dir.cross(pitch_target);
                    if pitch_axis.length_squared() > 1e-4 {
                        cone_dir = Quat::from_axis_angle(pitch_axis.normalize(), bias_forward) * cone_dir;
                    }
                }
                if bias_outward.abs() > 1e-4 && outward.length_squared() > 0.5 {
                    let abd_axis = base_dir.cross(outward);
                    if abd_axis.length_squared() > 1e-4 {
                        cone_dir = Quat::from_axis_angle(abd_axis.normalize(), bias_outward) * cone_dir;
                    }
                }
                cone_dir = cone_dir.normalize_or(dir);
                // Keep the build-pose `dir` at least 0.14 rad inside the swing limit.
                let safe_limit = (swing - 0.14).max(0.05);
                let cur_angle = dir.angle_between(cone_dir);
                if cur_angle > safe_limit && cur_angle > 1e-4 {
                    let frac = safe_limit / cur_angle;
                    let arc = Quat::from_rotation_arc(dir, cone_dir);
                    cone_dir = (Quat::IDENTITY.slerp(arc, frac) * dir).normalize_or(dir);
                }
                // Rotate parent frame by the pure swing arc `dir → cone_dir` so the
                // initial pose has zero twist relative to the parent frame.
                let parent_basis = (Quat::from_rotation_arc(dir, cone_dir) * child_basis).normalize();
                commands
                    .spawn((
                        Name::new(format!("puppet joint {part:?}")),
                        SphericalJoint::new(body1, body2)
                            .with_local_frame1(Isometry3d::new(anchor1, m.rot[p].inverse() * parent_basis))
                            .with_local_frame2(Isometry3d::new(Vec3::ZERO, m.rot[i].inverse() * child_basis))
                            .with_swing_limits(-swing, swing)
                            .with_twist_limits(-twist, twist),
                        JointCollisionDisabled,
                        PuppetPartOf(puppet),
                    ))
                    .id()
            }
            JointKind::Hinge { min, max, flexion } => {
                let hinge = hinge_axis(dir, m, flexion);
                let parent_dir = (m.rot[p] * parts[p].axis_local).normalize_or(-m.up);
                let flex0 = signed_angle(parent_dir, dir, hinge);
                let basis = Quat::from_mat3(&Mat3::from_cols(
                    hinge,
                    (dir - hinge * dir.dot(hinge)).normalize_or(m.up),
                    hinge.cross((dir - hinge * dir.dot(hinge)).normalize_or(m.up)),
                ))
                .normalize();
                commands
                    .spawn((
                        Name::new(format!("puppet joint {part:?}")),
                        RevoluteJoint::new(body1, body2)
                            .with_hinge_axis(Vec3::X)
                            .with_local_frame1(Isometry3d::new(anchor1, m.rot[p].inverse() * basis))
                            .with_local_frame2(Isometry3d::new(Vec3::ZERO, m.rot[i].inverse() * basis))
                            .with_angle_limits(min - flex0, max - flex0),
                        JointCollisionDisabled,
                        PuppetPartOf(puppet),
                    ))
                    .id()
            }
        };
        parts[i].joint = Some(joint);
        let mut muscle = crate::drive::MuscleJoint::new(body1, body2);
        muscle.target = (m.rot[p].inverse() * m.rot[i]).normalize();
        muscle.twist_axis_local = parts[i].axis_local;
        muscle.twist_scale = part.twist_stiffness_scale();
        parts[i].muscle = Some(
            commands
                .spawn((Name::new(format!("puppet muscle {part:?}")), muscle, PuppetPartOf(puppet)))
                .id(),
        );
    }

    // Collision between non-adjacent parts that overlap at rest (thighs vs
    // the lower spine, shoulders vs the head…) is only an issue with
    // self-collision on; then disable the usual offenders with an extra
    // contact-only joint-less filter: a distance joint with no limits.
    if config.self_collision {
        use PuppetPart::*;
        for (a, b) in [
            (SpineLower, ThighL),
            (SpineLower, ThighR),
            (SpineLower, UpperArmL),
            (SpineLower, UpperArmR),
            (Head, UpperArmL),
            (Head, UpperArmR),
            (Pelvis, CalfL),
            (Pelvis, CalfR),
            (ThighL, ThighR),
        ] {
            commands.spawn((
                DistanceJoint::new(parts[a.index()].body, parts[b.index()].body)
                    .with_limits(0.0, 1000.0),
                JointCollisionDisabled,
                PuppetPartOf(puppet),
            ));
        }
    }

    let feet_y = m.ground_y;
    let pelvis_height = m.pos[PuppetPart::Pelvis.index()].dot(m.up) - feet_y;
    let mut com = Vec3::ZERO;
    for part in PuppetPart::ALL {
        let i = part.index();
        let mid = m.pos[i] + (m.end[i] - m.pos[i]) * 0.5;
        com += mid * parts[i].mass;
    }
    com /= config.total_mass;
    // Foot planting: a muscle from a static anchor (identity pose) to each
    // foot, off until a behaviour plants the foot.
    let anchor = commands
        .spawn((Name::new("puppet anchor"), RigidBody::Static, Transform::IDENTITY, PuppetPartOf(puppet)))
        .id();
    let plants = [PuppetPart::FootL, PuppetPart::FootR].map(|foot| {
        commands
            .spawn((
                Name::new(format!("puppet plant {foot:?}")),
                crate::drive::MuscleJoint::new(anchor, parts[foot.index()].body),
                PuppetPartOf(puppet),
            ))
            .id()
    });
    PuppetRig {
        parts,
        height: m.height,
        pelvis_height,
        com_height: com.dot(m.up) - feet_y,
        ankle_height: m.ankle_height,
        total_mass: config.total_mass,
        anchor,
        plants,
    }
}

/// Mass fraction of a part after moving mass from the thighs to the feet.
pub fn part_mass_fraction(part: PuppetPart, foot_scale: f32) -> f32 {
    let foot = PuppetPart::FootL.mass_fraction();
    // Each foot borrows only the mass available above its thigh's minimum.
    let available = PuppetPart::ThighL.mass_fraction() - 0.02;
    let extra = (foot * (foot_scale.max(1.0) - 1.0)).min(available);
    match part {
        PuppetPart::FootL | PuppetPart::FootR => foot + extra,
        PuppetPart::ThighL | PuppetPart::ThighR => (part.mass_fraction() - extra).max(0.02),
        _ => part.mass_fraction(),
    }
}

/// Flexion axis of a hinge: positive rotation about it flexes the joint.
fn hinge_axis(child_dir: Vec3, m: &Measured, flexion: Flexion) -> Vec3 {
    let towards = match flexion {
        Flexion::Backward => -m.forward,
        Flexion::Forward => m.forward,
    };
    let a = child_dir.cross(towards);
    if a.length_squared() > 1e-4 {
        a.normalize()
    } else {
        match flexion {
            Flexion::Backward => -m.right,
            Flexion::Forward => m.right,
        }
    }
}

/// Signed angle from `a` to `b` around `axis` (right-handed).
pub fn signed_angle(a: Vec3, b: Vec3, axis: Vec3) -> f32 {
    a.cross(b).dot(axis).atan2(a.dot(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basis_keeps_y_and_is_orthonormal() {
        let y = Vec3::new(0.3, -0.9, 0.1).normalize();
        let q = basis_from_y(y, Vec3::X);
        assert!((q * Vec3::Y - y).length() < 1e-4);
        assert!((q * Vec3::X).dot(y).abs() < 1e-4);
    }

    #[test]
    fn signed_angle_is_right_handed() {
        // Rotating -Y about +X by +90° gives -Z.
        let a = signed_angle(Vec3::NEG_Y, Vec3::NEG_Z, Vec3::X);
        assert!((a - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "{a}");
    }

    #[test]
    fn spherical_basis_maps_avian_swing_axis_to_bone_dir() {
        let dir = Vec3::new(0.2, -0.95, 0.24).normalize();
        let q = spherical_joint_basis(dir, Vec3::X);
        // Avian's default `SphericalJoint::twist_axis` is `Vec3::Y`, whose
        // `any_orthonormal_vector()` is `Vec3::X` (the cone axis!).
        let avian_swing_axis = Vec3::Y.any_orthonormal_vector();
        assert!((q * avian_swing_axis - dir).length() < 1e-4, "cone axis mismatch");
        assert!((q * Vec3::Y).dot(dir).abs() < 1e-4, "twist reference not orthogonal");
    }

    #[test]
    fn yaw_between_180_stays_upright() {
        // Opposite horizontal headings must rotate 180° around Y, never flipping up to -Y.
        let q = yaw_between(Vec3::Z, Vec3::NEG_Z, Vec3::Y);
        assert!((q * Vec3::Y - Vec3::Y).length() < 1e-5, "up flipped: {:?}", q * Vec3::Y);
        assert!((q * Vec3::Z - Vec3::NEG_Z).length() < 1e-5, "fwd wrong: {:?}", q * Vec3::Z);
    }
}
