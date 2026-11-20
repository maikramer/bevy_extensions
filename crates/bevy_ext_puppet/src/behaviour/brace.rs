//! Spatial wall & obstacle bracing ([`WallBrace`], Euphoria `BraceForImpact`).
//!
//! When a stumbling or falling character approaches a steep wall, pillar, tree
//! trunk, or tall obstacle detected by [`crate::sense::PuppetSense::nearby_wall`],
//! `WallBrace`:
//! 1. Reaches one or both arms (depending on facing angle) using twist-locked
//!    arm IK (`solve_arm_ik`) with palms aligned flush against the surface normal.
//! 2. Pre-bends the elbows before impact so contact is absorbed compliantly.
//! 3. Upon hand/forearm contact, switches to eccentric high-damping absorption
//!    AND emits an implicit **arm [`VirtualForce`]** pushing cleanly back off
//!    the wall to arrest momentum and assist upright recovery!
//! 4. Protects the face/skull by turning the head slightly along the wall
//!    tangent (or tucking the chin forward when stumbling backwards).

use bevy::prelude::*;

use super::{
    Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, PuppetSignal,
    VirtualForce, priority,
};
use crate::ik::{align_palm, solve_arm_ik};
use crate::part::{PuppetPart, Side};
use crate::sense::SurfaceProbe;

/// Automatically braces against nearby walls and obstacles during stumbles or falls.
#[derive(Clone, Debug, Default)]
pub struct WallBrace {
    cached_wall: Option<SurfaceProbe>,
    lost_sight: f32,
    contact_time: f32,
    signaled_contact: bool,
}

impl WallBrace {
    pub const NAME: &'static str = "wall_brace";
}

impl Behaviour for WallBrace {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        let s = ctx.sense;
        let rig = ctx.rig;
        let h = rig.height;
        let up = s.up;

        // Refresh wall probe or coast briefly if ray momentarily slips while bracing.
        if let Some(probe) = s.nearby_wall {
            self.cached_wall = Some(probe);
            self.lost_sight = 0.0;
        } else {
            self.lost_sight += ctx.dt;
            if self.lost_sight > 0.25 {
                return BehaviourStatus::Done;
            }
        }
        let Some(wall) = self.cached_wall else {
            return BehaviourStatus::Done;
        };

        let chest_pos = ctx.pos(PuppetPart::SpineUpper);
        let wall_n = wall.normal.normalize_or(-s.heading);
        // Signed distance from chest to the wall plane along `+wall_n`.
        let dist_to_plane = (chest_pos - wall.point).dot(wall_n);
        let v_into = s.com_vel.dot(-wall_n);
        let any_arm_touch = [Side::Left, Side::Right].iter().any(|&side| {
            let hp = PuppetPart::hand(side);
            let lp = PuppetPart::lower_arm(side);
            (s.touching(hp) && s.contact_normal(hp).dot(wall_n) > 0.35)
                || (s.touching(lp) && s.contact_normal(lp).dot(wall_n) > 0.35)
        });

        // Disengage cleanly once safely moving away from the wall or out of reach.
        if !any_arm_touch && (dist_to_plane > 1.35 || (dist_to_plane > 0.65 && v_into < 0.15)) {
            return BehaviourStatus::Done;
        }

        if any_arm_touch {
            self.contact_time += ctx.dt;
        } else {
            self.contact_time = (self.contact_time - ctx.dt * 2.0).max(0.0);
        }

        // Wall horizontal tangent pointing to the character's right when facing the wall.
        let wall_right = up.cross(wall_n).normalize_or(ctx.right());
        let facing_wall = s.heading.dot(-wall_n); // +1 facing straight into wall
        let right_to_wall = ctx.right().dot(-wall_n); // +1 right shoulder toward wall

        // Choose bracing arms based on orientation relative to wall.
        let (use_left, use_right) = if facing_wall > 0.22 || facing_wall < -0.28 {
            (true, true)
        } else if right_to_wall > 0.20 {
            (false, true)
        } else if right_to_wall < -0.20 {
            (true, false)
        } else {
            (true, true)
        };

        // Eccentric shock absorption on contact: taper stiffness slightly while
        // keeping damping high so the arms cushion without rebounding violently.
        let (arm_k, arm_d) = if any_arm_touch {
            ((3.2 - 3.0 * self.contact_time).clamp(1.6, 3.2), 1.80)
        } else {
            (3.4, 1.05)
        };

        let mut active_braces = 0u8;
        for (side, active) in [(Side::Left, use_left), (Side::Right, use_right)] {
            if !active {
                continue;
            }
            let sign = if side == Side::Right { 1.0 } else { -1.0 };
            let upper = PuppetPart::upper_arm(side);
            let lower = PuppetPart::lower_arm(side);
            let hand = PuppetPart::hand(side);
            let urig = rig.part(upper);
            let lrig = rig.part(lower);
            let hrig = rig.part(hand);

            let shoulder = ctx.pos(upper);
            let max_reach = (urig.length + lrig.length) * 0.90;
            // Project shoulder onto wall plane + slight lateral spread so hands don't collide.
            let proj_on_wall = shoulder - wall_n * ((shoulder - wall.point).dot(wall_n) - 0.02);
            let spread = if facing_wall >= 0.0 {
                wall_right * (sign * 0.06 * h)
            } else {
                // Backward into wall: spread arms wider so shoulders stay in safe cone!
                wall_right * (-sign * 0.16 * h) - up * (0.08 * h)
            };
            let mut target = proj_on_wall + spread + up * (0.03 * h);
            // Clamp inside slightly pre-bent arm reach (never lock elbows straight!).
            let from_shoulder = target - shoulder;
            if from_shoulder.length() > max_reach {
                target = shoulder + from_shoulder.normalize() * max_reach;
            }

            // Elbow tip bows outward and slightly downward for strong push-off leverage.
            let elbow_pole = (wall_n * 0.45 + ctx.right() * (sign * 0.72) - up * 0.48).normalize();
            let (ur, lr, _) = solve_arm_ik(
                shoulder,
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
            let hr = align_palm(ctx.rot(hand), hrig.axis_local, hrig.up_local, wall_n, up);
            out.joint_weighted(upper, JointTarget::World(ur), arm_k, arm_d, priority::PROTECT, 1.0);
            out.joint_weighted(
                lower,
                JointTarget::Local((ur.inverse() * lr).normalize()),
                arm_k,
                arm_d,
                priority::PROTECT,
                1.0,
            );
            out.joint_weighted(hand, JointTarget::World(hr), 1.6, 1.3, priority::PROTECT, 1.0);

            // Check wall contact on this arm → emit arm VirtualForce push-off!
            let hand_touch = s.touching(hand) && s.contact_normal(hand).dot(wall_n) > 0.30;
            let fore_touch = s.touching(lower) && s.contact_normal(lower).dot(wall_n) > 0.30;
            if hand_touch || fore_touch {
                active_braces += 1;
                if !self.signaled_contact {
                    self.signaled_contact = true;
                    let hit_part = if hand_touch { hand } else { lower };
                    out.signal(PuppetSignal::WallBraced {
                        part: hit_part,
                        point: s.contact_points[hit_part.index()],
                        normal: wall_n,
                    });
                }
                // Virtual push-off force away from wall: arrests closing speed AND gently
                // rebounds the torso back toward upright stance!
                let mg = s.total_mass * s.g.max(9.81);
                let push_mag = ((0.18 + 0.24 * v_into.max(0.0)) * mg).clamp(0.0, 0.42 * mg);
                let lift_mag = (0.08 * mg).min(push_mag * 0.4);
                out.virtual_forces.push(VirtualForce::arm(
                    side,
                    wall_n * push_mag + up * lift_mag,
                    Vec3::ZERO,
                    shoulder,
                    0.5,
                ));
            }
        }

        // ---- Head protection: look toward wall initially, turn cheek / tuck chin close up ----
        let head = PuppetPart::Head;
        let chest_rot = ctx.rot(PuppetPart::SpineUpper);
        let anim_head = chest_rot * ctx.targets.local[head.index()];
        if facing_wall < -0.20 {
            // Stumbling backward into wall: tuck chin tightly forward so occiput doesn't hit!
            let right_local = (chest_rot.inverse() * ctx.right()).normalize_or(Vec3::X);
            let tuck = Quat::from_axis_angle(right_local, -0.62) * ctx.targets.local[head.index()];
            out.joint(head, JointTarget::Local(tuck.normalize()), 2.8, priority::PROTECT);
        } else if dist_to_plane < 0.55 || active_braces > 0 {
            // Close to wall facing it: turn head gently sideways to protect nose/forehead!
            let turn_sign = if right_to_wall >= 0.0 { -1.0 } else { 1.0 };
            let protect_rot = (Quat::from_axis_angle(up, turn_sign * 0.55) * anim_head).normalize();
            out.joint(head, JointTarget::World(protect_rot), 2.5, priority::PROTECT);
        } else {
            out.look_at(wall.point, priority::PROTECT - 2, 0.35);
        }

        BehaviourStatus::Running
    }
}
