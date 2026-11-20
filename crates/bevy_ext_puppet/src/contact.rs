//! Avian solves contacts before XPBD joints. During a fall the arm and neck
//! motors can move a limb back into its support before the pose is rendered.
//! Resolve that residual at the simulated pose against the actual level shape.

use avian3d::collision::collider::contact_query;
use avian3d::dynamics::joints::joint_graph::JointGraph;
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::rig::PuppetBody;
use crate::state::PuppetState;

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
