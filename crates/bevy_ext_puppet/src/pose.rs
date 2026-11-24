//! Animation ⇄ physics pose exchange.
//!
//! * [`capture_targets`] runs after the animation and before transform
//!   propagation: it reads the animated bone locals and composes the target
//!   world pose of every part (what the muscles and pins chase).
//! * [`write_back_pose`] then blends the animated pose with the simulated
//!   bodies by [`PuppetBlend`] and writes the result into the bone locals.
//!
//! Bones that no animation drives keep the value we wrote last frame; the
//! capture recognises each channel it wrote and keeps the last
//! animated local instead, so an unanimated rig still has its rest pose as
//! target instead of chasing its own physics.

use avian3d::prelude::*;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::part::{PART_COUNT, PuppetPart};
use crate::rig::PuppetRig;
use crate::state::PuppetState;

/// Target (animated) pose of every part, in world space.
#[derive(Component, Clone, Debug)]
pub struct PuppetTargets {
    pub world_pos: [Vec3; PART_COUNT],
    pub world_rot: [Quat; PART_COUNT],
    /// Target rotation of each part relative to its parent part.
    pub local: [Quat; PART_COUNT],
    /// Target velocities (finite differences between captures).
    pub lin_vel: [Vec3; PART_COUNT],
    pub ang_vel: [Vec3; PART_COUNT],
    pub(crate) prev_pos: [Vec3; PART_COUNT],
    pub(crate) prev_rot: [Quat; PART_COUNT],
    pub(crate) anim_local: [Transform; PART_COUNT],
    pub(crate) written: [Option<Transform>; PART_COUNT],
    pub(crate) global_scale: [Vec3; PART_COUNT],
    pub(crate) initialized: bool,
}

impl Default for PuppetTargets {
    fn default() -> Self {
        Self {
            world_pos: [Vec3::ZERO; PART_COUNT],
            world_rot: [Quat::IDENTITY; PART_COUNT],
            local: [Quat::IDENTITY; PART_COUNT],
            lin_vel: [Vec3::ZERO; PART_COUNT],
            ang_vel: [Vec3::ZERO; PART_COUNT],
            prev_pos: [Vec3::ZERO; PART_COUNT],
            prev_rot: [Quat::IDENTITY; PART_COUNT],
            anim_local: [Transform::IDENTITY; PART_COUNT],
            written: [None; PART_COUNT],
            global_scale: [Vec3::ONE; PART_COUNT],
            initialized: false,
        }
    }
}

impl PuppetTargets {
    /// Recover channels still owned by the previous physics write. A clip
    /// can key rotation without touching translation (or vice versa).
    pub(crate) fn animated_local(&self, part: PuppetPart, mut current: Transform) -> Transform {
        let i = part.index();
        if let Some(written) = self.written[i] {
            if current.rotation == written.rotation {
                current.rotation = self.anim_local[i].rotation;
            }
            if current.translation == written.translation {
                current.translation = self.anim_local[i].translation;
            }
        }
        current
    }

    #[inline]
    pub fn pos(&self, part: PuppetPart) -> Vec3 {
        self.world_pos[part.index()]
    }
    #[inline]
    pub fn rot(&self, part: PuppetPart) -> Quat {
        self.world_rot[part.index()]
    }
}

/// How much of each part the player sees from physics (1) versus animation (0).
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Default)]
pub struct PuppetBlend {
    pub physics_weight: [f32; PART_COUNT],
}

impl Default for PuppetBlend {
    fn default() -> Self {
        Self {
            physics_weight: [0.0; PART_COUNT],
        }
    }
}

impl PuppetBlend {
    pub fn any(&self) -> bool {
        self.physics_weight.iter().any(|w| *w > 1e-3)
    }
}

/// World affine of the puppet root (its parent's global × its local).
fn root_affine(
    root: Entity,
    local: Option<&Transform>,
    parents: &Query<&ChildOf>,
    gts: &Query<&GlobalTransform>,
) -> Affine3A {
    let local = local.map(|t| t.compute_affine()).unwrap_or_default();
    match parents.get(root).ok().and_then(|p| gts.get(p.parent()).ok()) {
        Some(parent) => parent.affine() * local,
        None => local,
    }
}

/// Captures the animated target pose of every built puppet.
pub fn capture_targets(
    time: Res<Time>,
    mut puppets: Query<(Entity, &PuppetRig, &mut PuppetTargets)>,
    transforms: Query<&Transform>,
    parents: Query<&ChildOf>,
    gts: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    for (root, rig, mut targets) in &mut puppets {
        let targets = &mut *targets;
        // Pass 1: the animated local of every part bone.
        for part in PuppetPart::ALL {
            let i = part.index();
            let Ok(t) = transforms.get(rig.parts[i].bone) else { continue };
            targets.anim_local[i] = targets.animated_local(part, *t);
        }
        // Pass 2: compose world targets along each bone path.
        let root_aff = root_affine(root, transforms.get(root).ok(), &parents, &gts);
        for part in PuppetPart::ALL {
            let i = part.index();
            let mut aff = root_aff;
            for (e, tag) in &rig.parts[i].path {
                let local = match tag {
                    Some(p) => targets.anim_local[p.index()],
                    None => transforms.get(*e).copied().unwrap_or_default(),
                };
                aff *= local.compute_affine();
            }
            let (s, r, t) = aff.to_scale_rotation_translation();
            targets.global_scale[i] = s;
            targets.world_pos[i] = t;
            targets.world_rot[i] = r.normalize();
        }
        for part in PuppetPart::ALL {
            let i = part.index();
            if let Some(p) = part.parent() {
                targets.local[i] =
                    (targets.world_rot[p.index()].inverse() * targets.world_rot[i]).normalize();
            }
            if dt > 0.0 && targets.initialized {
                let raw_lin = (targets.world_pos[i] - targets.prev_pos[i]) / dt;
                targets.lin_vel[i] = if raw_lin.is_finite() {
                    raw_lin.clamp_length_max(20.0)
                } else {
                    Vec3::ZERO
                };
                let dq = (targets.world_rot[i] * targets.prev_rot[i].inverse()).normalize();
                let raw_ang = shortest(dq).to_scaled_axis() / dt;
                targets.ang_vel[i] = if raw_ang.is_finite() {
                    raw_ang.clamp_length_max(30.0)
                } else {
                    Vec3::ZERO
                };
            }
            targets.prev_pos[i] = targets.world_pos[i];
            targets.prev_rot[i] = targets.world_rot[i];
        }
        targets.initialized = true;
    }
}

/// The same rotation with a non-negative `w` (the short way round).
#[inline]
pub fn shortest(q: Quat) -> Quat {
    if q.w < 0.0 { -q } else { q }
}

/// Writes the blended physics pose into the bones.
#[allow(clippy::type_complexity)]
pub fn write_back_pose(
    mut puppets: Query<(
        Entity,
        &PuppetRig,
        &mut PuppetTargets,
        &PuppetBlend,
        Option<&PuppetState>,
    )>,
    bodies: Query<(&Position, &Rotation)>,
    mut transforms: Query<&mut Transform>,
    parents: Query<&ChildOf>,
    gts: Query<&GlobalTransform>,
) {
    for (root, rig, mut targets, blend, state) in &mut puppets {
        if !blend.any() {
            // Hand the bones back to the animation (an unanimated rig would
            // otherwise keep the last physical pose).
            for part in PuppetPart::ALL {
                let i = part.index();
                if targets.written[i].is_some() {
                    if let Ok(mut tf) = transforms.get_mut(rig.parts[i].bone) {
                        let animated = targets.animated_local(part, *tf);
                        tf.set_if_neq(animated);
                    }
                    targets.written[i] = None;
                }
            }
            continue;
        }
        let fall_contacts = state.is_some_and(|s| s.mode.uses_fall_contacts());
        let root_local = transforms.get(root).ok().copied();
        let root_aff = root_affine(root, root_local.as_ref(), &parents, &gts);
        for part in PuppetPart::ALL {
            let i = part.index();
            let w = blend.physics_weight[i].clamp(0.0, 1.0);
            let Ok((pos, rot)) = bodies.get(rig.parts[i].body) else { continue };
            let body_pos = if pos.0.is_finite() {
                pos.0
            } else {
                targets.world_pos[i]
            };
            let body_rot = if rot.0.is_finite() {
                rot.0.normalize()
            } else {
                targets.world_rot[i]
            };
            let want_rot = targets.world_rot[i].slerp(body_rot, w).normalize();
            let want_pos = targets.world_pos[i].lerp(body_pos, w);
            // Parent affine from the (possibly just rewritten) locals.
            let path = &rig.parts[i].path;
            let mut parent_aff = root_aff;
            for (e, _) in &path[..path.len().saturating_sub(1)] {
                if let Ok(t) = transforms.get(*e) {
                    parent_aff *= t.compute_affine();
                }
            }
            let want = Affine3A::from_scale_rotation_translation(
                targets.global_scale[i],
                want_rot,
                want_pos,
            );
            let (_, r, t) = (parent_aff.inverse() * want).to_scale_rotation_translation();
            let Ok(mut tf) = transforms.get_mut(rig.parts[i].bone) else { continue };
            if r.is_finite() {
                tf.rotation = r.normalize();
            }
            // Follow the solved anchors as well as their rotations, while
            // respecting bone lengths so heavy hits never stretch the skinned
            // mesh like elastic rubber.
            // - While standing/active (`!fall_contacts`), lock closely to the
            //   anatomical bone length (≤ 2.5 cm).
            // - During falls/ragdoll (`fall_contacts`), allow up to 22 cm of
            //   socket compliance so `FallCollider` / `resolve_floor_overlap`
            //   keeps bulky skinned limbs (gloves, boots) above ramps/floors,
            //   while still strictly forbidding infinite elastic stretching.
            if part == PuppetPart::Pelvis {
                if t.is_finite() {
                    tf.translation = t;
                }
            } else {
                let anim_t = targets.anim_local[i].translation;
                let rest_len = anim_t.length();
                let physical = parent_aff.inverse().transform_point3(body_pos);
                let bounded = if physical.is_finite() {
                    if fall_contacts {
                        anim_t + (physical - anim_t).clamp_length_max(0.22)
                    } else {
                        let max_dev = (rest_len * 0.06).clamp(0.005, 0.025);
                        let max_len = rest_len * 1.05 + 0.01;
                        (anim_t + (physical - anim_t).clamp_length_max(max_dev))
                            .clamp_length_max(max_len)
                    }
                } else {
                    anim_t
                };
                tf.translation = anim_t.lerp(bounded, w);
            }
            targets.written[i] = Some(*tf);
        }
    }
}
