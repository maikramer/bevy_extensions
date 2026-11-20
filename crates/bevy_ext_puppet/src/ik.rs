//! Small geometric helpers for behaviours: two-bone IK and aiming a body
//! segment along a direction.

use bevy::prelude::*;

/// Position of the middle joint (knee/elbow) of a two-bone chain from `root`
/// to `target` with segment lengths `l1`, `l2`, bending towards `pole`.
/// Unreachable targets stretch the chain straight towards them.
pub fn two_bone_mid(root: Vec3, target: Vec3, l1: f32, l2: f32, pole: Vec3) -> Vec3 {
    let to = target - root;
    let dist = to.length();
    let dir = if dist > 1e-5 { to / dist } else { Vec3::NEG_Y };
    let d = dist.clamp((l1 - l2).abs() + 1e-3, l1 + l2 - 1e-3);
    let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();
    let mut perp = pole - dir * pole.dot(dir);
    if perp.length_squared() < 1e-8 {
        perp = dir.any_orthonormal_vector();
    }
    let perp = perp.normalize();
    root + (dir * cos_a + perp * sin_a) * l1
}

/// Rotation of a body whose local axis `axis_local` points along `dir`,
/// reached from `current` by the smallest rotation (keeps the twist).
pub fn aim(current: Quat, axis_local: Vec3, dir: Vec3) -> Quat {
    let from = (current * axis_local).normalize_or(Vec3::Y);
    let to = dir.normalize_or(from);
    (Quat::from_rotation_arc(from, to) * current).normalize()
}

/// Rotation of a body that aligns its local primary axis `axis_local` with
/// `dir` and its local secondary direction `forward_local` with the component
/// of `forward` orthogonal to `dir`. Unlike [`aim`], this eliminates axial
/// twist drift so hinge joints downstream (knees, elbows) stay aligned with
/// the intended bending plane.
pub fn aim_with_forward(
    fallback: Quat,
    axis_local: Vec3,
    forward_local: Vec3,
    dir: Vec3,
    forward: Vec3,
) -> Quat {
    let Some(a_loc) = axis_local.try_normalize() else {
        return aim(fallback, axis_local, dir);
    };
    let f_loc_raw = forward_local - a_loc * forward_local.dot(a_loc);
    let Some(f_loc) = f_loc_raw.try_normalize() else {
        return aim(fallback, axis_local, dir);
    };
    let Some(a_w) = dir.try_normalize() else {
        return fallback;
    };
    let f_w_raw = forward - a_w * forward.dot(a_w);
    let Some(f_w) = f_w_raw.try_normalize() else {
        return aim(fallback, axis_local, dir);
    };
    let r_loc = a_loc.cross(f_loc);
    let r_w = a_w.cross(f_w);
    let m_loc = Mat3::from_cols(a_loc, f_loc, r_loc);
    let m_w = Mat3::from_cols(a_w, f_w, r_w);
    Quat::from_mat3(&(m_w * m_loc.transpose())).normalize()
}

/// `current` turned (smallest rotation) so its local `up_local` matches `up`.
pub fn level(current: Quat, up_local: Vec3, up: Vec3) -> Quat {
    aim(current, up_local, up)
}

/// Twist-locked two-bone arm IK that guarantees the elbow's revolute hinge
/// plane stays aligned with the `(shoulder, elbow, target)` triangle.
///
/// `elbow_pole` points roughly where the **elbow tip** should bend (e.g.
/// backward and outward for catching a fall or bracing a wall). Because the
/// human elbow flexes *forward* (`Flexion::Forward`, toward `forward_local`),
/// the inner crease of the elbow aligns with `-elbow_pole`, preventing axial
/// twist drift that would lock the revolute elbow hinge sideways.
///
/// Returns `(upper_world_rot, lower_world_rot, elbow_world_pos)`.
#[allow(clippy::too_many_arguments)]
pub fn solve_arm_ik(
    shoulder: Vec3,
    target: Vec3,
    l1: f32,
    l2: f32,
    elbow_pole: Vec3,
    upper_axis_local: Vec3,
    upper_forward_local: Vec3,
    lower_axis_local: Vec3,
    lower_forward_local: Vec3,
    fallback_upper: Quat,
    fallback_lower: Quat,
) -> (Quat, Quat, Vec3) {
    let elbow = two_bone_mid(shoulder, target, l1, l2, elbow_pole);
    let upper_dir = (elbow - shoulder).normalize_or(Vec3::NEG_Y);
    let lower_dir = (target - elbow).normalize_or(upper_dir);
    // Inside of the elbow (flexion direction) lies strictly in the
    // `(shoulder, elbow, target)` plane, pointing from the upper segment toward
    // the forearm — never flips across `elbow_pole` singularities!
    let bend_in = lower_dir - upper_dir * lower_dir.dot(upper_dir);
    let upper_fwd = if bend_in.length_squared() > 1e-4 {
        bend_in.normalize()
    } else {
        (-elbow_pole)
            .reject_from(upper_dir)
            .try_normalize()
            .unwrap_or_else(|| upper_dir.any_orthonormal_vector())
    };
    let hinge_w = upper_dir
        .cross(upper_fwd)
        .try_normalize()
        .unwrap_or_else(|| upper_dir.any_orthonormal_vector());
    let lower_fwd = hinge_w.cross(lower_dir).normalize_or(upper_fwd);
    let upper_rot = aim_with_forward(
        fallback_upper,
        upper_axis_local,
        upper_forward_local,
        upper_dir,
        upper_fwd,
    );
    let lower_rot = aim_with_forward(
        fallback_lower,
        lower_axis_local,
        lower_forward_local,
        lower_dir,
        lower_fwd,
    );
    (upper_rot, lower_rot, elbow)
}

/// Aligns a hand so its fingers point roughly along `finger_dir` (projected
/// tangent to the surface) and its wrist/palm stays flat against a surface
/// with outward normal `surface_normal`.
pub fn align_palm(
    fallback: Quat,
    axis_local: Vec3,
    up_local: Vec3,
    surface_normal: Vec3,
    finger_dir: Vec3,
) -> Quat {
    let n = surface_normal.normalize_or(Vec3::Y);
    let tangent = finger_dir.reject_from(n);
    let fingers = if tangent.length_squared() > 1e-4 {
        tangent.normalize()
    } else {
        n.any_orthonormal_vector()
    };
    aim_with_forward(fallback, axis_local, up_local, fingers, n)
}

/// Smoothstep on `[0, 1]`.
#[inline]
pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smootherstep (C² continuous) on `[0, 1]` for organic neuromuscular onset/fade.
#[inline]
pub fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_bone_keeps_lengths_and_bends_to_pole() {
        let root = Vec3::new(0.0, 1.0, 0.0);
        let target = Vec3::new(0.0, 0.2, 0.0);
        let mid = two_bone_mid(root, target, 0.45, 0.45, Vec3::NEG_Z);
        assert!(((mid - root).length() - 0.45).abs() < 1e-4);
        assert!(((target - mid).length() - 0.45).abs() < 1e-3);
        assert!(mid.z < 0.0, "knee must bend towards the pole: {mid:?}");
    }

    #[test]
    fn aim_points_the_axis() {
        let q = aim(Quat::IDENTITY, Vec3::NEG_Y, Vec3::X);
        assert!((q * Vec3::NEG_Y - Vec3::X).length() < 1e-4);
    }

    #[test]
    fn aim_with_forward_eliminates_axial_twist() {
        // Start from a badly twisted fallback quaternion (90 deg around Y).
        let twisted = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let dir = Vec3::new(0.0, -0.8, 0.6).normalize();
        let q = aim_with_forward(twisted, Vec3::NEG_Y, Vec3::NEG_Z, dir, Vec3::NEG_Z);
        assert!((q * Vec3::NEG_Y - dir).length() < 1e-4);
        // Local forward must lie in the (dir, -Z) plane with zero X component.
        let fwd_w = q * Vec3::NEG_Z;
        assert!(fwd_w.x.abs() < 1e-4, "expected zero lateral twist, got {fwd_w:?}");
        assert!(fwd_w.dot(Vec3::NEG_Z) > 0.0);
    }

    #[test]
    fn arm_ik_aligns_elbow_hinge_plane() {
        // Arm reaching forward (-Z) and down, elbow tip bent backward (+Z).
        // Distance = sqrt(0.35^2 + 0.35^2) = 0.495 < l1 + l2 = 0.58 (reachable).
        let shoulder = Vec3::new(0.2, 1.4, 0.0);
        let target = Vec3::new(0.2, 1.05, -0.35);
        let pole = Vec3::new(0.0, 0.0, 1.0); // elbow tip toward +Z
        let (ur, lr, elbow) = solve_arm_ik(
            shoulder,
            target,
            0.30,
            0.28,
            pole,
            Vec3::NEG_Y,
            Vec3::NEG_Z,
            Vec3::NEG_Y,
            Vec3::NEG_Z,
            Quat::from_rotation_y(1.2), // badly twisted fallback
            Quat::from_rotation_y(-0.9),
        );
        // Elbow tip must bow toward +Z, forearm reaches target cleanly.
        assert!(elbow.z > (shoulder.z + target.z) * 0.5);
        let upper_end = shoulder + (ur * Vec3::NEG_Y) * 0.30;
        let lower_end = upper_end + (lr * Vec3::NEG_Y) * 0.28;
        assert!((lower_end - target).length() < 1e-3, "missed target: {lower_end:?}");
        // Relative rotation between upper and lower arm must be a pure hinge around local +X!
        let rel = (ur.inverse() * lr).normalize();
        let axis_angle = rel.to_scaled_axis();
        assert!(axis_angle.x > 0.2, "elbow must flex positively around +X: {axis_angle:?}");
        assert!(axis_angle.y.abs() < 1e-3 && axis_angle.z.abs() < 1e-3, "off-hinge twist: {axis_angle:?}");
    }
}
