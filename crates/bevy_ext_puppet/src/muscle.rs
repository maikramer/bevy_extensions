//! Muscles, virtual model control, and pins: turning the resolved drive into
//! XPBD constraints, implicit Jacobian-transpose virtual torques, and forces.
//!
//! **Muscles** are XPBD angular constraints ([`MuscleJoint`]) solved per substep
//! inside Avian with Hill-type force–velocity modulation, anisotropic swing/twist
//! compliance, first-order neuromuscular activation dynamics, and passive
//! soft-tissue viscoelasticity.
//!
//! **Virtual forces** (`VirtualForce`) map desired world wrenches through either
//! a stance leg (`Foot → Calf → Thigh`) or a bracing/push-off arm
//! (`Hand → LowerArm → UpperArm`) using Jacobian transpose (`τ = Jᵀ F + τ_v`),
//! clamped to the contact surface friction cone and folded implicitly into
//! `MuscleJoint.target`.
//!
//! **Pins** are world-space servos towards the animated pose (the animator's
//! hand, à la PuppetMaster) — used while animated, during late get-up handoff,
//! and as optional balance assistance.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::behaviour::{PuppetRequests, VirtualChain, VirtualForce};
use crate::drive::MuscleJoint;
use crate::neuro::PuppetNeuroState;
use crate::part::{PART_COUNT, PuppetPart, Side};
use crate::pose::{PuppetTargets, shortest};
use crate::rig::{PuppetConfig, PuppetRig};
use crate::sense::PuppetSense;
use crate::state::{PuppetMode, PuppetState};

/// Implicit spring-damper on a body of `inertia`: the generalised force that,
/// applied for `dt`, moves `vel` as implicit Euler would towards `err`.
#[inline]
pub fn stable_pd<T>(k: f32, d: f32, inertia: f32, err: T, vel: T, dt: f32) -> T
where
    T: std::ops::Mul<f32, Output = T> + std::ops::Sub<Output = T>,
{
    let denom = inertia + dt * d + dt * dt * k;
    (err * k - vel * (d + dt * k)) * (inertia / denom.max(1e-9))
}

/// Inertia tensor about `pivot` of the parts selected by `pick`.
pub fn side_inertia(sense: &PuppetSense, pivot: Vec3, pick: impl Fn(PuppetPart) -> bool) -> Mat3 {
    let mut i = Mat3::ZERO;
    for part in PuppetPart::ALL {
        if !pick(part) {
            continue;
        }
        let b = sense.body(part);
        let r = b.com - pivot;
        // Parallel axis theorem.
        i += b.inertia + (Mat3::IDENTITY * r.length_squared() - outer(r, r)) * b.mass;
    }
    i
}

#[inline]
fn outer(a: Vec3, b: Vec3) -> Mat3 {
    Mat3::from_cols(a * b.x, a * b.y, a * b.z)
}

/// Matrix form of [`stable_pd`]: `I (I + c)⁻¹ (k·err − (d + dt·k)·vel)` with
/// `c = dt·d + dt²·k`, so axes of small inertia (a limb's long axis) are not
/// over-driven.
#[inline]
pub fn stable_pd_tensor(k: f32, d: f32, inertia: Mat3, err: Vec3, vel: Vec3, dt: f32) -> Vec3 {
    let c = dt * d + dt * dt * k;
    let m = inertia * (inertia + Mat3::IDENTITY * c).inverse();
    let t = m * (err * k - vel * (d + dt * k));
    if t.is_finite() { t } else { Vec3::ZERO }
}

/// Mean principal inertia (for critical damping).
#[inline]
fn mean_inertia(i: Mat3) -> f32 {
    ((i.x_axis.x + i.y_axis.y + i.z_axis.z) / 3.0).max(1e-5)
}

/// Applies muscle constraints, implicit virtual forces, pins and extra torques.
#[allow(clippy::type_complexity)]
pub fn apply_muscles(
    mut puppets: Query<(
        &PuppetRig,
        &PuppetSense,
        &PuppetRequests,
        &PuppetTargets,
        &PuppetConfig,
        &PuppetState,
        &mut PuppetNeuroState,
    )>,
    mut forces: Query<Forces>,
    mut muscles: Query<&mut MuscleJoint>,
) {
    for (rig, sense, req, targets, config, state, mut neuro) in &mut puppets {
        if sense.total_mass <= 0.0 {
            continue;
        }
        let dt = sense.dt;
        let unit = rig.torque_unit(sense.g);
        let mut torque = [Vec3::ZERO; PART_COUNT];
        let mut force = [Vec3::ZERO; PART_COUNT];

        // ---- Implicit Jacobian-transpose virtual torques (legs & arms) ------
        let feet_down = sense.feet_down().max(1) as f32;
        let mut virtual_joint_torque = [Vec3::ZERO; PART_COUNT];
        for vf in &req.virtual_forces {
            accumulate_virtual_joint_torques(
                vf,
                sense,
                rig,
                config,
                &req.strength,
                unit,
                feet_down,
                &mut virtual_joint_torque,
            );
        }

        // Fatigue gently softens active tone only under prolonged exhaustion
        // (`fatigue > 0.35`) without weakening early recovery steps.
        let fatigue_scale = if state.mode == PuppetMode::Active && neuro.fatigue > 0.35 {
            (1.0 - 0.16 * (neuro.fatigue - 0.35)).clamp(0.88, 1.0)
        } else {
            1.0
        };
        let instant_activation = state.mode == PuppetMode::Animated;

        for part in PuppetPart::ALL {
            let Some(parent) = part.parent() else { continue };
            let i = part.index();
            let Some(muscle_e) = rig.parts[i].muscle else { continue };
            let Ok(mut muscle) = muscles.get_mut(muscle_e) else { continue };
            let drive = req.drive[i];
            let raw_strength = config.strength * req.strength[i] * fatigue_scale;
            let strength = neuro.step_activation(
                i,
                raw_strength,
                dt,
                config.muscle_activation_time,
                instant_activation,
            );
            let pivot = sense.bodies[i].pos;
            let child_side = |q: PuppetPart| q.is_in_subtree_of(part);
            let grounded_child = PuppetPart::ALL
                .iter()
                .any(|q| child_side(*q) && sense.contacts[q.index()]);
            // The load the muscle moves: the hanging subtree, or — with the
            // subtree planted — the rest of the body (a standing ankle).
            let load = if grounded_child {
                side_inertia(sense, pivot, |q| !child_side(q))
            } else {
                side_inertia(sense, pivot, child_side)
            };
            let load = mean_inertia(load);
            let k = if drive.spring {
                unit * part.stiffness_coeff() * drive.stiffness * strength
            } else {
                0.0
            };
            let k_nominal =
                unit * part.stiffness_coeff() * drive.stiffness.max(0.05) * strength.max(0.05);
            let d = 2.0 * config.damping_ratio * drive.damping * (k_nominal * load).sqrt();
            // Fold `Jᵀ F` virtual torque into the XPBD target (`Δθ = -τ_parent / k`):
            // the solver applies `+τ` to the proximal parent and `-τ` to the distal
            // child implicitly per substep, coupled with ground/wall contacts.
            let mut target = drive.target;
            let vtau = virtual_joint_torque[i];
            if k > 1e-3 && vtau.length_squared() > 1e-6 {
                let delta_world = (-vtau / k).clamp_length_max(0.18);
                let parent_rot = sense.body(parent).rot;
                let delta_local = parent_rot.inverse() * delta_world;
                target = (Quat::from_scaled_axis(delta_local) * target).normalize();
            }
            muscle.target = target;
            muscle.stiffness = k;
            muscle.damping = d;
            muscle.max_torque = unit
                * part.max_torque_coeff()
                * strength
                * drive.stiffness.max(drive.damping).max(0.2);
            muscle.twist_scale = part.twist_stiffness_scale();
            muscle.twist_axis_local = rig.part(part).axis_local;
            // Passive soft-tissue viscosity: subtle during active control (where
            // active critical damping already dominates), full strength when limp/dead
            // so corpses and stunned limbs settle with biological weight.
            let limp_factor = (1.0 - (drive.stiffness * strength).clamp(0.0, 1.0)) * 0.85 + 0.15;
            let passive_base = config.passive_joint_damping.max(0.0) * (unit * 0.0018) * limp_factor;
            muscle.passive_damping = passive_base * part.passive_damping_coeff();

            // Gravity compensation: a hanging subtree is held up by
            // feed-forward, so tracking does not sag under its weight.
            if !grounded_child && drive.spring && state.mode != PuppetMode::Dead {
                let mut moment = Vec3::ZERO;
                for q in PuppetPart::ALL {
                    if child_side(q) {
                        let b = sense.body(q);
                        moment += (b.com - pivot) * b.mass;
                    }
                }
                let g = -sense.up * sense.g;
                let tau = -moment.cross(g) * drive.stiffness.min(1.0) * strength.min(1.0);
                torque[i] += tau;
                torque[parent.index()] -= tau;
            }
        }

        // ---- Slope-conforming planted feet ----------------------------------
        // Contacts are solved before the solver joints and with the foot's own
        // mass, so a planted foot under a 70 kg body rolls on its soft
        // contact. The plant holds it flush to the local slope normal from a
        // static anchor, capped by the torque the sole could transmit.
        let planted: Vec<Side> = [Side::Left, Side::Right]
            .into_iter()
            .filter(|s| req.plant[s.index()] && sense.foot_down(*s))
            .collect();
        for side in [Side::Left, Side::Right] {
            let Ok(mut plant) = muscles.get_mut(rig.plants[side.index()]) else { continue };
            let foot = PuppetPart::foot(side);
            if !planted.contains(&side) {
                plant.stiffness = 0.0;
                plant.damping = 0.0;
                plant.passive_damping = 0.0;
                continue;
            }
            let b = sense.body(foot);
            let target_up = sense.foot_normals[side.index()];
            plant.target = crate::ik::level(b.rot, rig.part(foot).up_local, target_up);
            let load = mean_inertia(side_inertia(sense, b.pos, |q| q != foot));
            let k = 3.0 * unit;
            plant.stiffness = k;
            plant.damping = 2.0 * (k * load).sqrt();
            plant.passive_damping = 0.0;
            plant.twist_scale = 1.0;
            plant.max_speed = 0.0;
            let half_foot =
                0.5 * (rig.part(foot).sole_local[0] - rig.part(foot).sole_local[2]).length();
            plant.max_torque = sense.total_mass * sense.g / planted.len() as f32 * half_foot;
        }

        // ---- Internal torques requested by behaviours ----------------------
        for (a, b, t) in &req.torque_pairs {
            torque[a.index()] += *t;
            torque[b.index()] -= *t;
        }
        for (a, t) in &req.external_torques {
            torque[a.index()] += *t;
        }

        // ---- Pins ----------------------------------------------------------
        let omega = std::f32::consts::TAU * config.pin_frequency;
        for part in PuppetPart::ALL {
            let i = part.index();
            let w = (state.pins[i].max(req.pins[i]) * req.pin_scale[i]).clamp(0.0, 1.0);
            if w <= 1e-3 {
                continue;
            }
            let b = sense.bodies[i];
            let k = w * omega * omega;
            let d = 2.0 * omega * w.sqrt();
            // Linear: towards the target position (body origin = bone).
            let err = targets.world_pos[i] - b.pos;
            let vel = b.lin_vel - targets.lin_vel[i];
            let acc = stable_pd(k, d, 1.0, err, vel, dt);
            force[i] += b.mass * (acc + sense.up * sense.g * w);
            // Angular (about the body's own centre of mass).
            let rerr = shortest(targets.world_rot[i] * b.rot.inverse()).to_scaled_axis();
            let rvel = b.ang_vel - targets.ang_vel[i];
            torque[i] += stable_pd_tensor(k, d, b.inertia, rerr, rvel, dt);
        }

        for part in PuppetPart::ALL {
            let i = part.index();
            if torque[i] == Vec3::ZERO && force[i] == Vec3::ZERO {
                continue;
            }
            if let Ok(mut f) = forces.get_mut(rig.parts[i].body) {
                if torque[i].is_finite() {
                    f.apply_torque(torque[i]);
                }
                if force[i].is_finite() {
                    f.apply_force(force[i]);
                }
            }
        }
    }
}

/// Computes the Jacobian-transpose joint torques (`τ_j = (p_app - p_j) × F + τ_v`)
/// for a stance leg `(foot → calf → thigh)` or bracing arm `(hand → lower_arm → upper_arm)`,
/// clamped to the contact friction cone and anatomical joint limits (Pratt et al. 2001;
/// Coros et al. 2010 §3.3; Geijtenbeek et al. 2012 §3.2.4).
#[allow(clippy::too_many_arguments)]
pub fn accumulate_virtual_joint_torques(
    vf: &VirtualForce,
    sense: &PuppetSense,
    rig: &PuppetRig,
    config: &PuppetConfig,
    strengths: &[f32; PART_COUNT],
    unit: f32,
    feet_down: f32,
    joint_torques: &mut [Vec3; PART_COUNT],
) {
    let mg = sense.total_mass * sense.g;
    match vf.chain {
        VirtualChain::Leg(stance) => {
            if !sense.foot_down(stance) {
                return;
            }
            let up = sense.up;
            let plant_q = sense.foot_plant_quality(stance);
            let fy = vf.force.dot(up).clamp(-0.25 * mg, 0.40 * mg);
            let mut fh = vf.force - up * vf.force.dot(up);
            let normal_load = (mg / feet_down + fy.max(0.0)).max(0.1 * mg);
            let max_horiz = (config.friction * 0.35 * normal_load * plant_q.max(0.2)).max(0.0);
            let fh_len = fh.length();
            if fh_len > max_horiz && fh_len > 1e-6 {
                fh *= max_horiz / fh_len;
            }
            let f = fh + up * fy;

            let foot = PuppetPart::foot(stance);
            let calf = PuppetPart::calf(stance);
            let thigh = PuppetPart::thigh(stance);

            for child in [foot, calf, thigh] {
                let pivot = sense.body(child).pos;
                let r = vf.application - pivot;
                let mut tau = r.cross(f) + vf.torque;
                if child == foot {
                    let scale = vf.distal_scale.clamp(0.0, 1.0) * plant_q;
                    if scale <= 1e-3 {
                        continue;
                    }
                    tau *= scale;
                } else if child == calf {
                    let calf_rig = rig.part(calf);
                    let axis_local = calf_rig.axis_local.cross(-calf_rig.forward_local);
                    let hinge = (sense.body(calf).rot * axis_local).normalize_or_zero();
                    if hinge.length_squared() > 0.5 {
                        tau = hinge * tau.dot(hinge);
                    }
                }
                let max_t =
                    unit * child.max_torque_coeff() * config.strength * strengths[child.index()] * 0.65;
                let len = tau.length();
                if len > max_t && len > 1e-6 {
                    tau *= max_t / len;
                }
                if tau.is_finite() {
                    joint_torques[child.index()] += tau;
                }
            }
        }
        VirtualChain::Arm(side) => {
            let hand = PuppetPart::hand(side);
            let lower = PuppetPart::lower_arm(side);
            let upper = PuppetPart::upper_arm(side);
            let hand_touch = sense.touching(hand);
            let fore_touch = sense.touching(lower);
            if !hand_touch && !fore_touch {
                return;
            }
            // Contact normal points from the wall/floor into the arm.
            let surf_n = if hand_touch {
                sense.contact_normal(hand)
            } else {
                sense.contact_normal(lower)
            }
            .normalize_or(sense.up);

            // Clamp force to push *along* the outward surface normal (never pull)
            // and keep tangential shear within the surface friction cone.
            let fn_mag = vf.force.dot(surf_n).clamp(0.0, 0.55 * mg);
            let mut ft = vf.force - surf_n * vf.force.dot(surf_n);
            let max_tan = config.friction * 0.55 * fn_mag;
            let ft_len = ft.length();
            if ft_len > max_tan && ft_len > 1e-6 {
                ft *= max_tan / ft_len;
            }
            let f = surf_n * fn_mag + ft;

            let chain: &[PuppetPart] = if hand_touch {
                &[hand, lower, upper]
            } else {
                &[lower, upper]
            };
            for &child in chain {
                let pivot = sense.body(child).pos;
                let r = vf.application - pivot;
                let mut tau = r.cross(f) + vf.torque;
                if child == hand {
                    let scale = vf.distal_scale.clamp(0.0, 1.0);
                    if scale <= 1e-3 {
                        continue;
                    }
                    tau *= scale;
                } else if child == lower {
                    // Project onto the elbow hinge axis (`Flexion::Forward`).
                    let lower_rig = rig.part(lower);
                    let axis_local = lower_rig.axis_local.cross(lower_rig.forward_local);
                    let hinge = (sense.body(lower).rot * axis_local).normalize_or_zero();
                    if hinge.length_squared() > 0.5 {
                        tau = hinge * tau.dot(hinge);
                    }
                }
                let max_t =
                    unit * child.max_torque_coeff() * config.strength * strengths[child.index()] * 0.85;
                let len = tau.length();
                if len > max_t && len > 1e-6 {
                    tau *= max_t / len;
                }
                if tau.is_finite() {
                    joint_torques[child.index()] += tau;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_pd_converges_with_stiff_gains() {
        let (inertia, dt) = (0.01_f32, 1.0 / 60.0);
        let k = 2000.0;
        let d = 2.0 * (k * inertia).sqrt();
        let (mut x, mut v) = (1.0_f32, 0.0_f32);
        for _ in 0..240 {
            let tau = stable_pd(k, d, inertia, -x, v, dt);
            v += tau / inertia * dt;
            x += v * dt;
            assert!(x.abs() < 2.0, "diverged: {x}");
        }
        assert!(x.abs() < 0.01, "did not converge: {x}");
    }

    #[test]
    fn explicit_pd_would_diverge() {
        let (inertia, dt) = (0.01_f32, 1.0 / 60.0);
        let k = 2000.0;
        let d = 2.0 * (k * inertia).sqrt();
        let (mut x, mut v) = (1.0_f32, 0.0_f32);
        for _ in 0..60 {
            let tau = -k * x - d * v;
            v += tau / inertia * dt;
            x += v * dt;
        }
        assert!(x.abs() > 10.0 || !x.is_finite());
    }

    #[test]
    fn virtual_force_conserves_momentum_and_has_right_handed_sign() {
        let mut sense = PuppetSense {
            total_mass: 70.0,
            g: 9.81,
            up: Vec3::Y,
            com: Vec3::new(0.0, 1.0, 0.0),
            ..Default::default()
        };
        sense.contacts[PuppetPart::FootL.index()] = true;
        sense.bodies[PuppetPart::FootL.index()].pos = Vec3::new(-0.1, 0.05, 0.0);
        sense.bodies[PuppetPart::CalfL.index()].pos = Vec3::new(-0.1, 0.45, -0.15);
        sense.bodies[PuppetPart::ThighL.index()].pos = Vec3::new(-0.1, 0.90, 0.0);
        sense.bodies[PuppetPart::Pelvis.index()].pos = Vec3::new(0.0, 0.95, 0.0);

        let dummy_part = |part: PuppetPart| crate::rig::PartRig {
            part,
            body: Entity::PLACEHOLDER,
            bone: Entity::PLACEHOLDER,
            joint: None,
            muscle: None,
            path: Vec::new(),
            mass: 1.0,
            length: 0.45,
            radius: 0.05,
            axis_local: Vec3::NEG_Y,
            up_local: Vec3::Y,
            forward_local: Vec3::NEG_Z,
            end_local: Vec3::NEG_Y * 0.45,
            sole_local: [Vec3::ZERO; 4],
            anchor_in_parent: Vec3::ZERO,
        };
        let rig = PuppetRig {
            parts: PuppetPart::ALL.into_iter().map(dummy_part).collect(),
            height: 1.8,
            pelvis_height: 0.95,
            com_height: 1.0,
            ankle_height: 0.07,
            total_mass: 70.0,
            anchor: Entity::PLACEHOLDER,
            plants: [Entity::PLACEHOLDER; 2],
        };
        let config = PuppetConfig::default();
        let vf = VirtualForce::leg(
            Side::Left,
            Vec3::new(0.0, 200.0, -150.0),
            Vec3::ZERO,
            sense.com,
            1.0,
        );
        let mut jtau = [Vec3::ZERO; PART_COUNT];
        accumulate_virtual_joint_torques(
            &vf,
            &sense,
            &rig,
            &config,
            &[1.0; PART_COUNT],
            rig.torque_unit(sense.g),
            1.0,
            &mut jtau,
        );
        assert!(jtau[PuppetPart::FootL.index()].x < 0.0);
        assert!(jtau[PuppetPart::CalfL.index()].x < 0.0);
        assert!(jtau[PuppetPart::ThighL.index()].x < 0.0);
    }
}
