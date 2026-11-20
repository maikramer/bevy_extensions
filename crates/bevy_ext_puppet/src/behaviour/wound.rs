//! Injury clutching reflex ([`ReachForWound`], Euphoria `PointGun`/`Shot`
//! reach-for-wound).
//!
//! After a significant non-lethal hit to the chest, abdomen, shoulder, head, or
//! thigh, the nearest free hand (or both hands on heavy gut/chest blows) reaches
//! accurately toward the struck bone's world-space impact point using
//! twist-locked arm IK (`solve_arm_ik`). Runs just below `priority::REACTION`
//! so fresh hit recoil, [`super::WallBrace`], and [`super::CatchFall`] take
//! immediate precedence whenever survival requires the arms!

use bevy::prelude::*;

use super::{Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, priority};
use crate::ik::{smootherstep, solve_arm_ik};
use crate::part::{BodyZone, PuppetPart, Side};

/// Reaches the free hand(s) toward a struck wound location while staggering or down.
#[derive(Clone, Debug)]
pub struct ReachForWound {
    pub part: PuppetPart,
    /// Target offset in `part`'s local body frame.
    pub local_point: Vec3,
    /// Severity in `[0, 1]`.
    pub severity: f32,
    pub duration: f32,
    /// Brief delay before the hand starts clutching (lets initial hit flinch play first).
    pub onset_delay: f32,
    t: f32,
}

impl ReachForWound {
    pub const NAME: &'static str = "reach_for_wound";

    pub fn new(part: PuppetPart, local_point: Vec3, severity: f32) -> Self {
        let severity = severity.clamp(0.0, 1.0);
        Self {
            part,
            local_point,
            severity,
            duration: 1.15 + 0.75 * severity,
            onset_delay: 0.14,
            t: 0.0,
        }
    }

    /// Which hand(s) should clutch `self.part`.
    fn clutching_sides(&self, ctx: &BehaviourCtx, world_target: Vec3) -> (bool, bool) {
        match self.part.zone() {
            BodyZone::Arm(Side::Left) => (false, true), // right hand clutches left arm/shoulder
            BodyZone::Arm(Side::Right) => (true, false), // left hand clutches right arm/shoulder
            BodyZone::Leg(side) => (side == Side::Left, side == Side::Right),
            BodyZone::Abdomen => (true, self.severity > 0.45),
            BodyZone::Chest => {
                if self.severity > 0.65 {
                    (true, true)
                } else {
                    let dl = ctx.pos(PuppetPart::UpperArmL).distance_squared(world_target);
                    let dr = ctx.pos(PuppetPart::UpperArmR).distance_squared(world_target);
                    if dl <= dr { (true, false) } else { (false, true) }
                }
            }
            BodyZone::Head => {
                let dl = ctx.pos(PuppetPart::HandL).distance_squared(world_target);
                let dr = ctx.pos(PuppetPart::HandR).distance_squared(world_target);
                if dl <= dr { (true, false) } else { (false, true) }
            }
            BodyZone::Pelvis => (false, true),
        }
    }
}

impl Behaviour for ReachForWound {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        self.t += ctx.dt;
        if self.t >= self.duration {
            return BehaviourStatus::Done;
        }
        if self.t < self.onset_delay {
            return BehaviourStatus::Running;
        }

        let active_dur = (self.duration - self.onset_delay).max(0.1);
        let u = ((self.t - self.onset_delay) / active_dur).clamp(0.0, 1.0);
        let env = smootherstep(u / 0.22) * (1.0 - smootherstep((u - 0.68) / 0.32));
        if env <= 0.02 {
            return BehaviourStatus::Running;
        }

        let s = ctx.sense;
        let rig = ctx.rig;
        let h = rig.height;
        let up = s.up;
        let fwd = s.heading;
        let right = ctx.right();

        // Compute world target anchored to the moving struck bone, offset slightly
        // outside the capsule surface so the hand sits cleanly on top of the chest/shoulder.
        let body = s.body(self.part);
        let base_pt = body.pos + body.rot * self.local_point;
        let surface_offset = match self.part.zone() {
            BodyZone::Chest | BodyZone::Abdomen => fwd * (0.09 * h),
            BodyZone::Head => fwd * (0.07 * h) + up * (0.03 * h),
            BodyZone::Arm(side) => {
                let sign = if side == Side::Right { 1.0 } else { -1.0 };
                fwd * (0.05 * h) + right * (sign * 0.03 * h)
            }
            BodyZone::Leg(side) => {
                let sign = if side == Side::Right { 1.0 } else { -1.0 };
                fwd * (0.05 * h) + right * (sign * 0.05 * h)
            }
            BodyZone::Pelvis => fwd * (0.08 * h),
        };
        let wound_world = base_pt + surface_offset;

        let (use_l, use_r) = self.clutching_sides(ctx, wound_world);
        let prio = priority::REACTION - 2;
        let weight = (0.85 * env).clamp(0.05, 0.92);

        for (side, active) in [(Side::Left, use_l), (Side::Right, use_r)] {
            if !active {
                continue;
            }
            let sign = if side == Side::Right { 1.0 } else { -1.0 };
            let upper = PuppetPart::upper_arm(side);
            let lower = PuppetPart::lower_arm(side);
            let urig = rig.part(upper);
            let lrig = rig.part(lower);
            let shoulder = ctx.pos(upper);

            // Spread slightly when both hands clutch the same central wound.
            let hand_target = if use_l && use_r {
                wound_world + right * (sign * 0.05 * h)
            } else {
                wound_world
            };

            // Natural elbow pole: down and outward from torso.
            let elbow_pole = (-up * 0.65 + right * (sign * 0.65) - fwd * 0.30).normalize();
            let (ur, lr, _) = solve_arm_ik(
                shoulder,
                hand_target,
                urig.length,
                lrig.length,
                elbow_pole,
                urig.axis_local,
                urig.forward_local,
                lrig.axis_local,
                lrig.forward_local,
                ctx.rot(upper),
                ctx.rot(lower),
            );
            out.joint_weighted(upper, JointTarget::World(ur), 2.4, 1.15, prio, weight);
            out.joint_weighted(
                lower,
                JointTarget::Local((ur.inverse() * lr).normalize()),
                2.6,
                1.20,
                prio,
                weight,
            );
            // Release animation pins on the clutching arm during partial-hit mode so
            // the hand genuinely reaches across to the wound even while staying upright.
            let pin_keep = (1.0 - env * 0.92).clamp(0.08, 1.0);
            for p in [upper, lower, PuppetPart::hand(side)] {
                out.pin_scale[p.index()] = out.pin_scale[p.index()].min(pin_keep);
            }
        }

        // Subtle protective hunch toward the wound + brief downward glance.
        if matches!(self.part.zone(), BodyZone::Chest | BodyZone::Abdomen) {
            let pitch_axis = up.cross(fwd).normalize_or(right);
            let hunch = 0.20 * self.severity * env;
            let anim_chest = ctx.rot(PuppetPart::SpineLower) * ctx.targets.local[PuppetPart::SpineUpper.index()];
            let goal = (Quat::from_axis_angle(pitch_axis, hunch) * anim_chest).normalize();
            out.joint_weighted(PuppetPart::SpineUpper, JointTarget::World(goal), 2.0, 1.0, prio, 0.7 * env);
        }
        if u < 0.45 {
            out.look_at(wound_world, priority::GAZE - 1, 0.35);
        }

        BehaviourStatus::Running
    }
}
