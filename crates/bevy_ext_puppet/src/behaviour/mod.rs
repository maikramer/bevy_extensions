//! The behaviour network: Euphoria-style reactive motor programs.
//!
//! A behaviour is a small controller that, every physics step, reads what the
//! puppet senses ([`BehaviourCtx`]) and *requests* things from the body
//! ([`PuppetRequests`]): a target orientation for some joints, a strength
//! scale for a limb, pin weights, virtual forces, gaze targets, and signals.
//! Many behaviours run at once (balance the legs, brace a hand on a wall,
//! clutch a wound, turn the head toward a bumper, windmill the arms); the
//! [`junction`](resolve_requests) resolves the requests per joint — the
//! highest priority wins, equal priorities blend by weight — into the drive
//! the muscles execute. Joints nobody asks for track the animation.

use std::any::Any;

use bevy::prelude::*;

use crate::neuro::PuppetNeuroState;
use crate::part::{Limb, PART_COUNT, PuppetPart, Side};
use crate::pose::{PuppetTargets, shortest};
use crate::rig::{PuppetConfig, PuppetRig};
use crate::sense::PuppetSense;
use crate::state::PuppetState;

mod balance;
mod brace;
mod bump;
mod fall;
mod getup;
mod reactions;
mod wound;

pub use balance::{BodyBalance, StaggerFall};
pub use brace::WallBrace;
pub use bump::BumpReaction;
pub use fall::{BodyWrithe, CatchFall, Relax};
pub use getup::PhysicalGetUp;
pub use reactions::{ArmsWindmill, HitKind, HitReaction};
pub use wound::ReachForWound;

/// Conventional priorities (higher wins a joint).
pub mod priority {
    pub const ANIMATION: i32 = 0;
    pub const BALANCE: i32 = 10;
    pub const GAZE: i32 = 15;
    pub const REACTION: i32 = 20;
    pub const PROTECT: i32 = 30;
    pub const OVERRIDE: i32 = 100;
}

/// What a joint should aim for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JointTarget {
    /// The animated pose.
    Animation,
    /// A rotation of the child relative to its parent part.
    Local(Quat),
    /// A world rotation of the child (converted with the parent's current rotation).
    World(Quat),
    /// Hold the current relative rotation (pure damping).
    Current,
}

/// One behaviour's request for one joint.
#[derive(Clone, Copy, Debug)]
pub struct JointRequest {
    pub target: JointTarget,
    /// Stiffness multiplier of the joint's muscle (1 = nominal).
    pub stiffness: f32,
    /// Damping multiplier (1 = nominal damping ratio).
    pub damping: f32,
    pub priority: i32,
    pub weight: f32,
}

/// Signals behaviours raise for the state machine and event pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PuppetSignal {
    /// Balance is lost: fall.
    LostBalance,
    /// Balance is recovered: the animation can take over.
    Recovered,
    /// A recovery step landed (`side`, approximate foot world position).
    StepTaken { side: Side, point: Vec3 },
    /// A swing foot caught an obstacle mid-stride.
    Tripped { side: Side },
    /// High tipping momentum calls for [`ArmsWindmill`].
    NeedWindmill,
    /// Hands contacted a wall/obstacle while bracing.
    WallBraced {
        part: PuppetPart,
        point: Vec3,
        normal: Vec3,
    },
}

/// The muscle drive of one joint after the junction.
#[derive(Clone, Copy, Debug)]
pub struct JointDrive {
    /// Target rotation of the child relative to the parent.
    pub target: Quat,
    pub stiffness: f32,
    pub damping: f32,
    /// `false` when the target is "hold current" (only damping acts).
    pub spring: bool,
}

impl Default for JointDrive {
    fn default() -> Self {
        Self {
            target: Quat::IDENTITY,
            stiffness: 1.0,
            damping: 1.0,
            spring: true,
        }
    }
}

/// Limb chain transmitting a Jacobian-transpose virtual wrench from a surface contact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VirtualChain {
    /// Stance leg chain (`Foot → Calf → Thigh`).
    Leg(Side),
    /// Bracing / push-off arm chain (`Hand → LowerArm → UpperArm`).
    Arm(Side),
}

/// A Jacobian-transpose virtual force/torque transmitted through a contacting
/// leg or bracing arm (Pratt et al. 2001; Coros et al., SIGGRAPH 2010 §3.3;
/// Geijtenbeek et al., SCA 2012 §3.2.4). Converted into internal,
/// momentum-conserving torque pairs along the limb chain subject to surface
/// friction cones and anatomical joint torque ceilings, then folded implicitly
/// into the XPBD [`crate::drive::MuscleJoint`] targets.
#[derive(Clone, Copy, Debug)]
pub struct VirtualForce {
    /// Limb chain transmitting the virtual wrench from the contacting surface.
    pub chain: VirtualChain,
    /// Desired virtual force (N, world) applied at `application`.
    pub force: Vec3,
    /// Desired virtual torque (N·m, world) on the proximal trunk/pelvis.
    pub torque: Vec3,
    /// World point where `force` acts (typically hip, shoulder, or CoM).
    pub application: Vec3,
    /// Additional scale on the distal joint (ankle or wrist) `[0, 1]`.
    pub distal_scale: f32,
}

impl VirtualForce {
    /// Convenience constructor for a stance-leg virtual force.
    pub fn leg(
        stance: Side,
        force: Vec3,
        torque: Vec3,
        application: Vec3,
        ankle_scale: f32,
    ) -> Self {
        Self {
            chain: VirtualChain::Leg(stance),
            force,
            torque,
            application,
            distal_scale: ankle_scale,
        }
    }

    /// Convenience constructor for a bracing/push-off arm virtual force.
    pub fn arm(
        side: Side,
        force: Vec3,
        torque: Vec3,
        application: Vec3,
        wrist_scale: f32,
    ) -> Self {
        Self {
            chain: VirtualChain::Arm(side),
            force,
            torque,
            application,
            distal_scale: wrist_scale,
        }
    }
}

/// Requests accumulated during one physics step, and the resolved drive.
#[derive(Component, Debug)]
pub struct PuppetRequests {
    pub joints: [Vec<JointRequest>; PART_COUNT],
    /// Strength multiplier per part (behaviours combine by `min`).
    pub strength: [f32; PART_COUNT],
    /// Pin weight per part requested by behaviours (combined by `max` with the state's).
    pub pins: [f32; PART_COUNT],
    /// Cap on the final pin weight per part (combined by `min`).
    pub pin_scale: [f32; PART_COUNT],
    /// Physics weight per part requested by behaviours (combined by `max`).
    pub physics_weight: [f32; PART_COUNT],
    /// Internal torque pairs: `+t` on the first part, `-t` on the second.
    pub torque_pairs: Vec<(PuppetPart, PuppetPart, Vec3)>,
    /// External torques (not paired): balance assistance.
    pub external_torques: Vec<(PuppetPart, Vec3)>,
    /// Virtual forces mapped through stance-leg or bracing-arm Jacobian transpose (`Jᵀ F`).
    pub virtual_forces: Vec<VirtualForce>,
    /// Feet to keep planted (left, right): held flush against the ground slope
    /// with at most the torque the sole could transmit.
    pub plant: [bool; 2],
    /// Highest-priority gaze request emitted this step: `(world_point, priority, duration)`.
    pub gaze: Option<(Vec3, i32, f32)>,
    pub signals: Vec<PuppetSignal>,
    /// Resolved drive per joint (index = child part).
    pub drive: [JointDrive; PART_COUNT],
}

impl Default for PuppetRequests {
    fn default() -> Self {
        Self {
            joints: Default::default(),
            strength: [1.0; PART_COUNT],
            pins: [0.0; PART_COUNT],
            pin_scale: [1.0; PART_COUNT],
            physics_weight: [0.0; PART_COUNT],
            torque_pairs: Vec::new(),
            external_torques: Vec::new(),
            virtual_forces: Vec::new(),
            plant: [false; 2],
            gaze: None,
            signals: Vec::new(),
            drive: [JointDrive::default(); PART_COUNT],
        }
    }
}

impl PuppetRequests {
    pub fn clear(&mut self) {
        for j in &mut self.joints {
            j.clear();
        }
        self.strength = [1.0; PART_COUNT];
        self.pins = [0.0; PART_COUNT];
        self.pin_scale = [1.0; PART_COUNT];
        self.physics_weight = [0.0; PART_COUNT];
        self.torque_pairs.clear();
        self.external_torques.clear();
        self.virtual_forces.clear();
        self.plant = [false; 2];
        self.gaze = None;
        self.signals.clear();
    }

    /// Request a target for the joint of `part` (its link to its parent).
    pub fn joint(&mut self, part: PuppetPart, target: JointTarget, stiffness: f32, priority: i32) {
        self.joint_weighted(part, target, stiffness, 1.0, priority, 1.0);
    }

    pub fn joint_weighted(
        &mut self,
        part: PuppetPart,
        target: JointTarget,
        stiffness: f32,
        damping: f32,
        priority: i32,
        weight: f32,
    ) {
        if part.parent().is_none() || weight <= 0.0 {
            return;
        }
        self.joints[part.index()].push(JointRequest {
            target,
            stiffness,
            damping,
            priority,
            weight,
        });
    }

    /// Same target and stiffness for every joint of a limb.
    pub fn limb(&mut self, limb: Limb, target: JointTarget, stiffness: f32, priority: i32) {
        for p in limb.parts() {
            self.joint(*p, target, stiffness, priority);
        }
    }

    /// Scale the strength of a single part (combined by `min`).
    pub fn weaken_part(&mut self, part: PuppetPart, factor: f32) {
        let s = &mut self.strength[part.index()];
        *s = s.min(factor);
    }

    /// Scale the strength of a limb (combined by `min`).
    pub fn weaken(&mut self, limb: Limb, factor: f32) {
        for p in limb.parts() {
            self.weaken_part(*p, factor);
        }
    }

    pub fn pin(&mut self, part: PuppetPart, weight: f32) {
        let p = &mut self.pins[part.index()];
        *p = p.max(weight);
    }

    /// Cap the pin of a single part (combined by `min`).
    pub fn limit_pin(&mut self, part: PuppetPart, cap: f32) {
        let c = &mut self.pin_scale[part.index()];
        *c = c.min(cap);
    }

    /// Cap the pins of a limb (combined by `min`), e.g. to let it flop.
    pub fn limit_pins(&mut self, limb: Limb, cap: f32) {
        for p in limb.parts() {
            self.limit_pin(*p, cap);
        }
    }

    pub fn show_physics_part(&mut self, part: PuppetPart, weight: f32) {
        let w = &mut self.physics_weight[part.index()];
        *w = w.max(weight);
    }

    pub fn show_physics(&mut self, limb: Limb, weight: f32) {
        for p in limb.parts() {
            self.show_physics_part(*p, weight);
        }
    }

    /// Request a gaze look-at target (highest priority wins).
    pub fn look_at(&mut self, point: Vec3, priority: i32, duration: f32) {
        if !point.is_finite() || duration <= 0.0 {
            return;
        }
        let replace = match self.gaze {
            None => true,
            Some((_, p, _)) => priority >= p,
        };
        if replace {
            self.gaze = Some((point, priority, duration));
        }
    }

    pub fn signal(&mut self, s: PuppetSignal) {
        if !self.signals.contains(&s) {
            self.signals.push(s);
        }
    }
}

/// Everything a behaviour may read.
pub struct BehaviourCtx<'a> {
    pub sense: &'a PuppetSense,
    pub neuro: &'a PuppetNeuroState,
    pub rig: &'a PuppetRig,
    pub targets: &'a PuppetTargets,
    pub config: &'a PuppetConfig,
    pub state: &'a PuppetState,
    pub dt: f32,
}

impl BehaviourCtx<'_> {
    /// World rotation of the part now.
    pub fn rot(&self, part: PuppetPart) -> Quat {
        self.sense.body(part).rot
    }
    pub fn pos(&self, part: PuppetPart) -> Vec3 {
        self.sense.body(part).pos
    }
    /// World end of the part's segment now.
    pub fn end(&self, part: PuppetPart) -> Vec3 {
        let b = self.sense.body(part);
        b.pos + b.rot * self.rig.part(part).end_local
    }
    /// Unit lateral axis pointing to the puppet's right.
    pub fn right(&self) -> Vec3 {
        self.sense.heading.cross(self.sense.up).normalize_or(Vec3::X)
    }
}

/// Whether a behaviour keeps running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BehaviourStatus {
    Running,
    Done,
}

/// A reactive motor program.
pub trait Behaviour: Send + Sync + 'static {
    /// Unique name; starting a behaviour replaces a running one with the same name.
    fn name(&self) -> &'static str;
    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus;
    /// Optional downcast hook for in-place stacking (e.g. consecutive hits or bumps).
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        None
    }
}

/// The behaviours running on a puppet.
#[derive(Component, Default)]
pub struct PuppetBehaviours {
    pub active: Vec<Box<dyn Behaviour>>,
}

impl PuppetBehaviours {
    pub fn start(&mut self, b: Box<dyn Behaviour>) {
        let name = b.name();
        self.active.retain(|a| a.name() != name);
        self.active.push(b);
    }
    pub fn start_if_absent(&mut self, b: Box<dyn Behaviour>) {
        if !self.has(b.name()) {
            self.active.push(b);
        }
    }
    pub fn stop(&mut self, name: &str) {
        self.active.retain(|a| a.name() != name);
    }
    pub fn has(&self, name: &str) -> bool {
        self.active.iter().any(|a| a.name() == name)
    }
    pub fn get_mut<T: Behaviour>(&mut self, name: &str) -> Option<&mut T> {
        self.active
            .iter_mut()
            .find(|a| a.name() == name)
            .and_then(|a| a.as_any_mut())
            .and_then(|any| any.downcast_mut::<T>())
    }
    pub fn clear(&mut self) {
        self.active.clear();
    }
    pub fn names(&self) -> Vec<&'static str> {
        self.active.iter().map(|a| a.name()).collect()
    }
}

/// Runs the behaviours and resolves their requests.
#[allow(clippy::type_complexity)]
pub fn run_behaviours(
    mut puppets: Query<(
        &mut PuppetBehaviours,
        &mut PuppetRequests,
        &mut PuppetNeuroState,
        &PuppetSense,
        &PuppetRig,
        &PuppetTargets,
        &PuppetConfig,
        &PuppetState,
    )>,
) {
    for (mut behaviours, mut requests, mut neuro, sense, rig, targets, config, state) in &mut puppets {
        requests.clear();
        {
            let ctx = BehaviourCtx {
                sense,
                neuro: &neuro,
                rig,
                targets,
                config,
                state,
                dt: sense.dt,
            };
            behaviours
                .active
                .retain_mut(|b| b.update(&ctx, &mut requests) == BehaviourStatus::Running);
        }
        if let Some((pt, prio, dur)) = requests.gaze {
            neuro.set_gaze(pt, prio, dur, sense.time);
        }
        resolve_requests(&mut requests, sense, targets);
    }
}

/// The junction: per joint, the highest-priority requests win and blend by weight.
pub fn resolve_requests(req: &mut PuppetRequests, sense: &PuppetSense, targets: &PuppetTargets) {
    for part in PuppetPart::ALL {
        let Some(parent) = part.parent() else { continue };
        let i = part.index();
        let parent_rot = sense.body(parent).rot;
        let child_rot = sense.body(part).rot;
        let to_local = |t: JointTarget| -> Option<Quat> {
            match t {
                JointTarget::Animation => Some(targets.local[i]),
                JointTarget::Local(q) => Some(q),
                JointTarget::World(q) => Some((parent_rot.inverse() * q).normalize()),
                JointTarget::Current => None,
            }
        };
        let list = &req.joints[i];
        let drive = if list.is_empty() {
            JointDrive {
                target: targets.local[i],
                ..default()
            }
        } else {
            let top = list.iter().map(|r| r.priority).max().unwrap_or(0);
            let mut acc = Vec4::ZERO;
            let mut first: Option<Quat> = None;
            let (mut w_sum, mut stiff, mut damp, mut spring_w) = (0.0, 0.0, 0.0, 0.0);
            for r in list.iter().filter(|r| r.priority == top) {
                w_sum += r.weight;
                stiff += r.stiffness * r.weight;
                damp += r.damping * r.weight;
                if let Some(q) = to_local(r.target) {
                    let q = shortest(q);
                    let q = match first {
                        Some(f) if f.dot(q) < 0.0 => -q,
                        Some(_) => q,
                        None => {
                            first = Some(q);
                            q
                        }
                    };
                    acc += Vec4::from(q) * r.weight;
                    spring_w += r.weight;
                }
            }
            let w_sum = w_sum.max(1e-6);
            let target = if spring_w > 0.0 {
                Quat::from_vec4(acc).normalize()
            } else {
                (parent_rot.inverse() * child_rot).normalize()
            };
            JointDrive {
                target,
                stiffness: stiff / w_sum,
                damping: damp / w_sum,
                spring: spring_w > 0.0,
            }
        };
        req.drive[i] = drive;
    }
}
