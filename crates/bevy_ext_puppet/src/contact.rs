//! Avian solves contacts before XPBD joints. During a fall the arm and neck
//! motors can move a limb back into its support before the pose is rendered.
//! Resolve that residual at the simulated pose against the actual level shape.

use avian3d::collision::collider::contact_query;
use avian3d::dynamics::joints::joint_graph::JointGraph;
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::part::PuppetPart;
use crate::rig::{PuppetBody, PuppetRig};
use crate::state::PuppetState;

/// Maximum allowed distance (m) between a child body's origin and its parent's
/// joint anchor after the physics step. Stays above legitimate fall-collider
/// floor-contact lifts while catching any catastrophic joint dislocation or
/// floor-tunneling blow-up before it can run away.
const MAX_JOINT_SEPARATION: f32 = 0.25;
/// Hard linear speed ceiling (m/s) applied only when a violent impact or
/// solver ejection spikes a puppet limb to supersonic speed.
const MAX_BODY_LINEAR_SPEED: f32 = 30.0;

/// Post-step safety pass: uses read-only queries in the fast path (zero
/// change-detection churn during normal motion) and only writes back via
/// [`Commands`] if a body becomes non-finite, spikes past
/// [`MAX_BODY_LINEAR_SPEED`], or tears past [`MAX_JOINT_SEPARATION`] from its
/// parent joint socket.
#[allow(clippy::type_complexity)]
pub(crate) fn clamp_joint_separation(
    mut commands: Commands,
    puppets: Query<&PuppetRig>,
    bodies: Query<(&Position, &Rotation, &LinearVelocity, &AngularVelocity), With<PuppetBody>>,
) {
    let max_speed_sq = MAX_BODY_LINEAR_SPEED * MAX_BODY_LINEAR_SPEED;
    for rig in &puppets {
        let mut solved_pos = [Vec3::ZERO; crate::part::PART_COUNT];
        let mut solved_rot = [Quat::IDENTITY; crate::part::PART_COUNT];
        let mut solved_vel = [Vec3::ZERO; crate::part::PART_COUNT];

        // PuppetPart::ALL is ordered parents-first.
        for part in PuppetPart::ALL {
            let i = part.index();
            let body = rig.parts[i].body;
            let Ok((pos, rot, lin, ang)) = bodies.get(body) else {
                continue;
            };
            let mut p = pos.0;
            let mut r = rot.0;
            let mut lv = lin.0;
            let mut av = ang.0;
            let mut dirty = false;

            if let Some(parent_part) = part.parent() {
                let pi = parent_part.index();
                let anchor = solved_pos[pi] + solved_rot[pi] * rig.parts[i].anchor_in_parent;
                if !p.is_finite() || !r.is_finite() {
                    p = anchor;
                    r = solved_rot[pi];
                    lv = solved_vel[pi];
                    av = Vec3::ZERO;
                    dirty = true;
                } else {
                    if !lv.is_finite() {
                        lv = solved_vel[pi];
                        dirty = true;
                    } else if lv.length_squared() > max_speed_sq {
                        lv = lv.clamp_length_max(MAX_BODY_LINEAR_SPEED);
                        dirty = true;
                    }
                    if !av.is_finite() {
                        av = Vec3::ZERO;
                        dirty = true;
                    }
                    let diff = p - anchor;
                    let dist = diff.length();
                    if dist > MAX_JOINT_SEPARATION {
                        let dir = diff / dist;
                        p = anchor + dir * MAX_JOINT_SEPARATION;
                        let rel_v = lv - solved_vel[pi];
                        let outward = rel_v.dot(dir);
                        if outward > 0.0 {
                            lv -= dir * outward;
                        }
                        dirty = true;
                    }
                }
            } else {
                if !p.is_finite() {
                    p = Vec3::ZERO;
                    lv = Vec3::ZERO;
                    dirty = true;
                }
                if !r.is_finite() {
                    r = Quat::IDENTITY;
                    av = Vec3::ZERO;
                    dirty = true;
                }
                if !lv.is_finite() {
                    lv = Vec3::ZERO;
                    dirty = true;
                } else if lv.length_squared() > max_speed_sq {
                    lv = lv.clamp_length_max(MAX_BODY_LINEAR_SPEED);
                    dirty = true;
                }
                if !av.is_finite() {
                    av = Vec3::ZERO;
                    dirty = true;
                }
            }

            if dirty {
                commands
                    .entity(body)
                    .insert((Position(p), Rotation(r), LinearVelocity(lv), AngularVelocity(av)));
            }
            solved_pos[i] = p;
            solved_rot[i] = r;
            solved_vel[i] = lv;
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn resolve_floor_overlap(
    mut commands: Commands,
    spatial: SpatialQuery,
    gravity: Res<Gravity>,
    joints: Res<JointGraph>,
    parts: Query<
        (
            Entity,
            &PuppetBody,
            &Position,
            &Rotation,
            &Collider,
            &CollisionLayers,
        ),
        (
            Without<Sensor>,
            Without<ColliderDisabled>,
            Without<RigidBodyDisabled>,
            Without<ActiveCollisionHooks>,
        ),
    >,
    states: Query<&PuppetState>,
    surfaces: Query<
        (
            &Position,
            &Rotation,
            &Collider,
            &CollisionLayers,
            Option<&ColliderOf>,
        ),
        (
            Without<PuppetBody>,
            Without<Sensor>,
            Without<ColliderDisabled>,
            Without<ActiveCollisionHooks>,
        ),
    >,
    bodies: Query<&RigidBody, Without<RigidBodyDisabled>>,
) {
    let Some(up) = (-gravity.0).try_normalize() else {
        return;
    };
    for (entity, part, pos, rot, shape, layers) in &parts {
        let Ok(state) = states.get(part.puppet) else {
            continue;
        };
        if !state.mode.uses_fall_contacts() {
            continue;
        }
        let mut corrected = pos.0;
        spatial.aabb_intersections_with_aabb_callback(shape.aabb(pos.0, rot.0, 0.0), |other| {
            let Ok((p, r, floor, other_layers, owner)) = surfaces.get(other) else {
                return true;
            };
            if !layers.interacts_with(*other_layers) {
                return true;
            }
            if let Some(owner) = owner {
                let Ok(surface_body) = bodies.get(owner.body) else {
                    return true;
                };
                if surface_body.is_dynamic()
                    || joints
                        .joints_between(entity, owner.body)
                        .any(|j| j.collision_disabled)
                {
                    return true;
                }
            }
            if let Ok(Some(hit)) =
                contact_query::contact(shape, corrected, rot.0, floor, p.0, r.0, 0.0)
            {
                let normal = r.0 * hit.local_normal2;
                if hit.penetration > 0.0 && normal.dot(up) > 0.0 {
                    corrected += normal * hit.penetration;
                }
            }
            true
        });
        if corrected != pos.0 {
            commands.entity(entity).insert(Position(corrected));
        }
    }
}
