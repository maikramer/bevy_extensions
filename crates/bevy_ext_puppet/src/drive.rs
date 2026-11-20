//! The muscle as an XPBD constraint inside Avian's solver.
//!
//! A joint servo applied as an external torque is integrated explicitly and
//! each joint ignores its neighbours, so a chain of stiff servos rings and
//! gains energy (a light pelvis between three loaded joints is the worst
//! case). Inside the solver the servo is an angular *compliant* constraint
//! (compliance `1/k`) with XPBD damping: implicit, solved per substep and
//! Gauss–Seidel coupled with the joints and contacts — stable for any gain.
//!
//! The spring is the XPBD compliant angular constraint (`Δλ = −k·C·h² /
//! (1 + k·h²·w)` per substep, clamped to the maximum torque). Damping is not
//! done at position level: XPBD only updates velocities after the whole
//! substep, so every muscle on a body would damp the same stale velocity and
//! three of them on a light pelvis overshoot. [`damp_muscles`] runs in Avian's
//! velocity damping stage instead, Gauss–Seidel on current velocities, with
//! the implicit damper `J = −(1 + h·d·K)⁻¹·h·d·ω_rel` (`K = I₁⁻¹ + I₂⁻¹`).

use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::solver_body::{
    SolverBodies, SolverBody, SolverBodyIndex, SolverBodyInertia,
};
use avian3d::dynamics::solver::xpbd::{AngularConstraint, XpbdConstraint, XpbdConstraintSolverData};
use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::prelude::*;

use crate::pose::shortest;

/// A muscle between a parent and a child body: drives the child's rotation,
/// relative to the parent, towards `target`.
///
/// Models three key biomechanical properties inside Avian's XPBD solver:
/// 1. **Anisotropic swing vs. twist compliance** (`twist_scale` along `twist_axis_local`):
///    axial spine/shoulder/hip twist yields more naturally than flexion/extension.
/// 2. **Hill-type force–velocity ceiling** (`max_speed`): eccentric lengthening
///    (landing/impact absorption) yields up to 1.35× isometric torque while rapid
///    concentric contraction rolls off smoothly instead of snapping robotically.
/// 3. **Passive viscoelastic tissue damping** (`passive_damping`): biological
///    tendon/ligament resistance remains active even when muscles are relaxed.
#[derive(Component, Clone, Debug)]
#[require(MuscleJointSolverData)]
pub struct MuscleJoint {
    pub parent: Entity,
    pub child: Entity,
    /// Target rotation of the child relative to the parent (`R_child = R_parent · target`).
    pub target: Quat,
    /// N·m/rad (0 = no spring).
    pub stiffness: f32,
    /// Active velocity damping (N·m·s/rad).
    pub damping: f32,
    /// Isometric torque ceiling (N·m).
    pub max_torque: f32,
    /// Relative stiffness multiplier for axial twist around `twist_axis_local` (`1.0` = isotropic).
    pub twist_scale: f32,
    /// Local axis of the child body defining axial twist.
    pub twist_axis_local: Vec3,
    /// Characteristic concentric speed (rad/s) for Hill force–velocity roll-off
    /// (`<= 0.0` disables force–velocity modulation).
    pub max_speed: f32,
    /// Passive soft-tissue viscoelastic damping (N·m·s/rad), active even when limp.
    pub passive_damping: f32,
}

impl MuscleJoint {
    pub fn new(parent: Entity, child: Entity) -> Self {
        Self {
            parent,
            child,
            target: Quat::IDENTITY,
            stiffness: 0.0,
            damping: 0.0,
            max_torque: 0.0,
            twist_scale: 1.0,
            twist_axis_local: Vec3::Y,
            max_speed: 28.0,
            passive_damping: 0.0,
        }
    }
}

/// Hill-type force–velocity multiplier along the desired torque axis `n`:
/// * `v_along > 0`: concentric contraction (moving toward target) → full
///   athletic torque through normal speeds (`<= 0.35 * max_speed`), then smooth
///   hyperbolic roll-off at extreme speeds so limbs never snap robotically.
/// * `v_along < 0`: eccentric lengthening (absorbing impact/load) → torque
///   rises smoothly up to 1.35× isometric strength for shock absorption.
#[inline]
pub fn hill_force_velocity(v_along: f32, max_speed: f32) -> f32 {
    if max_speed <= 1e-3 || !v_along.is_finite() {
        return 1.0;
    }
    if v_along >= 0.0 {
        let excess = (v_along - 0.35 * max_speed).max(0.0);
        (1.0 / (1.0 + excess / max_speed)).max(0.30)
    } else {
        // Eccentric braking: physiological Hill plateau at ~1.30× isometric strength.
        let stretch = -v_along;
        1.0 + 0.30 * (stretch / (stretch + 0.35 * max_speed))
    }
}

#[derive(Component, Default, Debug)]
pub struct MuscleJointSolverData {
    rot1: Quat,
    rot2: Quat,
    /// Torque applied last step (on the child), for diagnostics.
    pub torque: Vec3,
    lagrange: Vec3,
}

impl XpbdConstraintSolverData for MuscleJointSolverData {
    fn clear_lagrange_multipliers(&mut self) {
        self.lagrange = Vec3::ZERO;
    }
}

impl EntityConstraint<2> for MuscleJoint {
    fn entities(&self) -> [Entity; 2] {
        [self.parent, self.child]
    }
}

impl MapEntities for MuscleJoint {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.parent = mapper.get_mapped(self.parent);
        self.child = mapper.get_mapped(self.child);
    }
}

impl AngularConstraint for MuscleJoint {}

/// `n·I⁻¹·n` from a symmetric inverse tensor.
#[inline]
fn inv_inertia_along(inv: avian3d::math::SymmetricTensor, n: Vec3) -> f32 {
    n.dot(inv * n)
}

impl MuscleJoint {
    /// XPBD spring update along a unit axis for error `c`, generalized
    /// inverse inertia `w`, stiffness `k`, torque ceiling `max_tau`, and substep `h`:
    /// `Δλ = −k·c·h² / (1 + k·h²·w)`.
    #[inline]
    fn delta_lagrange(k: f32, max_tau: f32, c: f32, w: f32, h: f32) -> f32 {
        let k = k.max(0.0);
        let dl = -k * c * h * h / (1.0 + k * h * h * w);
        let max = max_tau.max(0.0) * h * h;
        dl.clamp(-max, max)
    }

    #[inline]
    fn solve_axis(
        &self,
        body1: &mut SolverBody,
        body2: &mut SolverBody,
        inv1: avian3d::math::SymmetricTensor,
        inv2: avian3d::math::SymmetricTensor,
        axis_err: Vec3,
        k: f32,
        dt: f32,
    ) -> Vec3 {
        let angle = axis_err.length();
        if angle <= 1e-6 || k <= 0.0 {
            return Vec3::ZERO;
        }
        let n = axis_err / angle;
        let w = inv_inertia_along(inv1, n) + inv_inertia_along(inv2, n);
        if w <= f32::EPSILON {
            return Vec3::ZERO;
        }
        let v_rel = (body2.angular_velocity - body1.angular_velocity).dot(n);
        let fv = hill_force_velocity(v_rel, self.max_speed);
        let max_tau = self.max_torque.max(0.0) * fv;
        // The constraint is `C = −angle` along `n`; `apply_angular_lagrange_update`
        // turns body 2 by `+Δλ·I₂⁻¹·n` and body 1 the other way.
        let dl = Self::delta_lagrange(k, max_tau, -angle, w, dt);
        self.apply_angular_lagrange_update(body1, body2, inv1, inv2, dl, n);
        n * dl
    }
}

impl XpbdConstraint<2> for MuscleJoint {
    type SolverData = MuscleJointSolverData;

    fn prepare(
        &mut self,
        bodies: [&avian3d::dynamics::rigid_body::RigidBodyQueryReadOnlyItem; 2],
        solver_data: &mut MuscleJointSolverData,
    ) {
        solver_data.rot1 = bodies[0].rotation.0;
        solver_data.rot2 = bodies[1].rotation.0;
        solver_data.torque = Vec3::ZERO;
    }

    fn solve(
        &mut self,
        bodies: [&mut SolverBody; 2],
        inertias: [&SolverBodyInertia; 2],
        solver_data: &mut MuscleJointSolverData,
        dt: f32,
    ) {
        if self.stiffness <= 0.0 {
            return;
        }
        let [body1, body2] = bodies;
        let inv1 = inertias[0].effective_inv_angular_inertia();
        let inv2 = inertias[1].effective_inv_angular_inertia();
        let q1 = (body1.delta_rotation * solver_data.rot1).normalize();
        let q2 = (body2.delta_rotation * solver_data.rot2).normalize();
        let want = q1 * self.target;
        // Rotation the child still has to make (world axis·angle).
        let diff = shortest(want * q2.inverse()).to_scaled_axis();
        let mut total = Vec3::ZERO;
        let ts = self.twist_scale.clamp(0.05, 2.0);
        if (ts - 1.0).abs() > 0.02 && self.twist_axis_local.length_squared() > 0.5 {
            let twist_axis = (q2 * self.twist_axis_local).normalize();
            let twist_err = twist_axis * diff.dot(twist_axis);
            let swing_err = diff - twist_err;
            total += self.solve_axis(body1, body2, inv1, inv2, swing_err, self.stiffness, dt);
            total += self.solve_axis(body1, body2, inv1, inv2, twist_err, self.stiffness * ts, dt);
        } else {
            total += self.solve_axis(body1, body2, inv1, inv2, diff, self.stiffness, dt);
        }
        solver_data.lagrange += total;
        if dt > 0.0 {
            solver_data.torque = solver_data.lagrange / (dt * dt);
        }
    }
}

/// Implicit relative angular velocity damping of the muscles + passive soft-tissue
/// viscoelastic damping (see the module docs).
pub fn damp_muscles(
    mut solver_bodies: ResMut<SolverBodies>,
    index_query: Query<&SolverBodyIndex>,
    muscles: Query<&MuscleJoint>,
    time: Res<Time>,
) {
    let h = time.delta_secs();
    let access = solver_bodies.access();
    let mut dummy1 = SolverBody::DUMMY;
    let mut dummy2 = SolverBody::DUMMY;
    for m in &muscles {
        let eff_damping = m.damping.max(0.0) + m.passive_damping.max(0.0);
        if eff_damping <= 0.0 {
            continue;
        }
        let i1 = index_query.get(m.parent).copied().unwrap_or(SolverBodyIndex::INVALID);
        let i2 = index_query.get(m.child).copied().unwrap_or(SolverBodyIndex::INVALID);
        if i1 == i2 {
            continue;
        }
        let (mut body1, mut inertia1) = (&mut dummy1, &SolverBodyInertia::DUMMY);
        let (mut body2, mut inertia2) = (&mut dummy2, &SolverBodyInertia::DUMMY);
        // SAFETY: distinct bodies, muscles processed serially.
        let (b1, b2) = unsafe { access.get_pair_unchecked_mut(i1, i2) };
        if let Some((body, inertia)) = b1 {
            if !body.flags.is_kinematic() {
                inertia1 = inertia;
            }
            body1 = body;
        }
        if let Some((body, inertia)) = b2 {
            if !body.flags.is_kinematic() {
                inertia2 = inertia;
            }
            body2 = body;
        }
        let inv1 = inertia1.effective_inv_angular_inertia().to_mat3();
        let inv2 = inertia2.effective_inv_angular_inertia().to_mat3();
        let k = inv1 + inv2;
        let rel = body2.angular_velocity - body1.angular_velocity;
        let hd = h * eff_damping;
        let mat = Mat3::IDENTITY + k * hd;
        if mat.determinant().abs() < 1e-12 {
            continue;
        }
        let mut j = -(mat.inverse() * (rel * hd));
        // Passive tissue damping remains active even when active max_torque is tiny (e.g. Dead).
        let passive_ceiling = m.passive_damping.max(0.0) * 18.0;
        let max = (m.max_torque.max(0.0) + passive_ceiling) * h;
        if j.length() > max {
            j = j.normalize_or_zero() * max;
        }
        body1.angular_velocity -= inv1 * j;
        body2.angular_velocity += inv2 * j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hill_force_velocity_boosts_eccentric_and_softens_concentric() {
        let iso = hill_force_velocity(0.0, 14.0);
        assert!((iso - 1.0).abs() < 1e-6);

        let conc_fast = hill_force_velocity(18.9, 14.0);
        assert!((conc_fast - 0.5).abs() < 1e-3, "concentric half at 1.35*max_speed: {conc_fast}");
        assert!(hill_force_velocity(100.0, 14.0) >= 0.30);

        let ecc = hill_force_velocity(-8.0, 14.0);
        assert!(ecc > 1.15 && ecc <= 1.31, "eccentric braking boost expected: {ecc}");
    }
}
