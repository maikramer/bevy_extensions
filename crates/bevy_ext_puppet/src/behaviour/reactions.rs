//! Reactions to combat hits and loss of balance:
//!
//! * [`HitReaction`] — Euphoria-inspired **multi-zone anatomical hit reaction**
//!   with finite **sensorimotor reflex latency** (`config.reaction_delay` ≈ 65 ms
//!   of pure passive inertial yield before active flinch/guard kicks in),
//!   distinct biomechanics for 6 anatomical zones (`Head` whip + face-guard,
//!   `Chest` inertial head-lag whiplash, `Abdomen` forward gut-jackknife +
//!   clamped elbows, `Pelvis` base-sweep counter-tilt, `Leg` reflexive knee
//!   buckle + hip drop, `Arm` shoulder spin + axial thoracic twist), and
//!   in-place **hit stacking** so rapid blows compound naturally!
//! * [`ArmsWindmill`] — momentum-coupled flywheel arm windmilling aligned with
//!   both horizontal velocity and tipping centroidal angular momentum
//!   (Macchietto et al. 2009 / Pratt et al. 2006), with twist-locked elbows.

use std::any::Any;

use bevy::prelude::*;

use super::{Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, priority};
use crate::ik::{aim, smooth, smootherstep, solve_arm_ik};
use crate::part::{BodyZone, Limb, PuppetPart, Side};

/// Type of impact striking the body (modulates sharpness, whip, and guard reflex).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub enum HitKind {
    /// Blunt impact (punch, club, mace, heavy collision).
    #[default]
    Blunt,
    /// Slashing blade (sword, axe: sharper axial whip & defensive arm tuck).
    Slash,
    /// Piercing strike (arrow, spear: localized jolt + tight flinch).
    Pierce,
    /// Shockwave / blast (broad torso/head recoil + curl).
    Blast,
}

/// Multi-zone directional 360° hit reaction with sensorimotor latency and stacking.
#[derive(Clone, Debug)]
pub struct HitReaction {
    pub part: PuppetPart,
    /// Unit direction of the hit (where it pushes).
    pub direction: Vec3,
    /// Optional local hit offset relative to `part` origin (adds off-centre torsion).
    pub local_offset: Vec3,
    /// Impact kind (blunt/slash/pierce/blast).
    pub kind: HitKind,
    /// 0..1, how hard.
    pub severity: f32,
    pub duration: f32,
    /// Stay animated and only let the upper body + struck limb yield physically.
    pub partial: bool,
    /// Sensorimotor latency (s) before active muscular flinch overrides passive inertia.
    pub reaction_delay: f32,
    t: f32,
    stacks: u32,
}

impl HitReaction {
    pub const NAME: &'static str = "hit_reaction";

    pub fn new(part: PuppetPart, direction: Vec3, severity: f32) -> Self {
        let severity = severity.clamp(0.0, 1.0);
        Self {
            part,
            direction: direction.normalize_or_zero(),
            local_offset: Vec3::ZERO,
            kind: HitKind::Blunt,
            severity,
            duration: 0.52 + 0.42 * severity,
            partial: false,
            reaction_delay: 0.065,
            t: 0.0,
            stacks: 1,
        }
    }

    pub fn with_kind(mut self, kind: HitKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_local_offset(mut self, offset: Vec3) -> Self {
        self.local_offset = offset;
        self
    }

    pub fn with_delay(mut self, delay: f32) -> Self {
        self.reaction_delay = delay.clamp(0.0, 0.18);
        self
    }

    pub fn partial(mut self) -> Self {
        self.partial = true;
        self
    }

    /// Compounds a new incoming hit onto an already running reaction instead of
    /// abruptly discarding accumulated momentum/trauma.
    pub fn stack(
        &mut self,
        part: PuppetPart,
        direction: Vec3,
        local_offset: Vec3,
        kind: HitKind,
        severity: f32,
        partial: bool,
    ) {
        let dir = direction.normalize_or_zero();
        self.part = part;
        self.direction = if self.direction.length_squared() > 0.25 && dir.length_squared() > 0.25 {
            (self.direction * 0.25 + dir * 0.75).normalize_or(dir)
        } else {
            dir
        };
        self.local_offset = local_offset;
        self.kind = kind;
        self.stacks = self.stacks.saturating_add(1);
        self.severity = (self.severity * 0.45 + severity.clamp(0.0, 1.0) * 0.85).clamp(0.0, 1.0);
        self.partial = self.partial && partial;
        // Brief fresh passive yield, then deeper compounded recoil.
        self.t = 0.0;
        self.duration = (0.52 + 0.46 * self.severity).min(1.15);
    }
}

impl Behaviour for HitReaction {
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
        let delay = self.reaction_delay.min(self.duration * 0.25);
        let u_total = (self.t / self.duration).clamp(0.0, 1.0);
        let k = 1.0 - smooth(u_total);

        let limb = self.part.limb();
        let zone = self.part.zone();
        let up = ctx.sense.up;
        let fwd = ctx.sense.heading;
        let right = ctx.right();
        let dir = self.direction.reject_from(up).normalize_or_zero();

        // Keep partial visibility & pin release active IMMEDIATELY from t = 0 so
        // the passive impulse physically moves the struck body parts!
        if self.partial {
            let spine_k = (0.96 * k).clamp(0.0, 1.0);
            for p in [PuppetPart::SpineLower, PuppetPart::SpineUpper, PuppetPart::Head] {
                out.show_physics_part(p, spine_k);
                out.limit_pin(p, (1.0 - 0.96 * spine_k).max(0.02));
            }
            if limb != Limb::Spine {
                out.show_physics(limb, k);
                out.limit_pins(limb, (1.0 - k).max(0.02));
            }
            for arm_limb in [Limb::LeftArm, Limb::RightArm] {
                if arm_limb != limb {
                    let ak = 0.78 * spine_k;
                    out.show_physics(arm_limb, ak);
                    out.limit_pins(arm_limb, (1.0 - ak).max(0.18));
                }
            }
        }

        // ---- Phase 1: Sensorimotor Reflex Latency (0..delay) -----------------
        // During the first ~50–65 ms of a significant unexpected blow (`severity >= 0.12`),
        // voluntary/reflexive muscle commands have not arrived yet: the struck limb
        // and local trunk soften passively so Avian's impact impulse visibly moves
        // the body with genuine physical weight before active flinch kicks in.
        // Feather-light nudges (`severity < 0.12`) keep normal posture tone.
        let eff_delay = if self.severity >= 0.12 {
            delay * ((self.severity - 0.10) / 0.25).clamp(0.25, 1.0)
        } else {
            0.0
        };
        if self.t < eff_delay {
            let yield_k = (0.48 - 0.28 * self.severity).clamp(0.18, 0.48);
            if limb != Limb::Spine {
                out.weaken(limb, (0.35 + 0.35 * (1.0 - self.severity)).clamp(0.25, 0.70));
                out.limb(limb, JointTarget::Animation, yield_k, priority::REACTION);
            }
            match zone {
                BodyZone::Head => {
                    out.joint_weighted(
                        PuppetPart::Head,
                        JointTarget::Animation,
                        yield_k,
                        0.80,
                        priority::REACTION,
                        1.0,
                    );
                }
                BodyZone::Chest | BodyZone::Abdomen => {
                    out.joint_weighted(
                        PuppetPart::SpineUpper,
                        JointTarget::Animation,
                        yield_k + 0.10,
                        0.85,
                        priority::REACTION,
                        1.0,
                    );
                    out.joint_weighted(
                        PuppetPart::Head,
                        JointTarget::Animation,
                        yield_k,
                        0.75,
                        priority::REACTION,
                        1.0,
                    );
                }
                BodyZone::Leg(side) => {
                    out.weaken(Limb::leg(side), 0.28);
                    out.joint_weighted(
                        PuppetPart::calf(side),
                        JointTarget::Current,
                        0.20,
                        0.65,
                        priority::REACTION,
                        1.0,
                    );
                }
                _ => {}
            }
            return BehaviourStatus::Running;
        }

        // ---- Phase 2: Active Multi-Zone Anatomical Reaction ------------------
        let active_dur = (self.duration - eff_delay).max(0.08);
        let u = ((self.t - eff_delay) / active_dur).clamp(0.0, 1.0);
        let attack = smootherstep(u / 0.16);
        let recover = 1.0 - smootherstep((u - 0.16) / 0.84);
        let env = (attack * recover).clamp(0.0, 1.0);
        let kind_mult = match self.kind {
            HitKind::Blunt => 1.0,
            HitKind::Slash => 1.08,
            HitKind::Pierce => 0.90,
            HitKind::Blast => 1.20,
        };
        let trauma_boost = 1.0 + 0.25 * ctx.neuro.trauma;
        let amp = ((0.58 + 0.42 * self.severity) * kind_mult * trauma_boost).clamp(0.55, 1.25) * env;

        if limb != Limb::Spine {
            out.weaken(limb, (1.0 - 0.78 * self.severity * k).clamp(0.18, 1.0));
        }

        // Direct gaze briefly toward the incoming threat on recovery (`-dir`).
        if dir.length_squared() > 0.25 && u > 0.22 && u < 0.75 {
            let threat_pos = ctx.pos(PuppetPart::Head) - dir * 2.5;
            out.look_at(threat_pos, priority::GAZE, 0.45);
        }

        let axis = up.cross(dir);
        if axis.length_squared() <= 1e-6 {
            return BehaviourStatus::Running;
        }
        let axis = axis.normalize();
        // `fwd_hit > 0` = struck from BEHIND (pushes forward);
        // `fwd_hit < 0` = struck from IN FRONT (pushes backward).
        let fwd_hit = dir.dot(fwd);
        let lat_hit = dir.dot(right);
        let slash_twist_boost = if self.kind == HitKind::Slash { 1.35 } else { 1.0 };

        // Off-centre impact point torsion (world lever arm × impulse around `up`).
        let lever_w = ctx.rot(self.part) * self.local_offset;
        let offset_twist = lever_w.cross(dir).dot(up).clamp(-0.35, 0.35) * 2.2;

        // Zone-specific anatomical flinch profiles!
        match zone {
            BodyZone::Head => {
                // HEAD SHOT: immediate sharp cervical whip + spin, delayed torso sway,
                // both hands flash up to protect face/brow, knees momentarily dip!
                let head_whip = 0.95 * amp;
                let head_twist = (-lat_hit * 0.75 + offset_twist) * amp;
                let torso_lag = smootherstep((u - 0.08) / 0.22) * recover;
                let lower_bend = 0.26 * torso_lag * (0.6 + 0.4 * self.severity);
                let upper_bend = 0.44 * torso_lag * (0.6 + 0.4 * self.severity);

                apply_spine_flinch(
                    ctx,
                    out,
                    PuppetPart::SpineLower,
                    axis,
                    lower_bend,
                    up,
                    head_twist * 0.20,
                    3.8,
                );
                apply_spine_flinch(
                    ctx,
                    out,
                    PuppetPart::SpineUpper,
                    axis,
                    upper_bend,
                    up,
                    head_twist * 0.38,
                    4.0,
                );
                apply_spine_flinch(
                    ctx,
                    out,
                    PuppetPart::Head,
                    axis,
                    head_whip,
                    up,
                    head_twist * 0.95,
                    4.6,
                );

                // Defensive face-guard: both hands flash upward near cheekbones/brow!
                let guard_w = (0.90 * env * (0.55 + 0.45 * self.severity)).clamp(0.0, 0.95);
                if guard_w > 0.05 {
                    let head_p = ctx.pos(PuppetPart::Head);
                    let h = ctx.rig.height;
                    for side in [Side::Left, Side::Right] {
                        let sign = if side == Side::Right { 1.0 } else { -1.0 };
                        let target = head_p + fwd * (0.14 * h) + right * (sign * 0.09 * h) - up * (0.04 * h);
                        drive_arm_to(ctx, out, side, target, -up * 0.7 + right * (sign * 0.55) - fwd * 0.2, 2.8, guard_w);
                    }
                }
                // Heavy head blows daze the legs briefly (subtle knee dip).
                if !self.partial && self.severity > 0.45 && u < 0.45 {
                    let daze = (1.0 - u / 0.45) * (self.severity - 0.35);
                    out.weaken(Limb::LeftLeg, (1.0 - 0.45 * daze).clamp(0.55, 1.0));
                    out.weaken(Limb::RightLeg, (1.0 - 0.45 * daze).clamp(0.55, 1.0));
                }
            }

            BodyZone::Abdomen => {
                // GUT PUNCH / LOWER SPINE HIT:
                // Front hit → JACKKNIFE FOLD FORWARD around the blow (pelvis recoils back,
                // chest & head pitch FORWARD over the stomach, elbows clamp tightly in!).
                // Back hit → lumbar hyperextension arch + chest lags back.
                // Flank hit → sharp lateral crunch toward the struck ribs!
                let is_front_gut = fwd_hit < -0.15;
                let gut_pitch_axis = up.cross(fwd).normalize_or(right);
                let twist_w = (-lat_hit * 0.50 + offset_twist) * slash_twist_boost;

                if is_front_gut {
                    // Forward jackknife: chest pitches strongly forward (+fwd), head drops down!
                    let fold = (0.78 + 0.25 * self.severity) * amp;
                    apply_spine_flinch(
                        ctx,
                        out,
                        PuppetPart::SpineLower,
                        gut_pitch_axis,
                        fold * 0.48,
                        up,
                        twist_w * 0.20,
                        4.5,
                    );
                    apply_spine_flinch(
                        ctx,
                        out,
                        PuppetPart::SpineUpper,
                        gut_pitch_axis,
                        fold * 0.92,
                        up,
                        twist_w * 0.35,
                        4.8,
                    );
                    apply_spine_flinch(
                        ctx,
                        out,
                        PuppetPart::Head,
                        gut_pitch_axis,
                        fold * 0.65,
                        up,
                        twist_w * 0.25,
                        3.8,
                    );
                    // Clamp elbows inward across the stomach!
                    let gut_pos = ctx.pos(PuppetPart::SpineLower) + fwd * (0.11 * ctx.rig.height);
                    let guard_w = (0.88 * env).clamp(0.0, 0.95);
                    if guard_w > 0.05 {
                        for side in [Side::Left, Side::Right] {
                            let sign = if side == Side::Right { 1.0 } else { -1.0 };
                            let target = gut_pos + right * (sign * 0.06 * ctx.rig.height) + up * (0.02 * ctx.rig.height);
                            drive_arm_to(
                                ctx,
                                out,
                                side,
                                target,
                                -fwd * 0.55 + right * (sign * 0.65) - up * 0.4,
                                2.8,
                                guard_w,
                            );
                        }
                    }
                } else {
                    // Back or kidney blow: bend along blow + lateral side-crunch.
                    let lower_bend = 0.58 * amp;
                    let upper_bend = 0.46 * amp;
                    let head_bend = if u < 0.25 { -0.45 * amp } else { 0.35 * amp };
                    apply_spine_flinch(ctx, out, PuppetPart::SpineLower, axis, lower_bend, up, twist_w * 0.30, 4.4);
                    apply_spine_flinch(ctx, out, PuppetPart::SpineUpper, axis, upper_bend, up, twist_w * 0.45, 4.4);
                    apply_spine_flinch(ctx, out, PuppetPart::Head, axis, head_bend, up, twist_w * 0.30, 4.0);
                }
            }

            BodyZone::Leg(side) => {
                // LEG SWEEP / KNEE STRIKE:
                // Reflexive extensor inhibition + sharp knee buckle on struck leg,
                // hip drops on that side, torso counter-tilts, off-arm flings wide!
                let buckle_env = smootherstep(u / 0.14) * (1.0 - smootherstep((u - 0.20) / 0.80));
                let buckle = (0.75 + 0.65 * self.severity) * buckle_env;
                out.weaken(Limb::leg(side), (0.18 + 0.30 * (1.0 - buckle_env)).clamp(0.16, 0.85));

                // Flex the struck knee sharply backward around its hinge axis.
                let calf = PuppetPart::calf(side);
                let calf_rig = ctx.rig.part(calf);
                let hinge_local = calf_rig.axis_local.cross(-calf_rig.forward_local).normalize_or(Vec3::NEG_X);
                let anim_calf = ctx.targets.local[calf.index()];
                let buckled_calf = (Quat::from_axis_angle(hinge_local, buckle) * anim_calf).normalize();
                out.joint_weighted(calf, JointTarget::Local(buckled_calf), 2.8, 0.9, priority::REACTION, 1.0);

                // Slight hip flexion drop + upper body counter-tilt toward the dropping side.
                let side_sign = if side == Side::Right { 1.0 } else { -1.0 };
                let roll_axis = fwd;
                let lean = -side_sign * 0.36 * amp;
                apply_spine_flinch(ctx, out, PuppetPart::SpineLower, roll_axis, lean * 0.5, up, -side_sign * 0.20 * amp, 3.6);
                apply_spine_flinch(ctx, out, PuppetPart::SpineUpper, roll_axis, lean * 0.85, up, -side_sign * 0.32 * amp, 3.6);

                // Opposite arm flings wide to catch balance!
                let off_side = side.opposite();
                let off_sign = if off_side == Side::Right { 1.0 } else { -1.0 };
                let upper = PuppetPart::upper_arm(off_side);
                let flare_dir = (right * (off_sign * 0.85) + up * 0.20 - fwd * 0.15).normalize();
                let ur = aim(ctx.rot(upper), ctx.rig.part(upper).axis_local, flare_dir);
                out.joint_weighted(upper, JointTarget::World(ur), 2.6, 1.0, priority::REACTION, (0.85 * env).max(0.05));
            }

            BodyZone::Pelvis => {
                // PELVIS / HIP HIT:
                // Base sweeps out → upper torso lags OPPOSITE to the blow first (whiplash
                // counter-tilt!), then recovers; both arms flare wide for stability!
                let counter_phase = u < 0.36;
                let tilt_sign = if counter_phase { -0.68 } else { 0.32 };
                let twist_w = -lat_hit * 0.45 + offset_twist;
                apply_spine_flinch(ctx, out, PuppetPart::SpineLower, axis, 0.35 * tilt_sign * amp, up, twist_w * 0.25, 4.0);
                apply_spine_flinch(ctx, out, PuppetPart::SpineUpper, axis, 0.62 * tilt_sign * amp, up, twist_w * 0.40, 4.0);
                apply_spine_flinch(ctx, out, PuppetPart::Head, axis, 0.50 * tilt_sign * amp, up, twist_w * 0.25, 3.6);
                flare_arms_directional(ctx, out, limb, fwd, right, up, fwd_hit, lat_hit, amp);
            }

            BodyZone::Chest | BodyZone::Arm(_) => {
                // CHEST / SHOULDER / ARM HIT:
                // Classic Euphoria torque-wave: strong thoracic bend + axial shoulder spin
                // + true inertial cervical whiplash (head lags behind accelerating chest
                // during first 28%, then snaps through!) + directional arm recoil.
                let part_twist = match self.part.side() {
                    Some(Side::Left) => if fwd_hit < 0.0 { 1.0 } else { -1.0 },
                    Some(Side::Right) => if fwd_hit < 0.0 { -1.0 } else { 1.0 },
                    None => 0.0,
                };
                let twist_weight = if part_twist != 0.0 {
                    (part_twist * (0.70 + 0.35 * lat_hit.abs()) + offset_twist) * slash_twist_boost
                } else {
                    (-lat_hit * 0.58 + offset_twist) * slash_twist_boost
                };

                let back_boost = if fwd_hit > 0.0 { 1.18 } else { 1.0 };
                let lower_bend = 0.44 * back_boost * amp;
                let upper_bend = 0.76 * back_boost * amp;

                // True inertial cervical whiplash: lag behind accelerating chest first!
                let head_bend = if u < 0.28 {
                    let lag = smootherstep(u / 0.12) * (1.0 - smootherstep((u - 0.12) / 0.16));
                    -0.66 * (0.55 + 0.45 * self.severity) * lag
                } else {
                    0.54 * back_boost * amp
                };

                apply_spine_flinch(ctx, out, PuppetPart::SpineLower, axis, lower_bend, up, twist_weight * 0.26 * amp, 4.4);
                apply_spine_flinch(ctx, out, PuppetPart::SpineUpper, axis, upper_bend, up, twist_weight * 0.58 * amp, 4.5);
                apply_spine_flinch(ctx, out, PuppetPart::Head, axis, head_bend, up, twist_weight * 0.36 * amp, 4.2);

                flare_arms_directional(ctx, out, limb, fwd, right, up, fwd_hit, lat_hit, amp);
            }
        }

        BehaviourStatus::Running
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_spine_flinch(
    ctx: &BehaviourCtx,
    out: &mut PuppetRequests,
    part: PuppetPart,
    bend_axis: Vec3,
    bend_rad: f32,
    twist_axis: Vec3,
    twist_rad: f32,
    stiffness: f32,
) {
    let parent = part.parent().expect("spine parts have parents");
    let anim_world = ctx.rot(parent) * ctx.targets.local[part.index()];
    let bend_q = Quat::from_axis_angle(bend_axis, bend_rad.clamp(-1.1, 1.1));
    let twist_q = Quat::from_axis_angle(twist_axis, twist_rad.clamp(-0.9, 0.9));
    let flinch = (twist_q * bend_q * anim_world).normalize();
    out.joint(part, JointTarget::World(flinch), stiffness, priority::REACTION);
}

#[allow(clippy::too_many_arguments)]
fn drive_arm_to(
    ctx: &BehaviourCtx,
    out: &mut PuppetRequests,
    side: Side,
    target: Vec3,
    elbow_pole: Vec3,
    stiffness: f32,
    weight: f32,
) {
    let upper = PuppetPart::upper_arm(side);
    let lower = PuppetPart::lower_arm(side);
    let urig = ctx.rig.part(upper);
    let lrig = ctx.rig.part(lower);
    let (ur, lr, _) = solve_arm_ik(
        ctx.pos(upper),
        target,
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
    out.joint_weighted(upper, JointTarget::World(ur), stiffness, 1.05, priority::REACTION, weight);
    out.joint_weighted(
        lower,
        JointTarget::Local((ur.inverse() * lr).normalize()),
        stiffness,
        1.10,
        priority::REACTION,
        weight,
    );
}

#[allow(clippy::too_many_arguments)]
fn flare_arms_directional(
    ctx: &BehaviourCtx,
    out: &mut PuppetRequests,
    struck_limb: Limb,
    fwd: Vec3,
    right: Vec3,
    up: Vec3,
    fwd_hit: f32,
    lat_hit: f32,
    amp: f32,
) {
    let arm_env = (0.82 * amp).clamp(0.0, 0.95);
    if arm_env <= 0.04 {
        return;
    }
    for side in [Side::Left, Side::Right] {
        let sign = if side == Side::Right { 1.0 } else { -1.0 };
        let upper = PuppetPart::upper_arm(side);
        if upper.limb() == struck_limb {
            // Struck arm stays loose so physics carries it naturally with the blow.
            continue;
        }
        let upper_dir = if fwd_hit > 0.25 {
            // Hit from behind → arms flare forward & out (brace instinct).
            (fwd * 0.65 + right * (sign * 0.55) - up * 0.32).normalize()
        } else if fwd_hit < -0.25 {
            // Hit from front → arms recoil outward & slightly up.
            (-fwd * 0.35 + right * (sign * 0.70) - up * 0.38).normalize()
        } else {
            // Lateral hit → opposite arm lifts as dynamic counterweight.
            let contra = (-lat_hit * sign).max(0.0);
            (right * (sign * (0.55 + 0.35 * contra)) + up * (0.22 * contra - 0.50)).normalize()
        };
        let flare_rot = aim(ctx.rot(upper), ctx.rig.part(upper).axis_local, upper_dir);
        let anim_rot = ctx.rot(PuppetPart::SpineUpper) * ctx.targets.local[upper.index()];
        // Blend smoothly from the animated arm pose so feather-light nudges twitch
        // subtly while heavy hits recoil dramatically.
        let target_rot = anim_rot.slerp(flare_rot, arm_env).normalize();
        out.joint_weighted(
            upper,
            JointTarget::World(target_rot),
            2.6,
            1.0,
            priority::REACTION,
            1.0,
        );
    }
}

/// Momentum-driven arm windmilling to counter balance disturbances
/// (`ArmsWindmill`, inspired by Macchietto et al. 2009 / Pratt et al. 2006 /
/// Euphoria). Arms rotate in the true tipping-momentum plane with naturally
/// semi-bent twist-locked elbows so reaction torque opposes the fall, and
/// abduct laterally as a counterweight on side shoves.
#[derive(Clone, Debug)]
pub struct ArmsWindmill {
    pub duration: f32,
    /// Nominal revolutions per second (scales up automatically with tipping momentum).
    pub speed: f32,
    pub stiffness: f32,
    t: f32,
    phase_acc: f32,
}

impl Default for ArmsWindmill {
    fn default() -> Self {
        Self {
            duration: 1.15,
            speed: 1.45,
            stiffness: 1.65,
            t: 0.0,
            phase_acc: 0.0,
        }
    }
}

impl ArmsWindmill {
    pub const NAME: &'static str = "arms_windmill";
}

impl Behaviour for ArmsWindmill {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        self.t += ctx.dt;
        if self.t >= self.duration {
            return BehaviourStatus::Done;
        }
        let s = ctx.sense;
        let up = s.up;
        let fwd = s.heading;
        let right = ctx.right();
        let v_h = s.com_vel.reject_from(up);
        let speed = v_h.length();
        let l_h = s.angular_momentum.reject_from(up);
        let l_norm = l_h.length() / (ctx.rig.total_mass * ctx.rig.height).max(1.0);

        // Stop windmilling cleanly once landing impact is absorbed and solidly stable.
        if self.t > 0.45 && s.feet_down() == 2 && s.capture_margin < -0.01 && speed < 0.22 {
            return BehaviourStatus::Done;
        }
        let urgency = (((speed - 0.22) / 0.55)
            .max(s.capture_margin / (0.09 * ctx.rig.height))
            .max((l_norm - 0.15) / 0.40))
        .clamp(0.0, 1.0);
        if urgency <= 0.04 {
            return BehaviourStatus::Running;
        }
        let env = smooth(self.t / 0.14) * smooth((self.duration - self.t) / 0.28) * urgency;

        // Plane of disturbance: velocity direction (biased subtly by tipping angular momentum).
        let tip_from_l = l_h.cross(up);
        let raw_fall = if speed > 0.15 {
            v_h / speed + tip_from_l.normalize_or_zero() * (l_norm * 0.35).min(0.25)
        } else {
            fwd
        };
        let fall_dir = raw_fall.normalize_or(fwd);
        let side_component = fall_dir.dot(right);
        // Right-handed angular momentum conservation: to exert a restoring torque
        // opposing tipping (`ω_fall = up × fall_dir`), arms accelerate in `ω_fall`
        // (`-up → -fall_dir → +up → +fall_dir`).
        self.phase_acc += std::f32::consts::TAU * self.speed * ctx.dt;

        for side in [Side::Left, Side::Right] {
            let sign = if side == Side::Right { 1.0 } else { -1.0 };
            // Anti-phase (π offset) cancels linear forward/backward CoM kicks
            // while summing pure angular momentum opposing the fall!
            let phase = self.phase_acc + if side == Side::Right { std::f32::consts::PI } else { 0.0 };
            let lateral_flare = (0.48 + (-side_component * sign).max(0.0) * 0.45).min(0.90);
            let circle = -up * phase.cos() - fall_dir * phase.sin();
            let upper_dir = (circle * 0.72 + right * sign * lateral_flare + up * 0.08).normalize();
            let lower_dir = (upper_dir - fall_dir * 0.25 + up * 0.18).normalize();
            let upper = PuppetPart::upper_arm(side);
            let lower = PuppetPart::lower_arm(side);
            let ur = aim(ctx.rot(upper), ctx.rig.part(upper).axis_local, upper_dir);
            let lr = aim(ctx.rot(lower), ctx.rig.part(lower).axis_local, lower_dir);
            let w = env.max(1e-3);
            out.joint_weighted(upper, JointTarget::World(ur), self.stiffness, 1.0, priority::REACTION, w);
            out.joint_weighted(lower, JointTarget::World(lr), self.stiffness, 1.0, priority::REACTION, w);
        }
        BehaviourStatus::Running
    }
}
