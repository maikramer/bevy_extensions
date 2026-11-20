//! Falling, ground restlessness, and death collapse:
//!
//! * [`CatchFall`] — multi-phase ballistic fall protection (Euphoria / Ha & Liu
//!   2012):
//!   1. **Airborne Flail** (high drop / free-fall): momentum-righting arm
//!      windmilling, reflexive leg cycling, and downward landing gaze.
//!   2. **Protective Tuck** (high-speed launch or rapid tumble): hands shield
//!      head/face, spine curls into a ball, knees draw up to roll cleanly.
//!   3. **Directional Fall-Break** (forward, sideways, backward): twist-locked
//!      arm IK (`solve_arm_ik`), palm surface alignment (`align_palm`),
//!      eccentric push-up absorption with implicit **arm [`VirtualForce`]**,
//!      labyrinthine head righting (lateral tilt away from floor on side falls,
//!      strong chin tuck on back falls, face turn on forward falls).
//!   4. **Post-Impact Recoil**: brief stunned softening on heavy torso/pelvis slams.
//! * [`BodyWrithe`] — realistic ground breathing, wound-biased clutching/curl,
//!   asymmetric leg draw-up, and slow head rolling.
//! * [`Relax`] — progressive anatomical neuromuscular death collapse (legs
//!   buckle first, trunk slumps second, arms/head trail last) settling smoothly
//!   under passive viscoelastic soft-tissue damping.

use bevy::prelude::*;

use super::{
    Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, VirtualForce, priority,
};
use crate::ik::{aim, align_palm, level, smooth, solve_arm_ik};
use crate::part::{BodyZone, Limb, PuppetPart, Side};

/// Multi-phase ballistic fall-breaking and free-fall protection (`CatchFall`).
#[derive(Clone, Debug, Default)]
pub struct CatchFall {
    t: f32,
    hands_down: f32,
    flail_phase: f32,
    recoil_timer: f32,
    impacted_torso: bool,
}

impl CatchFall {
    pub const NAME: &'static str = "catch_fall";
}

impl Behaviour for CatchFall {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        let s = ctx.sense;
        let rig = ctx.rig;
        let h = rig.height;
        let dt = ctx.dt;
        self.t += dt;
        let up = s.up;
        let right = ctx.right();
        let fwd = s.heading;

        // Detect heavy torso/pelvis/head ground slams for brief post-impact recoil.
        if !self.impacted_torso
            && let Some(part) = s.peak_impact_part
            && matches!(
                part,
                PuppetPart::Pelvis | PuppetPart::SpineLower | PuppetPart::SpineUpper | PuppetPart::Head
            )
            && s.peak_impact_impulse > 26.0
        {
            self.impacted_torso = true;
            self.recoil_timer = 0.20;
        }
        if self.recoil_timer > 0.0 {
            self.recoil_timer = (self.recoil_timer - dt).max(0.0);
            // Stunned wind-knocked-out settling: briefly soften limbs while keeping neck safe.
            for limb in [Limb::LeftArm, Limb::RightArm, Limb::LeftLeg, Limb::RightLeg] {
                out.limb(limb, JointTarget::Current, 0.35, priority::PROTECT);
            }
            out.joint(PuppetPart::SpineLower, JointTarget::Current, 0.45, priority::PROTECT);
            out.joint(PuppetPart::SpineUpper, JointTarget::Current, 0.45, priority::PROTECT);
            tuck_chin(ctx, out, -0.50, 2.2);
            return BehaviourStatus::Running;
        }

        // ---- Mode 1: High Free-Fall (Airborne Flail & Righting Reflex) ------
        if s.is_high_fall(h) {
            self.flail_phase += std::f32::consts::TAU * 2.4 * dt;
            let l_h = s.angular_momentum.reject_from(up);
            let spin_sense = if l_h.dot(right) >= 0.0 { 1.0 } else { -1.0 };

            // Panicked momentum-righting arm windmill with semi-bent twist-locked elbows.
            for side in [Side::Left, Side::Right] {
                let sign = if side == Side::Right { 1.0 } else { -1.0 };
                let phase = self.flail_phase * spin_sense
                    + if side == Side::Right { std::f32::consts::PI } else { 0.0 };
                let upper = PuppetPart::upper_arm(side);
                let lower = PuppetPart::lower_arm(side);
                let urig = rig.part(upper);
                let lrig = rig.part(lower);
                let shoulder = ctx.pos(upper);
                let arm_dir = (fwd * phase.sin() * 0.72 + up * (phase.cos() * 0.65 + 0.25)
                    + right * (sign * 0.75))
                    .normalize();
                let reach = (urig.length + lrig.length) * 0.88;
                let target = shoulder + arm_dir * reach;
                let pole = (-fwd * 0.35 + right * (sign * 0.65) - up * 0.45).normalize();
                let (ur, lr, _) = solve_arm_ik(
                    shoulder,
                    target,
                    urig.length,
                    lrig.length,
                    pole,
                    urig.axis_local,
                    urig.forward_local,
                    lrig.axis_local,
                    lrig.forward_local,
                    ctx.rot(upper),
                    ctx.rot(lower),
                );
                out.joint_weighted(upper, JointTarget::World(ur), 2.5, 0.95, priority::PROTECT, 1.0);
                out.joint_weighted(
                    lower,
                    JointTarget::Local((ur.inverse() * lr).normalize()),
                    2.2,
                    1.0,
                    priority::PROTECT,
                    1.0,
                );
            }

            // Reflexive leg cycling / bicycling while trying to find ground!
            let cycle_phase = self.flail_phase * 1.15;
            for side in [Side::Left, Side::Right] {
                let offset = if side == Side::Right { std::f32::consts::PI } else { 0.0 };
                let c = (cycle_phase + offset).sin();
                let thigh = PuppetPart::thigh(side);
                let calf = PuppetPart::calf(side);
                let hip_pitch = 0.45 + 0.42 * c; // forward bicycle kick
                let knee_bend = 0.75 + 0.45 * (-c).max(0.0);
                let hip_axis = (ctx.rot(PuppetPart::Pelvis).inverse() * right).normalize_or(Vec3::X);
                let thigh_q = Quat::from_axis_angle(hip_axis, -hip_pitch) * ctx.targets.local[thigh.index()];
                let calf_rig = rig.part(calf);
                let hinge_local = calf_rig.axis_local.cross(-calf_rig.forward_local).normalize_or(Vec3::NEG_X);
                let calf_q = Quat::from_axis_angle(hinge_local, knee_bend) * ctx.targets.local[calf.index()];
                out.joint(thigh, JointTarget::Local(thigh_q.normalize()), 1.4, priority::PROTECT);
                out.joint(calf, JointTarget::Local(calf_q.normalize()), 1.4, priority::PROTECT);
            }

            // Look down toward the approaching landing spot!
            let ground_pt = s.lift(s.flat(s.com), s.ground);
            out.look_at(ground_pt, priority::PROTECT - 1, 0.25);
            tuck_chin(ctx, out, -0.35, 2.0);
            out.joint(PuppetPart::SpineLower, JointTarget::Animation, 0.9, priority::PROTECT);
            out.joint(PuppetPart::SpineUpper, JointTarget::Animation, 0.9, priority::PROTECT);
            return BehaviourStatus::Running;
        }

        // ---- Classify fall velocity, spin, and orientation ------------------
        let v_h = s.com_vel.reject_from(up);
        let h_speed = v_h.length();
        let ang_speed = s.centroidal_ang_vel.length();
        let chest_pos = ctx.pos(PuppetPart::SpineUpper);
        let pelvis_pos = ctx.pos(PuppetPart::Pelvis);
        let trunk_up = (chest_pos - pelvis_pos).normalize_or(up);
        let inverted = trunk_up.dot(up) < -0.35;

        // ---- Mode 2: Protective Tuck / Roll (high-speed launch or airborne tumble) ---
        if (h_speed > 5.0 || ang_speed > 5.2 || (inverted && (!s.body_touching() || ang_speed > 3.0)))
            && self.t < 1.8
        {
            let head_pos = ctx.pos(PuppetPart::Head);
            // Shield temples & forehead with forearms, elbows tucked tightly to ribs!
            for side in [Side::Left, Side::Right] {
                let sign = if side == Side::Right { 1.0 } else { -1.0 };
                let upper = PuppetPart::upper_arm(side);
                let lower = PuppetPart::lower_arm(side);
                let urig = rig.part(upper);
                let lrig = rig.part(lower);
                let target = head_pos + fwd * (0.12 * h) + right * (sign * 0.08 * h) + up * (0.04 * h);
                let pole = (fwd * 0.35 + right * (sign * 0.55) - up * 0.65).normalize();
                let (ur, lr, _) = solve_arm_ik(
                    ctx.pos(upper),
                    target,
                    urig.length,
                    lrig.length,
                    pole,
                    urig.axis_local,
                    urig.forward_local,
                    lrig.axis_local,
                    lrig.forward_local,
                    ctx.rot(upper),
                    ctx.rot(lower),
                );
                out.joint_weighted(upper, JointTarget::World(ur), 2.8, 1.4, priority::PROTECT, 1.0);
                out.joint_weighted(
                    lower,
                    JointTarget::Local((ur.inverse() * lr).normalize()),
                    3.0,
                    1.5,
                    priority::PROTECT,
                    1.0,
                );
            }
            // Tight protective ball curl (chin tucked, spine rounded, knees pulled in).
            tuck_chin(ctx, out, -0.78, 3.0);
            curl_spine(ctx, out, -0.38, 1.5);
            tuck_legs(ctx, out, 0.95, 1.35, 1.1);
            return BehaviourStatus::Running;
        }

        // ---- Mode 3: Directional Fall-Breaking (Forward / Sideways / Back) --
        let fall = if h_speed > 0.25 {
            v_h / h_speed
        } else {
            (chest_pos - pelvis_pos).reject_from(up).normalize_or(fwd)
        };
        let fwd_dot = fall.dot(fwd);
        let side_dot = fall.dot(right); // > 0 falling to right, < 0 falling to left
        let backward = fwd_dot < -0.30;
        let sideways = !backward && fwd_dot < 0.35 && side_dot.abs() > 0.45;

        let hands_touch = s.touching(PuppetPart::HandL)
            || s.touching(PuppetPart::HandR)
            || s.touching(PuppetPart::LowerArmL)
            || s.touching(PuppetPart::LowerArmR);
        if hands_touch {
            self.hands_down += dt;
        }
        // Firm reach before impact; upon contact, eccentric compliant push-up
        // absorption (taper stiffness, raise damping) so elbows soak energy smoothly,
        // then relax gently onto the ground once velocity is absorbed.
        let (arm_k, arm_d) = if self.hands_down > 0.0 {
            let relax = (1.0 - ((self.hands_down - 0.55) / 0.85).clamp(0.0, 1.0)).max(0.15);
            (((3.2 - 5.2 * self.hands_down).max(0.85)) * relax, 1.80)
        } else {
            (3.2, 1.05)
        };

        let ground_n = s.ground_normal;
        let t_imp = s.predicted_impact_time.clamp(0.05, 0.48);
        let lead = (v_h * (t_imp * 0.65)).clamp_length_max(0.45 * h);
        let anchor_flat = s.flat(s.com + lead * 0.55);

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

            // Sideways fall: upside arm counter-flares / shields ribs instead of awkwardly crossing!
            let is_upside_arm = sideways && (side_dot * sign < -0.25);
            if is_upside_arm {
                let shield_target = chest_pos + fwd * (0.16 * h) - lateral * (0.04 * h) - up * (0.05 * h);
                let pole = (lateral * 0.70 - fwd * 0.35 + up * 0.35).normalize();
                let (ur, lr, _) = solve_arm_ik(
                    shoulder,
                    shield_target,
                    urig.length,
                    lrig.length,
                    pole,
                    urig.axis_local,
                    urig.forward_local,
                    lrig.axis_local,
                    lrig.forward_local,
                    ctx.rot(upper),
                    ctx.rot(lower),
                );
                out.joint_weighted(upper, JointTarget::World(ur), 2.0, 1.2, priority::PROTECT, 0.85);
                out.joint_weighted(
                    lower,
                    JointTarget::Local((ur.inverse() * lr).normalize()),
                    2.0,
                    1.2,
                    priority::PROTECT,
                    0.85,
                );
                continue;
            }

            let side_bias = fall.dot(lateral).max(0.0);
            let reach = if backward {
                // Backward break-fall (ukemi): arms reach OUTWARD and slightly back,
                // never straight behind the spine where shoulders dislocate!
                fall * (0.18 * h) + lateral * (0.24 * h)
            } else if sideways {
                // Downside arm reaches out & slightly forward to cushion shoulder/ribs.
                fall * (0.26 * h) + fwd * (0.12 * h) + lateral * (0.14 * h)
            } else {
                // Forward fall: hands slightly wider than shoulders ahead of chest.
                fall * (0.29 * h) + lateral * ((0.12 + 0.12 * side_bias) * h)
            };

            let mut hand_target = s.lift(anchor_flat + s.flat(reach), s.ground + 0.02);
            // Keep elbows slightly pre-bent (~92% reach) so arms never lock rigid at impact.
            let max_reach = (urig.length + lrig.length) * 0.93;
            let from_sh = hand_target - shoulder;
            if from_sh.length() > max_reach {
                hand_target = shoulder + from_sh.normalize() * max_reach;
            }

            // Twist-locked arm IK: elbow tip bows backward + outward + slightly up!
            let elbow_pole = if backward {
                (fwd * 0.45 + lateral * 0.75 + up * 0.25).normalize()
            } else {
                (-fall * 0.55 + lateral * 0.70 + up * 0.30).normalize()
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
            let finger_dir = if backward { lateral } else { fall };
            let hr = align_palm(ctx.rot(hand), hrig.axis_local, hrig.up_local, ground_n, finger_dir);
            out.joint_weighted(upper, JointTarget::World(ur), arm_k, arm_d, priority::PROTECT, 1.0);
            out.joint_weighted(
                lower,
                JointTarget::Local((ur.inverse() * lr).normalize()),
                arm_k,
                arm_d,
                priority::PROTECT,
                1.0,
            );
            out.joint_weighted(hand, JointTarget::World(hr), 1.4, 1.3, priority::PROTECT, 1.0);

            // Upon hand/forearm ground contact while STILL descending, emit pure
            // velocity-braking arm VirtualForce along ground normal (zero static
            // push-up force once settled so the character rests calmly on the floor!).
            let arm_touch = s.touching(hand) || s.touching(lower);
            let v_down = (-s.com_vel.dot(ground_n)).max(0.0);
            if arm_touch && !backward && v_down > 0.35 && self.hands_down < 0.55 {
                let mg = s.total_mass * s.g.max(9.81);
                let brace_f = (0.14 * (v_down - 0.35) * mg).clamp(0.0, 0.32 * mg);
                out.virtual_forces.push(VirtualForce::arm(
                    side,
                    ground_n * brace_f,
                    Vec3::ZERO,
                    shoulder,
                    0.40,
                ));
            }
        }

        // ---- Head & Spine Protection (direction-specific!) ------------------
        let settle_fade = (1.0 - ((self.t - 1.1) / 0.8).clamp(0.0, 1.0)).max(0.15);
        if backward {
            // Strong chin-to-chest tuck protects the occiput; rounded spine rolls cleanly.
            tuck_chin(ctx, out, -0.75 * settle_fade, 3.0 * settle_fade);
            curl_spine(ctx, out, -0.30 * settle_fade, 1.0 * settle_fade);
            // Flex hips & knees slightly while airborne, then relax softly once any foot/body touches!
            if self.t < 0.9 && s.feet_down() == 0 && !s.body_touching() {
                tuck_legs(ctx, out, 0.50, 0.72, 0.45);
            } else {
                out.limb(Limb::LeftLeg, JointTarget::Current, 0.15, priority::PROTECT);
                out.limb(Limb::RightLeg, JointTarget::Current, 0.15, priority::PROTECT);
            }
        } else if sideways {
            // Labyrinthine head righting: tilt head AWAY from the approaching ground!
            let chest_rot = ctx.rot(PuppetPart::SpineUpper);
            let anim_head = chest_rot * ctx.targets.local[PuppetPart::Head.index()];
            let tilt_away = if side_dot > 0.0 { 0.55 } else { -0.55 };
            let roll_q = Quat::from_axis_angle(fwd, tilt_away * settle_fade);
            let chin_q = Quat::from_axis_angle(right, -0.22 * settle_fade);
            let goal = (roll_q * chin_q * anim_head).normalize();
            out.joint(PuppetPart::Head, JointTarget::World(goal), 2.6 * settle_fade, priority::PROTECT);
            out.joint(PuppetPart::SpineLower, JointTarget::Animation, 0.70 * settle_fade, priority::PROTECT);
            out.joint(PuppetPart::SpineUpper, JointTarget::Animation, 0.70 * settle_fade, priority::PROTECT);
            if self.t < 0.9 && s.feet_down() == 0 && !s.body_touching() {
                tuck_legs(ctx, out, 0.35, 0.55, 0.35);
            } else {
                out.limb(Limb::LeftLeg, JointTarget::Current, 0.15, priority::PROTECT);
                out.limb(Limb::RightLeg, JointTarget::Current, 0.15, priority::PROTECT);
            }
        } else {
            // Forward fall: slight head turn aside + chin lift protects nose/teeth from floor!
            let chest_rot = ctx.rot(PuppetPart::SpineUpper);
            let anim_head = chest_rot * ctx.targets.local[PuppetPart::Head.index()];
            let turn_sign = if side_dot >= 0.0 { -1.0 } else { 1.0 };
            let turn_q = Quat::from_axis_angle(up, turn_sign * 0.48 * settle_fade);
            let lift_q = Quat::from_axis_angle(right, 0.22 * settle_fade);
            let goal = (turn_q * lift_q * anim_head).normalize();
            out.joint(PuppetPart::Head, JointTarget::World(goal), 2.5 * settle_fade, priority::PROTECT);
            out.joint(PuppetPart::SpineLower, JointTarget::Animation, 0.80 * settle_fade, priority::PROTECT);
            out.joint(PuppetPart::SpineUpper, JointTarget::Animation, 0.80 * settle_fade, priority::PROTECT);
            out.limb(Limb::LeftLeg, JointTarget::Animation, 0.25 * settle_fade, priority::PROTECT);
            out.limb(Limb::RightLeg, JointTarget::Animation, 0.25 * settle_fade, priority::PROTECT);
        }

        BehaviourStatus::Running
    }
}

fn tuck_chin(ctx: &BehaviourCtx, out: &mut PuppetRequests, angle: f32, stiffness: f32) {
    let chest_rot = ctx.rot(PuppetPart::SpineUpper);
    let axis_local = (chest_rot.inverse() * ctx.right()).normalize_or(Vec3::X);
    let head_local = Quat::from_axis_angle(axis_local, angle) * ctx.targets.local[PuppetPart::Head.index()];
    out.joint(PuppetPart::Head, JointTarget::Local(head_local.normalize()), stiffness, priority::PROTECT);
}

fn curl_spine(ctx: &BehaviourCtx, out: &mut PuppetRequests, angle: f32, stiffness: f32) {
    let right = ctx.right();
    for part in [PuppetPart::SpineLower, PuppetPart::SpineUpper] {
        let parent_rot = ctx.rot(part.parent().unwrap_or(PuppetPart::Pelvis));
        let a = (parent_rot.inverse() * right).normalize_or(Vec3::X);
        let q = Quat::from_axis_angle(a, angle) * ctx.targets.local[part.index()];
        out.joint(part, JointTarget::Local(q.normalize()), stiffness, priority::PROTECT);
    }
}

fn tuck_legs(ctx: &BehaviourCtx, out: &mut PuppetRequests, hip_flex: f32, knee_flex: f32, stiffness: f32) {
    let right = ctx.right();
    let pelvis_rot = ctx.rot(PuppetPart::Pelvis);
    let hip_axis = (pelvis_rot.inverse() * right).normalize_or(Vec3::X);
    for side in [Side::Left, Side::Right] {
        let thigh = PuppetPart::thigh(side);
        let calf = PuppetPart::calf(side);
        // Right-handed `+right` rotates downward femur (`-up`) toward `forward` (`+right × -up = forward`).
        let thigh_q = Quat::from_axis_angle(hip_axis, hip_flex) * ctx.targets.local[thigh.index()];
        let calf_rig = ctx.rig.part(calf);
        let hinge_local = calf_rig.axis_local.cross(-calf_rig.forward_local).normalize_or(Vec3::NEG_X);
        let calf_q = Quat::from_axis_angle(hinge_local, knee_flex) * ctx.targets.local[calf.index()];
        out.joint(thigh, JointTarget::Local(thigh_q.normalize()), stiffness, priority::PROTECT);
        out.joint(calf, JointTarget::Local(calf_q.normalize()), stiffness, priority::PROTECT);
    }
}

/// Organic ground restlessness (`BodyWrithe`): realistic slow breathing cycle,
/// wound-biased protective curl, asymmetric knee draw-up, and slow head rolling.
#[derive(Clone, Debug)]
pub struct BodyWrithe {
    pub amplitude: f32,
    pub frequency: f32,
    pub stiffness: f32,
    t: f32,
}

impl Default for BodyWrithe {
    fn default() -> Self {
        Self {
            amplitude: 0.35,
            frequency: 0.36,
            stiffness: 0.35,
            t: 0.0,
        }
    }
}

impl BodyWrithe {
    pub const NAME: &'static str = "body_writhe";
}

impl Behaviour for BodyWrithe {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        self.t += ctx.dt;
        let fade = smooth(self.t / 0.7);
        let s = ctx.sense;
        let rig = ctx.rig;
        let right = ctx.right();
        let up = s.up;
        let fwd = s.heading;

        // 1. Breathing cycle + gentle fetal spine curl (stronger if wounded in chest/gut).
        let breath = (std::f32::consts::TAU * self.frequency * self.t).sin();
        let groan = (std::f32::consts::TAU * self.frequency * 0.55 * self.t + 0.9).sin();
        let curl_bias = if ctx.neuro.wound.is_some_and(|w| matches!(w.part.zone(), BodyZone::Abdomen | BodyZone::Chest)) {
            0.34
        } else {
            0.16
        };
        let pelvis_rot = ctx.rot(PuppetPart::Pelvis);
        let spine_pitch = (pelvis_rot.inverse() * right).normalize_or(Vec3::X);
        let lower_q = Quat::from_axis_angle(spine_pitch, -(curl_bias + 0.10 * breath) * fade)
            * ctx.targets.local[PuppetPart::SpineLower.index()];
        let upper_q = Quat::from_axis_angle(spine_pitch, -(curl_bias * 1.1 + 0.14 * breath) * fade)
            * ctx.targets.local[PuppetPart::SpineUpper.index()];
        out.joint(PuppetPart::SpineLower, JointTarget::Local(lower_q.normalize()), self.stiffness * 1.2, priority::BALANCE);
        out.joint(PuppetPart::SpineUpper, JointTarget::Local(upper_q.normalize()), self.stiffness * 1.2, priority::BALANCE);

        // 2. Slow head roll & slight groaning lift.
        let head_yaw = 0.42 * self.amplitude * groan * fade;
        let head_pitch = -0.18 * (0.5 + 0.5 * breath) * fade;
        let chest_rot = ctx.rot(PuppetPart::SpineUpper);
        let head_q = Quat::from_axis_angle((chest_rot.inverse() * up).normalize_or(Vec3::Y), head_yaw)
            * Quat::from_axis_angle((chest_rot.inverse() * right).normalize_or(Vec3::X), head_pitch)
            * ctx.targets.local[PuppetPart::Head.index()];
        out.joint(PuppetPart::Head, JointTarget::Local(head_q.normalize()), self.stiffness * 1.3, priority::BALANCE);

        // 3. Asymmetric legs: draw one knee softly up while the other rests looser.
        let knee_l = (0.85 + 0.25 * breath) * fade;
        let knee_r = (0.35 + 0.20 * groan) * fade;
        for (side, hip_f, knee_f) in [(Side::Left, 0.52, knee_l), (Side::Right, 0.22, knee_r)] {
            let thigh = PuppetPart::thigh(side);
            let calf = PuppetPart::calf(side);
            let tq = Quat::from_axis_angle(spine_pitch, -hip_f * fade) * ctx.targets.local[thigh.index()];
            let calf_rig = rig.part(calf);
            let hinge = calf_rig.axis_local.cross(-calf_rig.forward_local).normalize_or(Vec3::NEG_X);
            let cq = Quat::from_axis_angle(hinge, knee_f) * ctx.targets.local[calf.index()];
            out.joint(thigh, JointTarget::Local(tq.normalize()), self.stiffness, priority::BALANCE);
            out.joint(calf, JointTarget::Local(cq.normalize()), self.stiffness, priority::BALANCE);
        }

        // 4. Arms: clutch sore area gently or draw loosely toward chest/side.
        let chest = ctx.pos(PuppetPart::SpineUpper);
        let wound_pt = ctx.neuro.wound.map(|w| {
            let b = s.body(w.part);
            b.pos + b.rot * w.local_point + fwd * (0.06 * rig.height)
        });
        for side in [Side::Left, Side::Right] {
            let sign = if side == Side::Right { 1.0 } else { -1.0 };
            let upper = PuppetPart::upper_arm(side);
            let lower = PuppetPart::lower_arm(side);
            if side == Side::Right
                && let Some(wp) = wound_pt
            {
                let urig = rig.part(upper);
                let lrig = rig.part(lower);
                let pole = (right * 0.65 - up * 0.55 - fwd * 0.3).normalize();
                let (ur, lr, _) = solve_arm_ik(
                    ctx.pos(upper),
                    wp,
                    urig.length,
                    lrig.length,
                    pole,
                    urig.axis_local,
                    urig.forward_local,
                    lrig.axis_local,
                    lrig.forward_local,
                    ctx.rot(upper),
                    ctx.rot(lower),
                );
                out.joint(upper, JointTarget::World(ur), self.stiffness * 1.4, priority::BALANCE);
                out.joint(lower, JointTarget::Local((ur.inverse() * lr).normalize()), self.stiffness * 1.4, priority::BALANCE);
            } else {
                let rest_dir = (right * (sign * (0.55 + 0.18 * groan))
                    + fwd * (0.35 + 0.15 * breath)
                    - up * 0.45)
                    .normalize();
                let ur = aim(ctx.rot(upper), rig.part(upper).axis_local, rest_dir);
                let lr_dir = (chest + fwd * (0.14 * rig.height) - ctx.pos(lower)).normalize_or(rest_dir);
                let lr = aim(ctx.rot(lower), rig.part(lower).axis_local, lr_dir);
                out.joint(upper, JointTarget::World(ur), self.stiffness, priority::BALANCE);
                out.joint(lower, JointTarget::World(lr), self.stiffness, priority::BALANCE);
            }
        }
        BehaviourStatus::Running
    }
}

/// Progressive anatomical neuromuscular collapse on death/knock-out (`Relax`).
///
/// Instead of snapping instantaneously to a weightless zero-tone doll on frame 0,
/// muscle tone fades asynchronously over `collapse_time` (default 0.30 s):
/// 1. Legs buckle immediately (0–30 % of collapse) so the body crumples
///    downward organically rather than toppling stiffly like a plank.
/// 2. Spine slumps next (0–65 %).
/// 3. Arms and head trail last (0–100 %), settling naturally under passive
///    soft-tissue damping (`config.passive_joint_damping`).
///
/// Set `collapse_time = 0.0` for instant limp ragdoll.
#[derive(Clone, Debug)]
pub struct Relax {
    pub tone: f32,
    pub collapse_time: f32,
    t: f32,
}

impl Default for Relax {
    fn default() -> Self {
        Self {
            tone: 0.025,
            collapse_time: 0.30,
            t: 0.0,
        }
    }
}

impl Relax {
    pub const NAME: &'static str = "relax";

    pub fn with_collapse(collapse_time: f32) -> Self {
        Self {
            collapse_time: collapse_time.max(0.0),
            ..default()
        }
    }

    pub fn instant() -> Self {
        Self {
            collapse_time: 0.0,
            ..default()
        }
    }
}

impl Behaviour for Relax {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        self.t += ctx.dt;
        let ct = self.collapse_time;
        for part in PuppetPart::ALL {
            if part.parent().is_none() {
                continue;
            }
            if ct <= 1e-4 || self.t >= ct {
                out.joint_weighted(
                    part,
                    JointTarget::Current,
                    self.tone,
                    0.50,
                    priority::OVERRIDE,
                    1.0,
                );
                continue;
            }
            // Asynchronous anatomical tone decay: legs fail first → knees buckle!
            let (decay_window, peak_tone) = match part.zone() {
                BodyZone::Leg(_) => (0.28 * ct, 0.18),
                BodyZone::Abdomen | BodyZone::Chest => (0.65 * ct, 0.40),
                BodyZone::Head | BodyZone::Arm(_) => (ct, 0.35),
                BodyZone::Pelvis => (ct, self.tone),
            };
            let u = (self.t / decay_window.max(1e-3)).clamp(0.0, 1.0);
            let tone = peak_tone * (1.0 - smooth(u)) + self.tone * smooth(u);

            // Slight involuntary knee buckle during the first 100 ms of dying collapse.
            if matches!(part, PuppetPart::CalfL | PuppetPart::CalfR) && u < 0.85 {
                let calf_rig = ctx.rig.part(part);
                let hinge = calf_rig.axis_local.cross(-calf_rig.forward_local).normalize_or(Vec3::NEG_X);
                let buckle = Quat::from_axis_angle(hinge, 0.55 * (1.0 - u)) * ctx.targets.local[part.index()];
                out.joint_weighted(part, JointTarget::Local(buckle.normalize()), tone, 0.55, priority::OVERRIDE, 1.0);
            } else if matches!(part, PuppetPart::SpineUpper) && u < 0.9 {
                let parent_rot = ctx.rot(PuppetPart::SpineLower);
                let pitch = (parent_rot.inverse() * ctx.right()).normalize_or(Vec3::X);
                let slump = Quat::from_axis_angle(pitch, -0.22 * (1.0 - u)) * ctx.targets.local[part.index()];
                out.joint_weighted(part, JointTarget::Local(slump.normalize()), tone, 0.55, priority::OVERRIDE, 1.0);
            } else {
                let target = if u < 0.5 { JointTarget::Current } else { JointTarget::Animation };
                out.joint_weighted(part, target, tone, 0.55, priority::OVERRIDE, 1.0);
            }
        }
        // Keep level-grounded feet unplanted once dead.
        let _ = level;
        BehaviourStatus::Running
    }
}
