//! Ground contact for a fallen skinned character. The standing controller is
//! tuned for anatomical capsules, but a skull, belly or coat can extend well
//! beyond them. Fit the fall shapes once, when the rig is built.

use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::mesh::{
    VertexAttributeValues,
    skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
};
use bevy::prelude::*;

use crate::part::PART_COUNT;
use crate::rig::PuppetBody;
use crate::state::PuppetState;

#[derive(Component)]
pub(crate) struct FallCollider {
    /// Shape to exchange with the body's current collider at a mode change.
    alternate: Collider,
    fallen: bool,
}

#[derive(SystemParam)]
pub struct PuppetSkins<'w, 's> {
    skins: Query<'w, 's, (&'static Mesh3d, &'static SkinnedMesh)>,
    meshes: Option<Res<'w, Assets<Mesh>>>,
    bindposes: Option<Res<'w, Assets<SkinnedMeshInverseBindposes>>>,
}

impl PuppetSkins<'_, '_> {
    pub(crate) fn fit_fall_colliders(
        &self,
        puppet: Entity,
        bones: &[Entity; PART_COUNT],
        height: f32,
        children: &Query<&Children>,
        parents: &Query<&ChildOf>,
        world: impl Fn(Entity) -> Option<Affine3A>,
    ) -> [Option<FallCollider>; PART_COUNT] {
        let (Some(meshes), Some(bindposes)) = (&self.meshes, &self.bindposes) else {
            return Default::default();
        };
        let mut points: [Vec<Vec3>; PART_COUNT] = Default::default();
        for (mesh, skin) in self
            .skins
            .iter_many(children.iter_descendants(puppet))
            .flatten()
        {
            let (Some(mesh), Some(binds)) =
                (meshes.get(mesh), bindposes.get(&skin.inverse_bindposes))
            else {
                continue;
            };
            let (
                Ok(VertexAttributeValues::Float32x3(vertices)),
                Ok(VertexAttributeValues::Uint16x4(indices)),
                Ok(VertexAttributeValues::Float32x4(weights)),
            ) = (
                mesh.try_attribute(Mesh::ATTRIBUTE_POSITION),
                mesh.try_attribute(Mesh::ATTRIBUTE_JOINT_INDEX),
                mesh.try_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT),
            )
            else {
                continue;
            };

            // Finger, toe and twist bones belong to their nearest simulated
            // ancestor. Inverse bind matrices take mesh vertices into the
            // joint's frame; then express them in the unscaled Avian body.
            let frames: Vec<_> = skin
                .joints
                .iter()
                .zip(binds.iter())
                .map(|(&joint, bind)| {
                    let part = std::iter::successors(Some(joint), |e| {
                        parents.get(*e).ok().map(ChildOf::parent)
                    })
                    .find_map(|e| bones.iter().position(|&b| b == e))?;
                    let (_, rotation, position) =
                        world(bones[part])?.to_scale_rotation_translation();
                    let body = Affine3A::from_rotation_translation(rotation.normalize(), position);
                    let transform = body.inverse() * world(joint)? * Affine3A::from_mat4(*bind);
                    Some((part, transform))
                })
                .collect();
            for ((v, indices), weights) in vertices.iter().zip(indices).zip(weights) {
                for (&index, &weight) in indices.iter().zip(weights) {
                    // Include shared skin at an elbow or hip on both bodies.
                    // Tiny influences reach far outside the joint they affect.
                    if weight >= 0.1
                        && let Some(Some((part, transform))) = frames.get(index as usize)
                    {
                        let p = transform.transform_point3(Vec3::from(*v));
                        if p.is_finite() {
                            points[*part].push(p);
                        }
                    }
                }
            }
        }
        // A small rounded border allows for contact compression at impact.
        let border = 0.005 * height;
        points.map(|cloud| {
            if cloud.len() < 4 {
                return None;
            }
            let shape = avian3d::parry::shape::SharedShape::round_convex_hull(&cloud, border)?;
            Some(FallCollider {
                alternate: shape.into(),
                fallen: false,
            })
        })
    }
}

pub(crate) fn switch_fall_colliders(
    states: Query<&PuppetState>,
    mut parts: Query<(&PuppetBody, &mut FallCollider, &mut Collider)>,
) {
    for (body, mut shapes, mut collider) in &mut parts {
        let Ok(state) = states.get(body.puppet) else {
            continue;
        };
        let fallen = state.mode.uses_fall_contacts();
        if fallen != shapes.fallen {
            std::mem::swap(&mut *collider, &mut shapes.alternate);
            shapes.fallen = fallen;
        }
    }
}
