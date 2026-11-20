//! A procedural humanoid skeleton with the AiGameKit (UE mannequin) bone
//! names, for tests, the sandbox and as a reference of what the rig builder
//! expects. Standing A-pose, facing `-Z`, feet on `y = 0`, 1.8 m tall.

use bevy::prelude::*;

/// Bones of the test skeleton: `(name, parent index, position in the model)`.
pub const TEST_SKELETON: &[(&str, Option<usize>, Vec3)] = &[
    ("pelvis", None, Vec3::new(0.0, 0.98, 0.0)),
    ("spine_01", Some(0), Vec3::new(0.0, 1.08, 0.0)),
    ("spine_02", Some(1), Vec3::new(0.0, 1.20, 0.0)),
    ("spine_03", Some(2), Vec3::new(0.0, 1.32, 0.0)),
    ("neck_01", Some(3), Vec3::new(0.0, 1.50, 0.0)),
    ("Head", Some(4), Vec3::new(0.0, 1.58, 0.0)),
    ("clavicle_l", Some(3), Vec3::new(-0.05, 1.44, 0.0)),
    ("upperarm_l", Some(6), Vec3::new(-0.19, 1.42, 0.0)),
    ("lowerarm_l", Some(7), Vec3::new(-0.27, 1.15, 0.0)),
    ("hand_l", Some(8), Vec3::new(-0.33, 0.90, 0.0)),
    ("clavicle_r", Some(3), Vec3::new(0.05, 1.44, 0.0)),
    ("upperarm_r", Some(10), Vec3::new(0.19, 1.42, 0.0)),
    ("lowerarm_r", Some(11), Vec3::new(0.27, 1.15, 0.0)),
    ("hand_r", Some(12), Vec3::new(0.33, 0.90, 0.0)),
    ("thigh_l", Some(0), Vec3::new(-0.10, 0.93, 0.0)),
    ("calf_l", Some(14), Vec3::new(-0.11, 0.51, 0.01)),
    ("foot_l", Some(15), Vec3::new(-0.12, 0.08, 0.02)),
    ("ball_l", Some(16), Vec3::new(-0.12, 0.025, -0.12)),
    ("thigh_r", Some(0), Vec3::new(0.10, 0.93, 0.0)),
    ("calf_r", Some(18), Vec3::new(0.11, 0.51, 0.01)),
    ("foot_r", Some(19), Vec3::new(0.12, 0.08, 0.02)),
    ("ball_r", Some(20), Vec3::new(0.12, 0.025, -0.12)),
];

/// Spawns [`TEST_SKELETON`] under `root` (bones with identity rotations,
/// positions relative to their parents). Returns the bone entities in order.
pub fn spawn_test_skeleton(commands: &mut Commands, root: Entity) -> Vec<Entity> {
    let mut out: Vec<Entity> = Vec::with_capacity(TEST_SKELETON.len());
    for (name, parent, pos) in TEST_SKELETON {
        let (parent_entity, parent_pos) = match parent {
            Some(p) => (out[*p], TEST_SKELETON[*p].2),
            None => (root, Vec3::ZERO),
        };
        let e = commands
            .spawn((
                Name::new(*name),
                Transform::from_translation(*pos - parent_pos),
                ChildOf(parent_entity),
            ))
            .id();
        out.push(e);
    }
    out
}

/// Same as [`spawn_test_skeleton`] on a `World`.
pub fn spawn_test_skeleton_world(world: &mut World, root: Entity) -> Vec<Entity> {
    let mut out: Vec<Entity> = Vec::with_capacity(TEST_SKELETON.len());
    for (name, parent, pos) in TEST_SKELETON {
        let (parent_entity, parent_pos) = match parent {
            Some(p) => (out[*p], TEST_SKELETON[*p].2),
            None => (root, Vec3::ZERO),
        };
        let e = world
            .spawn((
                Name::new(*name),
                Transform::from_translation(*pos - parent_pos),
                ChildOf(parent_entity),
            ))
            .id();
        out.push(e);
    }
    out
}
