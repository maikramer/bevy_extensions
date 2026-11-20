//! Glancing shoulder bump & crowd jostle reaction ([`BumpReaction`]).
//!
//! Unlike a combat punch ([`super::HitReaction`]), when a real human is bumped
//! in a crowd or brushes past another person:
//! 1. **Brief passive yield** (~45 ms): the struck shoulder gives way physically.
//! 2. **Ribcage slip-twist & shoulder dip**: the thoracic spine rotates axially
//!    so the struck shoulder slips *past* the obstacle (making room!) with a
//!    subtle lateral lean rather than a dramatic combat recoil.
//! 3. **Arm choreography**: the struck arm trails behind briefly; on a frontal
//!    bump both forearms raise slightly palms-out ("whoa!"); on a shoulder brush
//!    the off-arm abducts subtly as counterweight.
//! 4. **Orienting gaze reflex**: the head glances naturally toward the bumper
//!    before returning forward — one of the strongest visual cues of lifelike NPCs.

use std::any::Any;

use bevy::prelude::*;

use super::{Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, priority};
use crate::ik::{aim, smooth, smootherstep, solve_arm_ik};
use crate::part::{Limb, PuppetPart, Side};

/// Natural glancing shoulder-slip & orienting gaze reaction to being bumped.
#[derive(Clone, Debug)]
pub struct BumpReaction {
    pub part: PuppetPart,
    /// Horizontal push direction (pointing away from the bumper).
    pub direction: Vec3,
    /// Optional world position of whoever bumped us (for orienting gaze!).
    pub bumper_pos: Option<Vec3>,
    /// Intensity in `[0, 1]` (0.2 = light crowd brush, 0.6 = firm shoulder check, 1.0 = hard bump).
    pub intensity: f32,
    pub duration: f32,
    /// Stay in Animated mode (upper-body shoulder slip while keeping locomotion).
    pub partial: bool,
    pub reaction_delay: f32,
    t: f32,
}

impl BumpReaction {
    pub const NAME: &'static str = "bump_reaction";

    pub fn new(part: PuppetPart, direction: Vec3, intensity: f32) -> Self {
        let intensity = intensity.clamp(0.0, 1.0);
        Self {
            part,
            direction: direction.normalize_or_zero(),
            bumper_pos: None,
            intensity,
            duration: 0.48 + 0.34 * intensity,
            partial: true,
            reaction_delay: 0.045,
            t: 0.0,
        }
    }

    pub fn with_bumper_pos(mut self, pos: Option<Vec3>) -> Self {
        self.bumper_pos = pos;
        self
    }

    pub fn with_delay(mut self, delay: f32) -> Self {
        self.reaction_delay = delay.clamp(0.0, 0.12);
        self
    }

    pub fn full_body(mut self) -> Self {
        self.partial = false;
        self
    }

    /// Smoothly refreshes an ongoing bump if jostled again in a tight crowd.
    pub fn stack(&mut self, part: PuppetPart, direction: Vec3, bumper_pos: Option<Vec3>, intensity: f32) {
        self.part = part;
        let d = direction.normalize_or_zero();
        if d.length_squared() > 0.25 {
            self.direction = d;
        }
        if bumper_pos.is_some() {
            self.bumper_pos = bumper_pos;
        }
        self.intensity = (self.intensity * 0.4 + intensity.clamp(0.0, 1.0) * 0.8).clamp(0.0, 1.0);
        self.t = (self.t * 0.3).min(0.04);
        self.duration = (0.48 + 0.36 * self.intensity).min(0.95);
    }
}

impl Behaviour for BumpReaction {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        self.t += ctx.dt;
        if self.t >= self.duration {
            return BehaviourStatus::Done;
        }
        let u_total = (self.t / self.duration).clamp(0.0, 1.0);
        let k = 1.0 - smooth(u_total);
        let limb = self.part.limb();

        if self.partial {
            // Keep Pelvis pinned to locomotion while releasing thoracic spine, head & arms.
            let spine_k = (0.92 * k).clamp(0.0, 1.0);
            for p in [PuppetPart::SpineLower, PuppetPart::SpineUpper, PuppetPart::Head] {
                out.show_physics_part(p, spine_k);
                out.limit_pin(p, (1.0 - 0.92 * spine_k).max(0.04));
            }
            for arm_limb in [Limb::LeftArm, Limb::RightArm] {
                let ak = if arm_limb == limb { k } else { 0.72 * spine_k };
                out.show_physics(arm_limb, ak);
                out.limit_pins(arm_limb, (1.0 - ak).max(0.06));
            }
        }

        // Brief passive yield on contact so the shoulder genuinely absorbs the brush.
        let delay = self.reaction_delay.min(self.duration * 0.2);
        if self.t < delay {
            if limb != Limb::Spine {
                out.weaken(limb, 0.35);
                out.limb(limb, JointTarget::Current, 0.3, priority::REACTION);
            }
            out.joint_weighted(PuppetPart::SpineUpper, JointTarget::Current, 0.45, 0.9, priority::REACTION, 1.0);
            return BehaviourStatus::Running;
        }

        let up = ctx.sense.up;
        let fwd = ctx.sense.heading;
        let right = ctx.right();
        let dir = self.direction.reject_from(up).normalize_or_zero();
        if dir.length_squared() < 1e-4 {
            return BehaviourStatus::Running;
        }

        let active_dur = (self.duration - delay).max(0.08);
        let u = ((self.t - delay) / active_dur).clamp(0.0, 1.0);
        let onset = smootherstep(u / 0.18);
        let fade = 1.0 - smootherstep((u - 0.18) / 0.82);
        let env = (onset * fade).clamp(0.0, 1.0);
        let amp = (0.42 + 0.58 * self.intensity) * env;

        let fwd_push = dir.dot(fwd); // > 0 pushed forward (bumped from behind), < 0 bumped from front
        let lat_push = dir.dot(right); // > 0 pushed right (bumped on left shoulder)

        // ---- 1. Ribcage slip-twist + subtle lateral shoulder dip -------------
        // Determine which shoulder took the brush: spin that shoulder *backward*
        // so the torso turns sideways to slip smoothly past the passerby!
        let slip_sign = match self.part.side() {
            Some(Side::Left) => if fwd_push < 0.2 { 1.0 } else { -1.0 },
            Some(Side::Right) => if fwd_push < 0.2 { -1.0 } else { 1.0 },
            None => if lat_push >= 0.0 { 1.0 } else { -1.0 },
        };
        let twist_angle = slip_sign * (0.45 + 0.32 * lat_push.abs()) * amp;
        let axis = up.cross(dir).normalize_or(right);
        // Much subtler spine pitch than a combat punch — feels like a crowd bump, not a fist!
        let lower_bend = 0.18 * amp;
        let upper_bend = 0.30 * amp;

        for (part, bend, tw_frac) in [
            (PuppetPart::SpineLower, lower_bend, 0.35),
            (PuppetPart::SpineUpper, upper_bend, 0.78),
        ] {
            let parent = part.parent().expect("spine has parent");
            let anim_world = ctx.rot(parent) * ctx.targets.local[part.index()];
            let bend_q = Quat::from_axis_angle(axis, bend);
            let twist_q = Quat::from_axis_angle(up, twist_angle * tw_frac);
            let target = (twist_q * bend_q * anim_world).normalize();
            out.joint(part, JointTarget::World(target), 3.6, priority::REACTION);
        }

        // ---- 2. Orienting Gaze Reflex: glance toward the bumper! -------------
        let head_pos = ctx.pos(PuppetPart::Head);
        let glance_target = self.bumper_pos.unwrap_or_else(|| head_pos - dir * 2.0);
        let to_bumper = (glance_target - head_pos).reject_from(up).normalize_or(-dir);
        let glance_env = smootherstep((u - 0.04) / 0.18) * (1.0 - smootherstep((u - 0.55) / 0.45));
        if glance_env > 0.03 {
            // Publish gaze target for balance coordination AND turn neck smoothly.
            out.look_at(glance_target, priority::GAZE + 2, 0.55);
            let yaw_raw = crate::rig::signed_angle(fwd, to_bumper, up);
            // Comfortable human glance yaw ±58° (don't owl-spin 180° when bumped from straight behind).
            let glance_yaw = yaw_raw.clamp(-1.0, 1.0) * glance_env * (0.65 + 0.35 * self.intensity);
            let neck_anim = ctx.rot(PuppetPart::SpineUpper) * ctx.targets.local[PuppetPart::Head.index()];
            // Subtle head tilt/recoil + counter-compensate spine twist so eyes lock on bumper.
            let look_q = Quat::from_axis_angle(up, glance_yaw) * Quat::from_axis_angle(axis, 0.10 * amp);
            out.joint(
                PuppetPart::Head,
                JointTarget::World((look_q * neck_anim).normalize()),
                3.8,
                priority::REACTION,
            );
        }

        // ---- 3. Natural Arm Choreography -------------------------------------
        if limb != Limb::Spine {
            out.weaken(limb, (1.0 - 0.55 * self.intensity * k).clamp(0.35, 1.0));
        }
        let h = ctx.rig.height;
        let is_frontal = fwd_push < -0.45;
        let arm_w = (0.78 * amp).clamp(0.0, 0.90);
        if arm_w > 0.04 {
            if is_frontal {
                // Frontal bump: hands come up defensively palms-out in front of chest!
                let chest = ctx.pos(PuppetPart::SpineUpper);
                for side in [Side::Left, Side::Right] {
                    let sign = if side == Side::Right { 1.0 } else { -1.0 };
                    let target = chest + fwd * (0.24 * h) + right * (sign * 0.15 * h) - up * (0.04 * h);
                    let pole = (-fwd * 0.4 + right * (sign * 0.6) - up * 0.5).normalize();
                    let upper = PuppetPart::upper_arm(side);
                    let lower = PuppetPart::lower_arm(side);
                    let urig = ctx.rig.part(upper);
                    let lrig = ctx.rig.part(lower);
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
                    out.joint_weighted(upper, JointTarget::World(ur), 2.5, 1.0, priority::REACTION, arm_w);
                    out.joint_weighted(
                        lower,
                        JointTarget::Local((ur.inverse() * lr).normalize()),
                        2.5,
                        1.05,
                        priority::REACTION,
                        arm_w,
                    );
                }
            } else {
                // Glancing shoulder brush: struck arm trails passively, off-arm lifts gently.
                for side in [Side::Left, Side::Right] {
                    let upper = PuppetPart::upper_arm(side);
                    if upper.limb() == limb {
                        continue;
                    }
                    let sign = if side == Side::Right { 1.0 } else { -1.0 };
                    let flare = (right * (sign * 0.62) + fwd * 0.22 - up * 0.65).normalize();
                    let ur = aim(ctx.rot(upper), ctx.rig.part(upper).axis_local, flare);
                    out.joint_weighted(upper, JointTarget::World(ur), 2.2, 1.0, priority::REACTION, arm_w);
                }
            }
        }

        BehaviourStatus::Running
    }
}
