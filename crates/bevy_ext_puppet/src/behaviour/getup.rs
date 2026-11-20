//! Biomechanical multi-stage physical get-up ([`PhysicalGetUp`]).
//!
//! Replaces rigid world-space pin levitation with a 3-stage physical motor
//! program that works whether or not the game supplies a `getup_front`/`getup_back`
//! animation clip:
//!
//! 1. **Head Righting & Limb Gather (`u = 0.00..0.28`)**: vestibular head lift
//!    first; hands plant beside shoulders (prone) or beside hips (supine) using
//!    twist-locked arm IK (`solve_arm_ik`); knees flex to draw feet beneath pelvis.
//! 2. **Push-Up / Sit-Up & Lunge Tuck (`u = 0.28..0.64`)**: arms extend pushing
//!    the chest off the ground (assisted by implicit arm [`VirtualForce`]),
//!    spine rises toward vertical, and the lead leg plants forward under the CoM.
//! 3. **Squat Rise & Handoff (`u = 0.64..1.00`)**: both legs extend smoothly
//!    into standing posture as world pins complete the handoff to animation.

use bevy::prelude::*;

use super::{
    Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, VirtualForce, priority,
};
use crate::ik::{align_palm, level, smootherstep, solve_arm_ik};
use crate::part::{PuppetPart, Side};

/// Multi-stage physical get-up from supine (`face_up = true`) or prone (`face_up = false`).
#[derive(Clone, Debug)]
pub struct PhysicalGetUp {
    pub face_up: bool,
    pub duration: f32,
    t: f32,
}

impl PhysicalGetUp {
    pub const NAME: &'static str = "physical_getup";

    pub fn new(face_up: bool, duration: f32) -> Self {
        Self {
            face_up,
            duration: duration.max(0.4),
            t: 0.0,
        }
    }
}

impl Behaviour for PhysicalGetUp {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        self.t += ctx.dt;
        let u = (self.t / self.duration).clamp(0.0, 1.0);
        if u >= 1.0 {
            return BehaviourStatus::Done;
        }

        let s = ctx.sense;
        let rig = ctx.rig;
        let h = rig.height;
        let up = s.up;
        let ground_n = s.ground_normal;
        let fwd = s.heading;
        let right = ctx.right();
        let pelvis_pos = ctx.pos(PuppetPart::Pelvis);
        let chest_pos = ctx.pos(PuppetPart::SpineUpper);

        // Pin shaping: suppress world-space levitation early so physical arms/legs
        // actually drive the initial push-up, then ramp smoothly into full rise!
        let pin_cap = if u < 0.28 {
            0.08 + 0.16 * smootherstep(u / 0.28)
        } else if u < 0.64 {
            0.24 + 0.44 * smootherstep((u - 0.28) / 0.36)
        } else {
            0.68 + 0.32 * smootherstep((u - 0.64) / 0.36)
        };
        for part in PuppetPart::ALL {
            // Let feet and pelvis guide gently earlier; keep hands/arms free to push off floor!
            let cap = if matches!(
                part,
                PuppetPart::UpperArmL
                    | PuppetPart::LowerArmL
                    | PuppetPart::HandL
                    | PuppetPart::UpperArmR
                    | PuppetPart::LowerArmR
                    | PuppetPart::HandR
            ) && u < 0.68
            {
                (pin_cap * 0.35).min(0.25)
            } else {
                pin_cap
            };
            out.limit_pin(part, cap);
        }

        // ---- 1. Labyrinthine Head Righting (runs from frame 0!) -------------
        let head = PuppetPart::Head;
        let leveled_head = level(ctx.rot(head), rig.part(head).up_local, up);
        let head_w = 1.0 - smootherstep((u - 0.70) / 0.30);
        if head_w > 0.05 {
            out.joint_weighted(head, JointTarget::World(leveled_head), 2.2, 1.15, priority::BALANCE, head_w);
        }

        // ---- 2. Stage 1 & 2: Hand Plant & Push-Up / Sit-Up ------------------
        if u < 0.72 {
            let push_env = smootherstep(u / 0.18) * (1.0 - smootherstep((u - 0.56) / 0.16));
            let chest_anchor = s.flat(chest_pos);
            let pelvis_anchor = s.flat(pelvis_pos);

            for side in [Side::Left, Side::Right] {
                let sign = if side == Side::Right { 1.0 } else { -1.0 };
                let lateral = right * sign;
                let upper = PuppetPart::upper_arm(side);
                let lower = PuppetPart::lower_arm(side);
                let hand = PuppetPart::hand(side);
                let urig = rig.part(upper);
                let lrig = rig.part(lower);
                let hrig = rig.part(hand);
                let shoulder = ctx.pos(upper);

                // Prone: plant palms beside chest/shoulders and push up;
                // Supine: plant hands beside hips/lower ribs to prop torso upright!
                let base_flat = if self.face_up {
                    pelvis_anchor.lerp(chest_anchor, 0.35)
                } else {
                    chest_anchor
                };
                let spread_offset = if self.face_up {
                    lateral * (0.24 * h) - fwd * (0.08 * h)
                } else {
                    lateral * (0.22 * h) + fwd * (0.06 * h)
                };
                let mut hand_target = s.lift(base_flat + s.flat(spread_offset), s.ground + 0.02);
                let max_reach = (urig.length + lrig.length) * 0.94;
                let from_sh = hand_target - shoulder;
                if from_sh.length() > max_reach {
                    hand_target = shoulder + from_sh.normalize() * max_reach;
                }

                let elbow_pole = if self.face_up {
                    (-fwd * 0.55 + lateral * 0.70 + up * 0.30).normalize()
                } else {
                    (-fwd * 0.45 + lateral * 0.65 + up * 0.50).normalize()
                };
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
                let hr = align_palm(ctx.rot(hand), hrig.axis_local, hrig.up_local, ground_n, fwd);
                out.joint_weighted(upper, JointTarget::World(ur), 2.4, 1.25, priority::BALANCE, push_env.max(0.1));
                out.joint_weighted(
                    lower,
                    JointTarget::Local((ur.inverse() * lr).normalize()),
                    2.6,
                    1.25,
                    priority::BALANCE,
                    push_env.max(0.1),
                );
                out.joint_weighted(hand, JointTarget::World(hr), 1.4, 1.2, priority::BALANCE, push_env.max(0.1));

                // Stage 2: arm push-off VirtualForce lifting chest off the ground!
                if u >= 0.22 && (s.touching(hand) || s.touching(lower)) {
                    let lift_ramp = smootherstep((u - 0.22) / 0.20) * (1.0 - smootherstep((u - 0.54) / 0.16));
                    let mg = s.total_mass * s.g.max(9.81);
                    out.virtual_forces.push(VirtualForce::arm(
                        side,
                        ground_n * (0.28 * mg * lift_ramp),
                        Vec3::ZERO,
                        shoulder,
                        0.45,
                    ));
                }
            }
        }

        // ---- 3. Spine Uncurl & Squat/Lunge Leg Tuck -------------------------
        let pelvis_rot = ctx.rot(PuppetPart::Pelvis);
        let pitch_axis = (pelvis_rot.inverse() * right).normalize_or(Vec3::X);

        if u < 0.76 {
            let tuck_env = smootherstep(u / 0.22) * (1.0 - smootherstep((u - 0.55) / 0.21));
            // Left leg tucks deeper into a lunge plant; right leg trails slightly for asymmetric realism.
            for (side, hip_flex, knee_flex) in [
                (Side::Left, 1.05 * tuck_env, 1.45 * tuck_env),
                (Side::Right, 0.70 * tuck_env, 1.15 * tuck_env),
            ] {
                let thigh = PuppetPart::thigh(side);
                let calf = PuppetPart::calf(side);
                let tq = Quat::from_axis_angle(pitch_axis, -hip_flex) * ctx.targets.local[thigh.index()];
                let calf_rig = rig.part(calf);
                let hinge = calf_rig.axis_local.cross(-calf_rig.forward_local).normalize_or(Vec3::NEG_X);
                let cq = Quat::from_axis_angle(hinge, knee_flex) * ctx.targets.local[calf.index()];
                out.joint_weighted(thigh, JointTarget::Local(tq.normalize()), 1.8, 1.2, priority::BALANCE, tuck_env.max(0.1));
                out.joint_weighted(calf, JointTarget::Local(cq.normalize()), 1.8, 1.2, priority::BALANCE, tuck_env.max(0.1));
            }
            // During Stage 2, actively extend trunk toward upright!
            if u >= 0.25 {
                let spine_env = smootherstep((u - 0.25) / 0.25) * (1.0 - smootherstep((u - 0.65) / 0.18));
                if spine_env > 0.05 {
                    for part in [PuppetPart::SpineLower, PuppetPart::SpineUpper] {
                        let leveled = level(ctx.rot(part), rig.part(part).up_local, up);
                        out.joint_weighted(part, JointTarget::World(leveled), 1.8, 1.2, priority::BALANCE, spine_env);
                    }
                }
            }
        } else {
            // Stage 3: plant grounded feet and stand smoothly into the animation pose.
            for side in [Side::Left, Side::Right] {
                if s.foot_down(side) {
                    out.plant[side.index()] = true;
                }
            }
        }

        BehaviourStatus::Running
    }
}
