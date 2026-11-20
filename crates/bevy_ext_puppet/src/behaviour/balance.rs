//! Standing balance: SIMBICON trunk control, Jacobian-transpose virtual force
//! control (`Jᵀ F`, Coros et al. 2010 / Geijtenbeek et al. 2012), centroidal
//! angular-momentum damping (Macchietto et al. 2009), Ankle → Flywheel Hip/Arm
//! → IPM Stepping continuum (Pratt et al. 2006), terrain-adaptive step landing
//! on slopes/ramps, heel-strike/toe-off foot articulation, mid-swing obstacle
//! trip detection, and gaze-integrated vestibular head stabilization.

use bevy::prelude::*;

use super::{
    Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, PuppetSignal,
    VirtualForce, priority,
};
use crate::ik::{aim, aim_with_forward, level, smooth, two_bone_mid};
use crate::part::{Limb, PuppetPart, Side};

#[derive(Clone, Copy, Debug)]
enum Phase {
    Stand,
    Swing {
        side: Side,
        t: f32,
        duration: f32,
        from: Vec3,
        to: Vec3,
    },
    /// Brief double-support landing absorption after footstrike so virtual
    /// forces damp residual momentum before deciding on another step.
    Settle {
        t: f32,
        duration: f32,
    },
}

/// Keep the body upright, absorbing pushes with virtual forces, angular
/// momentum regulation, flywheel upper-body/arm counterweight, and
/// terrain-adaptive recovery steps (Euphoria's `BodyBalance`).
#[derive(Clone, Debug)]
pub struct BodyBalance {
    /// Muscle strength of legs and trunk (1 = nominal).
    pub strength: f32,
    /// Capture-point distance outside the support polygon that triggers a
    /// step, as a fraction of the height. Below it the planted feet, virtual
    /// forces, hips and arms absorb the push.
    pub step_threshold: f32,
    /// Steps allowed before giving up (`None` = the config's).
    pub max_steps: Option<u32>,
    /// Nominal duration of a step (s, scaled with rig height and CoM speed).
    pub step_time: f32,
    /// Lean of the trunk target (world axis-angle), for staggering.
    pub lean: Vec3,
    /// Raise `LostBalance` when failing (otherwise just keep trying).
    pub give_up: bool,
    /// Strength of Jacobian-transpose virtual force balance control (1 = nominal, 0 = off).
    pub virtual_force_gain: f32,
    /// Strength of centroidal angular-momentum damping (1 = nominal, 0 = off).
    pub momentum_gain: f32,
    phase: Phase,
    steps: u32,
    calm: f32,
    airborne: f32,
    unbalanced: f32,
    trip_boost: f32,
}

impl Default for BodyBalance {
    fn default() -> Self {
        Self {
            strength: 1.0,
            step_threshold: 0.055,
            max_steps: None,
            step_time: 0.24,
            lean: Vec3::ZERO,
            give_up: true,
            virtual_force_gain: 1.0,
            momentum_gain: 1.0,
            phase: Phase::Stand,
            steps: 0,
            calm: 0.0,
            airborne: 0.0,
            unbalanced: 0.0,
            trip_boost: 0.0,
        }
    }
}

impl BodyBalance {
    pub const NAME: &'static str = "body_balance";

    /// Steps taken so far.
    pub fn steps(&self) -> u32 {
        self.steps
    }

    pub fn is_stepping(&self) -> bool {
        matches!(self.phase, Phase::Swing { .. })
    }

    /// Geometrically reachable pelvis height given the current foot spread and
    /// landing shock absorption crouch.
    fn desired_pelvis_height(&self, ctx: &BehaviourCtx) -> f32 {
        let s = ctx.sense;
        let rig = ctx.rig;
        let base = rig.pelvis_height * 0.985;
        let l_leg = rig.part(PuppetPart::ThighL).length + rig.part(PuppetPart::CalfL).length;
        let hip_to_pelvis = ((ctx.pos(PuppetPart::Pelvis) - ctx.pos(PuppetPart::ThighL))
            .dot(s.up)
            .max(0.0)
            + (ctx.pos(PuppetPart::Pelvis) - ctx.pos(PuppetPart::ThighR))
                .dot(s.up)
                .max(0.0))
            * 0.5;
        if s.feet_down() < 2 {
            // During a recovery step, lower the stance hip just enough for the
            // reaching swing leg to physically touch the floor at `to`!
            if let Phase::Swing { side, t, duration, to, .. } = self.phase {
                let u = (t / duration.max(1e-3)).clamp(0.0, 1.0);
                let swing_d = (s.flat(to) - s.flat(ctx.pos(PuppetPart::thigh(side)))).length();
                let reach_h = (l_leg * l_leg - (swing_d * 0.95).powi(2))
                    .max(0.30 * l_leg * l_leg)
                    .sqrt();
                let step_cap = (reach_h + rig.ankle_height + hip_to_pelvis) * 0.97;
                let blend = smooth((u - 0.25) / 0.50);
                let target = (base * 0.97).lerp(step_cap.min(base * 0.97), blend);
                return target.max(0.76 * rig.pelvis_height);
            }
            return base * 0.97;
        }
        // Exact per-leg horizontal offset from each hip to its planted foot
        // (works accurately for both lateral straddles AND front-to-back splits!).
        let d_l = (s.flat(ctx.pos(PuppetPart::FootL)) - s.flat(ctx.pos(PuppetPart::ThighL))).length();
        let d_r = (s.flat(ctx.pos(PuppetPart::FootR)) - s.flat(ctx.pos(PuppetPart::ThighR))).length();
        let max_hip_off = d_l.max(d_r);
        let max_leg_h = (l_leg * l_leg - max_hip_off * max_hip_off)
            .max(0.25 * l_leg * l_leg)
            .sqrt();
        // Keep ~3 cm knee-bend margin below straight-leg lock so rising never
        // pulls the trailing foot off the floor in a wide split!
        let geom_cap = (max_leg_h + rig.ankle_height + hip_to_pelvis) * 0.94;
        // Subtle athletic knee-bend shock absorption when absorbing fast stumbles.
        let absorb_dip = if matches!(self.phase, Phase::Settle { .. }) {
            (s.horizontal_speed() * 0.014).clamp(0.0, 0.022) * rig.height
        } else {
            0.0
        };
        // On a slope where one foot lands higher than `s.ground`, lift the target
        // pelvis height smoothly by the mean foot elevation above `s.ground`.
        let slope_lift = if s.feet_down() == 2 {
            let fl = ctx.pos(PuppetPart::FootL).dot(s.up) - (s.ground + rig.ankle_height);
            let fr = ctx.pos(PuppetPart::FootR).dot(s.up) - (s.ground + rig.ankle_height);
            ((fl.max(0.0) + fr.max(0.0)) * 0.5).clamp(0.0, 0.25 * rig.height)
        } else {
            0.0
        };
        let target = (base + slope_lift - absorb_dip).min(geom_cap + slope_lift).max(0.68 * rig.pelvis_height);
        let max_rise = if matches!(self.phase, Phase::Settle { .. }) { 0.015 } else { 0.03 };
        target.min(s.pelvis_height + max_rise)
    }

    /// Adaptive step duration: faster push speeds or post-trip hops demand quicker swing.
    fn adaptive_step_duration(&self, ctx: &BehaviourCtx) -> f32 {
        let h = ctx.rig.height;
        let speed = ctx.sense.horizontal_speed();
        let trip_fast = if self.trip_boost > 0.0 { 0.82 } else { 1.0 };
        let scale = (1.0 - 0.16 * (speed - 0.6).max(0.0)).clamp(0.68, 1.0) * trip_fast;
        self.step_time * (h / 1.8).sqrt() * scale
    }

    fn step_target(&self, ctx: &BehaviourCtx, swing: Side, t_rem: f32) -> Vec3 {
        let s = ctx.sense;
        let rig = ctx.rig;
        let h = rig.height;
        let right = ctx.right();
        let stance = PuppetPart::foot(swing.opposite());
        let stance_flat = s.flat(ctx.pos(stance));
        let v_flat = s.flat(s.com_vel);
        let dir = (s.capture_point - s.support_centre).normalize_or_zero();
        // Constant-length IPM capture point plus a braking margin so the foot
        // lands past the capture point and reverses horizontal velocity cleanly.
        let mut to = s.capture_point + dir * (0.08 * h);
        // Keep the swing foot on its own side of the stance foot (avoid leg crossing).
        let right2 = s.flat(right).normalize_or(Vec2::X);
        let sign = if swing == Side::Right { 1.0 } else { -1.0 };
        let lateral = (to - stance_flat).dot(right2) * sign;
        let min_lat = 0.11 * h;
        if lateral < min_lat {
            to += right2 * sign * (min_lat - lateral);
        }
        // Cap the stride relative to the predicted landing hip position.
        let l_leg = rig.part(PuppetPart::thigh(swing)).length
            + rig.part(PuppetPart::calf(swing)).length;
        let hip_now = s.flat(ctx.pos(PuppetPart::thigh(swing)));
        let hip_pred = hip_now + v_flat * (t_rem.clamp(0.0, 0.25) * 0.45);
        let max_stride = (0.50 * l_leg).min(0.40 * h);
        let from_hip = to - hip_pred;
        if from_hip.length() > max_stride {
            to = hip_pred + from_hip.normalize() * max_stride;
        }

        // Terrain-adaptive landing elevation: use `ahead_ground` slope/ramp height
        // when probed near the step target, falling back to `s.ground`.
        let mut target_ground_h = s.ground;
        if let Some(ahead) = s.ahead_ground {
            let ahead_flat = s.flat(ahead.point);
            let ahead_h = ahead.point.dot(s.up);
            if (ahead_flat - to).length() < 0.65 * h && (ahead_h - s.ground).abs() < 0.42 * h {
                // Project along the probed slope plane from `ahead.point` to `to`.
                let n = ahead.normal;
                let n_up = n.dot(s.up).max(0.45);
                let delta_w = s.lift(to - ahead_flat, 0.0);
                let slope_dh = -delta_w.dot(n) / n_up;
                target_ground_h = (ahead_h + slope_dh).clamp(s.ground - 0.35 * h, s.ground + 0.35 * h);
            }
        }
        s.lift(to, target_ground_h + rig.ankle_height)
    }

    /// Trunk orientation the balance aims for: the animated pelvis, turned
    /// to the current heading and leaned by `lean`.
    fn pelvis_goal(&self, ctx: &BehaviourCtx) -> Quat {
        let s = ctx.sense;
        let anim = ctx.targets.rot(PuppetPart::Pelvis);
        let fwd_local = ctx.rig.part(PuppetPart::Pelvis).forward_local;
        let anim_heading = (anim * fwd_local).reject_from(s.up).normalize_or(s.heading);
        let yaw = crate::rig::yaw_between(anim_heading, s.heading, s.up);
        (Quat::from_scaled_axis(self.lean) * yaw * anim).normalize()
    }

    /// Jacobian-transpose virtual forces (`Jᵀ F`, Coros et al. 2010 §3.3;
    /// Geijtenbeek et al. 2012 §3.2.4) and centroidal angular-momentum damping
    /// (Macchietto et al. 2009) transmitted through the planted legs.
    fn emit_virtual_forces(
        &self,
        ctx: &BehaviourCtx,
        out: &mut PuppetRequests,
        swing_side: Option<Side>,
        want_pelvis_h: f32,
    ) {
        if self.virtual_force_gain <= 0.0 && self.momentum_gain <= 0.0 {
            return;
        }
        let s = ctx.sense;
        let stance_legs: Vec<Side> = [Side::Left, Side::Right]
            .into_iter()
            .filter(|side| Some(*side) != swing_side && s.foot_down(*side))
            .collect();
        if stance_legs.is_empty() {
            return;
        }

        let m = s.total_mass;
        let n = stance_legs.len() as f32;
        let up = s.up;
        let settling = matches!(self.phase, Phase::Settle { .. });
        let double_stance = stance_legs.len() == 2;

        // 1. Horizontal CoM velocity & position regulation.
        let settle_ramp = match self.phase {
            Phase::Settle { t, .. } => (t / 0.14).clamp(0.15, 1.0),
            _ => 1.0,
        };
        let com_flat = s.flat(s.com);
        let pos_err2 = s.support_centre - com_flat;
        let pos_err = s.lift(pos_err2, 0.0);
        let vel_h = s.com_vel.reject_from(up);
        let (kp_h, kd_h) = if double_stance {
            if settling {
                (4.0 * settle_ramp, 6.0 * settle_ramp)
            } else {
                (6.5, 7.0)
            }
        } else {
            (0.0, 1.5)
        };
        let f_horiz = (pos_err * kp_h - vel_h * kd_h) * m * self.virtual_force_gain * self.strength;

        // 2. Vertical height support.
        let h_err = (want_pelvis_h - s.pelvis_height).clamp(-0.08, 0.08);
        let v_y = s.com_vel.dot(up);
        let kp_y = if double_stance { 18.0 } else { 10.0 };
        let kd_y = if v_y > 0.0 { 6.0 } else { 2.5 };
        let f_vert = up * ((h_err * kp_y - v_y * kd_y) * m * self.virtual_force_gain * self.strength);

        // 3. Centroidal angular-momentum damping (pitch/roll only, preserves yaw).
        let l_horiz = s.angular_momentum.reject_from(up);
        let am_gain = if double_stance { 1.8 } else { 0.7 };
        let tau_am = -l_horiz * (am_gain * self.momentum_gain * self.strength);

        let total_force = f_horiz + f_vert;
        for side in stance_legs {
            let share = if double_stance {
                let d_self = (s.flat(ctx.pos(PuppetPart::foot(side))) - com_flat).length();
                let d_other =
                    (s.flat(ctx.pos(PuppetPart::foot(side.opposite()))) - com_flat).length();
                let sum = (d_self + d_other).max(1e-3);
                (d_other / sum).clamp(0.25, 0.75)
            } else {
                1.0 / n
            };
            let hip_pos = ctx.pos(PuppetPart::thigh(side));
            out.virtual_forces.push(VirtualForce::leg(
                side,
                total_force * share,
                tau_am * share,
                hip_pos,
                if double_stance { 0.85 } else { 0.55 },
            ));
        }
    }

    /// Flywheel upper-body & arm counterweight (Pratt et al. 2006) +
    /// gaze-integrated vestibular head stabilization (Euphoria `HeadLook`).
    fn upper_body_counterweight(&self, ctx: &BehaviourCtx, out: &mut PuppetRequests) {
        let s = ctx.sense;
        let rig = ctx.rig;
        let up = s.up;
        let right = ctx.right();
        let v_h = s.com_vel.reject_from(up);
        let speed = v_h.length();

        // 1. Hip/spine flywheel lunge into disturbance when capture margin is positive.
        if speed > 0.22 && s.capture_margin > -0.02 * rig.height && self.lean.length_squared() < 1e-4 {
            let push_dir = v_h / speed;
            let pitch_axis = up.cross(push_dir).normalize_or_zero();
            if pitch_axis.length_squared() > 0.5 {
                let lunge = ((speed - 0.22) * 0.24).clamp(0.0, 0.26) * self.strength;
                for (part, frac) in [
                    (PuppetPart::SpineLower, 0.45),
                    (PuppetPart::SpineUpper, 0.55),
                ] {
                    let parent = part.parent().expect("spine has parent");
                    let anim_world = ctx.rot(parent) * ctx.targets.local[part.index()];
                    let goal = (Quat::from_axis_angle(pitch_axis, lunge * frac) * anim_world).normalize();
                    out.joint(part, JointTarget::World(goal), self.strength * 0.9, priority::BALANCE);
                }
            }
        }

        // 2. Subtle counter-arm abduction flare during moderate balance recovery
        // (tightrope reflex at `priority::BALANCE`, seamlessly overridden by
        // `ArmsWindmill`, `HitReaction`, `ReachForWound`, or `WallBrace`!).
        let balance_urgency = ((speed - 0.30) / 0.90)
            .max((s.capture_margin + 0.01 * rig.height) / (0.12 * rig.height))
            .clamp(0.0, 1.0);
        if balance_urgency > 0.12 {
            let push_dir = if speed > 0.15 { v_h / speed } else { s.heading };
            let lat_bias = push_dir.dot(right);
            for side in [Side::Left, Side::Right] {
                let sign = if side == Side::Right { 1.0 } else { -1.0 };
                let upper = PuppetPart::upper_arm(side);
                let contra = (-lat_bias * sign).max(0.0);
                let abduct = (0.38 + 0.40 * balance_urgency + 0.35 * contra).min(0.95);
                let target_dir = (right * (sign * abduct) - push_dir * (0.20 * balance_urgency) - up * 0.65)
                    .normalize();
                let ur = aim(ctx.rot(upper), rig.part(upper).axis_local, target_dir);
                out.joint_weighted(
                    upper,
                    JointTarget::World(ur),
                    self.strength * 0.9,
                    1.05,
                    priority::BALANCE,
                    balance_urgency * 0.55,
                );
            }
        }

        // 3. Vestibular head stabilization + orienting gaze coordination!
        let head = PuppetPart::Head;
        let head_rot = ctx.rot(head);
        let head_up = rig.part(head).up_local;
        let leveled = level(head_rot, head_up, up);
        let anim_head = ctx.rot(PuppetPart::SpineUpper) * ctx.targets.local[head.index()];
        let mut head_goal = anim_head.slerp(leveled, 0.58).normalize();

        if let Some(gaze_pt) = ctx.neuro.active_gaze(s.time) {
            let to_gaze = gaze_pt - ctx.pos(head);
            let to_flat = to_gaze.reject_from(up);
            if to_flat.length_squared() > 0.04 {
                let yaw = crate::rig::signed_angle(s.heading, to_flat.normalize(), up).clamp(-0.95, 0.95);
                let pitch_raw = (to_gaze.dot(up) / to_flat.length().max(0.2)).atan().clamp(-0.45, 0.38);
                let gaze_q = Quat::from_axis_angle(up, yaw) * Quat::from_axis_angle(right, pitch_raw);
                head_goal = (gaze_q * head_goal).normalize();
            }
        }
        out.joint(head, JointTarget::World(head_goal), self.strength * 1.15, priority::BALANCE);
    }

    #[allow(clippy::too_many_arguments)]
    fn leg(
        &self,
        ctx: &BehaviourCtx,
        out: &mut PuppetRequests,
        side: Side,
        ankle: Vec3,
        swing_u: Option<f32>,
        swing_fwd_dot: f32,
        pelvis_goal: Quat,
        want_pelvis_h: f32,
    ) {
        let s = ctx.sense;
        let rig = ctx.rig;
        let thigh = PuppetPart::thigh(side);
        let calf = PuppetPart::calf(side);
        let foot = PuppetPart::foot(side);
        let k = self.strength;
        let swing = swing_u.is_some();

        let ik = || {
            let l1 = rig.part(thigh).length;
            let l2 = rig.part(calf).length;
            let mut hip = ctx.pos(thigh);
            if !swing {
                hip += s.up * (want_pelvis_h - s.pelvis_height).clamp(-0.04, 0.025);
                // In double stance, bias the target hip gently toward the support
                // centre and against horizontal velocity so rising from a deep
                // recovery crouch straightens cleanly over the base instead of
                // pushing the pelvis backward!
                if s.feet_down() == 2 {
                    let pos_err = s.lift(s.support_centre - s.flat(s.com), 0.0);
                    let vel_h = s.com_vel.reject_from(s.up);
                    let horiz_bias = (pos_err * 0.18 - vel_h * 0.055).clamp_length_max(0.035);
                    hip += horiz_bias;
                }
            }
            let pole = s.heading + s.up * 0.05;
            let knee = two_bone_mid(hip, ankle, l1, l2, pole);
            let thigh_r = rig.part(thigh);
            let calf_r = rig.part(calf);
            (
                aim_with_forward(ctx.rot(thigh), thigh_r.axis_local, thigh_r.forward_local, knee - hip, s.heading),
                aim_with_forward(ctx.rot(calf), calf_r.axis_local, calf_r.forward_local, ankle - knee, s.heading),
            )
        };

        let foot_r = rig.part(foot);
        let slope_up = if swing {
            s.ahead_ground.map_or(s.ground_normal, |g| g.normal)
        } else {
            s.foot_normals[side.index()]
        };
        let mut foot_rot = aim_with_forward(ctx.rot(foot), foot_r.up_local, foot_r.forward_local, slope_up, s.heading);

        if let Some(u) = swing_u {
            // Subtle mid-swing dorsiflexion (toes slightly UP for ground clearance
            // and heel-lead on forward steps), strictly 0 at liftoff (`u < 0.22`)
            // and touchdown (`u >= 0.80`) so the rigid sole never jams into the floor!
            let ankle_pitch = if u > 0.22 && u < 0.80 && swing_fwd_dot > 0.25 {
                let bell = (std::f32::consts::PI * ((u - 0.22) / 0.58)).sin();
                0.14 * bell
            } else {
                0.0
            };
            if ankle_pitch.abs() > 1e-3 {
                foot_rot = (Quat::from_axis_angle(ctx.right(), ankle_pitch) * foot_rot).normalize();
            }
            let (thigh_rot, calf_rot) = ik();
            out.joint_weighted(thigh, JointTarget::World(thigh_rot), k * 1.45, 1.05, priority::BALANCE, 1.0);
            out.joint_weighted(
                calf,
                JointTarget::Local((thigh_rot.inverse() * calf_rot).normalize()),
                k * 1.15,
                1.10,
                priority::BALANCE,
                1.0,
            );
            out.joint(foot, JointTarget::World(foot_rot), k * 0.72, priority::BALANCE);
            return;
        }

        // Stance (SIMBICON torso control + Coros 2010 / Geijtenbeek 2012 VMC).
        // Use animated knee only during quiet standing before any step; once
        // stepping starts, IK lets the stance knee yield naturally so the
        // reaching swing foot actually makes ground contact on time.
        let (thigh_goal, calf_goal) = if self.steps == 0 && !self.is_stepping() {
            (ctx.rot(thigh), None)
        } else {
            let (t, c) = ik();
            (t, Some(c))
        };
        out.joint(
            thigh,
            JointTarget::Local((pelvis_goal.inverse() * thigh_goal).normalize()),
            k,
            priority::BALANCE,
        );
        match calf_goal {
            Some(c) => {
                let damp = if matches!(self.phase, Phase::Settle { .. }) { 1.65 } else { 1.25 };
                out.joint_weighted(
                    calf,
                    JointTarget::Local((thigh_goal.inverse() * c).normalize()),
                    k,
                    damp,
                    priority::BALANCE,
                    1.0,
                );
                out.joint(foot, JointTarget::World(foot_rot), k, priority::BALANCE);
            }
            None => {
                out.joint(calf, JointTarget::Animation, k, priority::BALANCE);
                out.joint(foot, JointTarget::Animation, k, priority::BALANCE);
            }
        }
    }
}

impl Behaviour for BodyBalance {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        let s = ctx.sense;
        let rig = ctx.rig;
        let dt = ctx.dt;
        let h = rig.height;
        let max_steps = self.max_steps.unwrap_or(ctx.config.max_steps);
        self.trip_boost = (self.trip_boost - dt).max(0.0);

        // ---- Failure -------------------------------------------------------
        let swinging = self.is_stepping();
        if s.feet_down() == 0 && !swinging {
            self.airborne += dt;
        } else {
            self.airborne = 0.0;
        }
        let failed = s.trunk_tilt > 1.05
            || s.pelvis_height < 0.48 * rig.pelvis_height
            || self.airborne > 0.6
            || s.is_high_fall(h)
            || (self.steps >= max_steps && !swinging && s.capture_margin > 0.14 * h);
        if failed && self.give_up {
            out.signal(PuppetSignal::LostBalance);
        }

        // Organic auto-windmill request on severe tipping angular momentum spikes!
        let l_norm = s.angular_momentum.reject_from(s.up).length() / (rig.total_mass * h).max(1.0);
        if l_norm > 0.58 && s.horizontal_speed() > 0.70 && s.capture_margin > 0.10 * h {
            out.signal(PuppetSignal::NeedWindmill);
        }

        // ---- Settle timer after landing ------------------------------------
        if let Phase::Settle { t, duration } = self.phase {
            let next_t = t + dt;
            let urgent = s.feet_down() == 2
                && next_t >= 0.08
                && s.capture_margin > 0.16 * h
                && s.horizontal_speed() > 0.65;
            if (next_t >= duration && s.feet_down() == 2) || next_t >= duration * 1.8 || urgent {
                self.phase = Phase::Stand;
            } else {
                self.phase = Phase::Settle { t: next_t, duration };
            }
        }

        // ---- Step decision -------------------------------------------------
        let trigger = if self.steps == 0 {
            self.step_threshold * h
        } else {
            (self.step_threshold * 1.15) * h
        };
        if matches!(self.phase, Phase::Stand) && s.capture_margin > trigger {
            self.unbalanced += dt;
        } else {
            self.unbalanced = 0.0;
        }
        let step_confirmed = self.unbalanced >= 0.05 || s.horizontal_speed() > 0.40;
        if let Phase::Stand = self.phase
            && s.capture_margin > trigger
            && step_confirmed
            && self.steps < max_steps
            && s.feet_down() > 0
        {
            let swing = if s.feet_down() == 1 {
                if s.foot_down(Side::Left) { Side::Right } else { Side::Left }
            } else {
                let right = ctx.right();
                let right2 = s.flat(right).normalize_or(Vec2::X);
                let delta = s.capture_point - s.support_centre;
                let lateral = delta.dot(right2);
                let lon = (delta - right2 * lateral).length();
                if lateral.abs() > 0.06 * h && lateral.abs() > lon * 0.75 {
                    let same_side = if lateral > 0.0 { Side::Right } else { Side::Left };
                    let sign = if same_side == Side::Right { 1.0 } else { -1.0 };
                    let foot_span = (s.flat(ctx.pos(PuppetPart::foot(same_side)))
                        - s.flat(ctx.pos(PuppetPart::foot(same_side.opposite()))))
                        .dot(right2)
                        * sign;
                    if foot_span > 0.28 * h { same_side.opposite() } else { same_side }
                } else {
                    let dl = (s.flat(ctx.pos(PuppetPart::FootL)) - s.capture_point).length();
                    let dr = (s.flat(ctx.pos(PuppetPart::FootR)) - s.capture_point).length();
                    if dl > dr { Side::Left } else { Side::Right }
                }
            };
            let duration = self.adaptive_step_duration(ctx);
            self.phase = Phase::Swing {
                side: swing,
                t: 0.0,
                duration,
                from: ctx.pos(PuppetPart::foot(swing)),
                to: self.step_target(ctx, swing, duration),
            };
        }

        // ---- Legs ----------------------------------------------------------
        let pelvis_goal = self.pelvis_goal(ctx);
        let want_pelvis_h = self.desired_pelvis_height(ctx);
        let mut swing_side = None;
        if let Phase::Swing { side, t, duration, from, to } = self.phase {
            let u = t / duration;
            let mut to = to;
            if u < 0.85 {
                to = self.step_target(ctx, side, (duration - t).max(0.0));
            }
            let u_clamped = u.clamp(0.0, 1.0);
            let swing_dir = (to - from).reject_from(s.up).normalize_or(s.heading);
            let swing_fwd_dot = swing_dir.dot(s.heading);

            // Detect mid-swing obstacle trip (shin or toe hits steep obstacle opposing swing!).
            let foot_p = PuppetPart::foot(side);
            let calf_p = PuppetPart::calf(side);
            let snagged = u > 0.16 && u < 0.72 && [foot_p, calf_p].iter().any(|&p| {
                s.touching(p)
                    && s.contact_normal(p).dot(s.up).abs() < 0.55
                    && s.contact_normal(p).dot(swing_dir) < -0.50
                    && (s.contact_speed(p) > 0.65 || s.contact_impulse(p) > 2.5)
            });

            if snagged {
                // Obstacle tripped the swing foot! Abort step, request windmill, speed up recovery hop!
                out.signal(PuppetSignal::Tripped { side });
                out.signal(PuppetSignal::NeedWindmill);
                self.trip_boost = 0.65;
                self.steps += 1;
                self.phase = Phase::Settle { t: 0.0, duration: 0.10 };
            } else {
                // Terrain-aware arc clearance: lift above the HIGHER of `from` and `to`
                // so stepping up onto a ramp or step never scuffs!
                let base_from_h = from.dot(s.up);
                let base_to_h = to.dot(s.up);
                let linear_h = base_from_h + (base_to_h - base_from_h) * smooth(u_clamped);
                let crest_boost = (base_to_h - base_from_h).max(0.0) * 0.45;
                let lift = (0.065 * h + crest_boost) * (std::f32::consts::PI * u_clamped.powf(0.7)).sin();
                let press = if u >= 0.85 { -0.025 * ((u - 0.85) / 0.15).clamp(0.0, 1.0) } else { 0.0 };
                let flat_p = s.flat(from).lerp(s.flat(to), smooth(u_clamped));
                let p = s.lift(flat_p, linear_h + lift + press);
                self.leg(ctx, out, side, p, Some(u_clamped), swing_fwd_dot, pelvis_goal, want_pelvis_h);
                swing_side = Some(side);
                let t = t + dt;
                let landed = (u >= 0.85 && s.foot_down(side)) || u >= 1.4;
                self.phase = if landed {
                    self.steps += 1;
                    out.signal(PuppetSignal::StepTaken {
                        side,
                        point: ctx.pos(foot_p),
                    });
                    Phase::Settle { t: 0.0, duration: 0.22 }
                } else {
                    Phase::Swing { side, t, duration, from, to }
                };
            }
        }
        for side in [Side::Left, Side::Right] {
            if Some(side) == swing_side {
                continue;
            }
            let hip_flat = s.flat(ctx.pos(PuppetPart::thigh(side)));
            let mut ankle_flat = s.flat(ctx.pos(PuppetPart::foot(side)));
            let l_leg = rig.part(PuppetPart::thigh(side)).length
                + rig.part(PuppetPart::calf(side)).length;
            let max_reach = if s.foot_down(side) { 0.56 * l_leg } else { 0.42 * l_leg };
            let off = ankle_flat - hip_flat;
            if off.length() > max_reach {
                ankle_flat = hip_flat + off.normalize() * max_reach;
            }
            let on_slope = s.foot_normals[side.index()].angle_between(s.up) > 0.06;
            let foot_h = if on_slope && s.foot_down(side) {
                ctx.pos(PuppetPart::foot(side))
                    .dot(s.up)
                    .clamp(s.ground - 0.25 * h, s.ground + rig.ankle_height + 0.30 * h)
            } else {
                s.ground + rig.ankle_height
            };
            let ankle = s.lift(ankle_flat, foot_h);
            self.leg(ctx, out, side, ankle, None, 0.0, pelvis_goal, want_pelvis_h);
        }

        // ---- Jacobian-transpose virtual forces & angular momentum control --
        self.emit_virtual_forces(ctx, out, swing_side, want_pelvis_h);

        // ---- Upper-body flywheel counterweight & vestibular head -----------
        self.upper_body_counterweight(ctx, out);

        // ---- Upright assistance (optional game-balancer helper) ------------
        let assist = ctx.config.upright_assist * self.strength;
        if assist > 0.0 && s.feet_down() > 0 {
            let unit = rig.torque_unit(s.g);
            let max = assist * unit;
            for (part, goal) in [
                (PuppetPart::Pelvis, pelvis_goal),
                (
                    PuppetPart::SpineUpper,
                    (pelvis_goal
                        * ctx.targets.rot(PuppetPart::Pelvis).inverse()
                        * ctx.targets.rot(PuppetPart::SpineUpper))
                    .normalize(),
                ),
            ] {
                let b = s.body(part);
                let err = crate::pose::shortest(goal * b.rot.inverse()).to_scaled_axis();
                let inertia = crate::muscle::side_inertia(s, b.pos, |p| {
                    !matches!(p.limb(), Limb::LeftLeg | Limb::RightLeg)
                });
                let k = 2.0 * unit;
                let mean = ((inertia.x_axis.x + inertia.y_axis.y + inertia.z_axis.z) / 3.0).max(0.1);
                let d = 2.0 * (k * mean).sqrt();
                let mut tau = crate::muscle::stable_pd_tensor(k, d, inertia, err, b.ang_vel, dt);
                if tau.length() > max {
                    tau = tau.normalize() * max;
                }
                out.external_torques.push((part, tau * 0.5));
            }
        }

        // ---- Planted feet --------------------------------------------------
        for side in [Side::Left, Side::Right] {
            if Some(side) != swing_side && s.foot_down(side) {
                out.plant[side.index()] = true;
            }
        }

        // ---- Recovered? ----------------------------------------------------
        let calm = !matches!(self.phase, Phase::Swing { .. })
            && s.feet_down() == 2
            && s.capture_margin < -0.005
            && s.horizontal_speed() < 0.22
            && s.trunk_tilt < 0.25;
        self.calm = if calm { self.calm + dt } else { 0.0 };
        if self.calm > 0.40 {
            out.signal(PuppetSignal::Recovered);
        }
        BehaviourStatus::Running
    }
}

/// Stumble in the push direction with weakening legs, then fall (`StaggerFall`).
#[derive(Clone, Debug)]
pub struct StaggerFall {
    /// Horizontal push direction.
    pub direction: Vec3,
    /// Seconds before giving up.
    pub duration: f32,
    /// Leg strength at the end (fraction).
    pub final_strength: f32,
    /// Trunk lean into the push at the end (rad).
    pub lean: f32,
    t: f32,
    balance: BodyBalance,
}

impl StaggerFall {
    pub const NAME: &'static str = "stagger_fall";

    pub fn new(direction: Vec3) -> Self {
        Self {
            direction,
            duration: 2.0,
            final_strength: 0.45,
            lean: 0.45,
            t: 0.0,
            balance: BodyBalance {
                step_threshold: 0.02,
                max_steps: Some(8),
                step_time: 0.26,
                give_up: false,
                ..default()
            },
        }
    }
}

impl Behaviour for StaggerFall {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        self.t += ctx.dt;
        let k = (self.t / self.duration).clamp(0.0, 1.0);
        let dir = self.direction.reject_from(ctx.sense.up).normalize_or_zero();
        self.balance.strength = 1.0 + (self.final_strength - 1.0) * k;
        self.balance.lean = ctx.sense.up.cross(dir) * (self.lean * k);
        self.balance.update(ctx, out);
        // Recovering is not part of a stagger; never signal it.
        out.signals.retain(|s| *s != PuppetSignal::Recovered);
        if self.t >= self.duration || ctx.sense.trunk_tilt > 0.9 {
            out.signal(PuppetSignal::LostBalance);
        }
        out.weaken(Limb::Spine, 0.6 + 0.4 * (1.0 - k));
        BehaviourStatus::Running
    }
}
