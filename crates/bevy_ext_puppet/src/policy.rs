//! Hooks for learned control (DReCon/DeepMimic style).
//!
//! A [`PuppetPolicy`] maps a fixed-size [`PuppetObservation`] (the body in its
//! heading frame, contacts, balance features and the animated target) to a
//! [`PuppetAction`]: per-joint residual rotations on top of the animated
//! target and per-limb stiffness. [`PolicyBehaviour`] runs it as one more
//! behaviour, so a policy composes with the procedural ones through the
//! junction (give it a priority above `BALANCE` to replace balancing).
//!
//! With the `ml` feature, [`PuppetRecorder`] writes observations and the
//! resolved drive of puppets marked [`RecordObservations`] as JSON lines, the
//! raw material to train a policy offline (e.g. by imitation of the
//! procedural controller, then RL).

use bevy::prelude::*;

use crate::behaviour::{Behaviour, BehaviourCtx, BehaviourStatus, JointTarget, PuppetRequests, priority};
use crate::part::{Limb, PART_COUNT, PuppetPart};
use crate::pose::PuppetTargets;
use crate::sense::PuppetSense;

/// Length of [`PuppetObservation::features`].
pub const OBSERVATION_SIZE: usize = PART_COUNT * 15 + PART_COUNT + 7 + (PART_COUNT - 1) * 6;

/// What a policy sees, in the puppet's heading frame (yaw-invariant).
#[derive(Clone, Debug, Default)]
pub struct PuppetObservation {
    pub features: Vec<f32>,
}

/// First two columns of a rotation matrix: a continuous rotation encoding.
fn rot6(q: Quat) -> [f32; 6] {
    let m = Mat3::from_quat(q);
    [m.x_axis.x, m.x_axis.y, m.x_axis.z, m.y_axis.x, m.y_axis.y, m.y_axis.z]
}

/// Builds the observation from what the puppet senses.
pub fn observe(sense: &PuppetSense, targets: &PuppetTargets) -> PuppetObservation {
    let up = sense.up;
    let forward = sense.heading.reject_from(up).normalize_or(sense.lift(Vec2::NEG_Y, 0.0));
    let right = forward.cross(up);
    let to_heading = Quat::from_mat3(&Mat3::from_cols(right, up, -forward)).inverse();
    let mut f = Vec::with_capacity(OBSERVATION_SIZE);
    for part in PuppetPart::ALL {
        let b = sense.body(part);
        f.extend((to_heading * (b.pos - sense.com)).to_array());
        f.extend(rot6(to_heading * b.rot));
        f.extend((to_heading * b.lin_vel).to_array());
        f.extend((to_heading * b.ang_vel).to_array());
    }
    for part in PuppetPart::ALL {
        f.push(if sense.touching(part) { 1.0 } else { 0.0 });
    }
    f.push(sense.com.dot(up) - sense.ground);
    f.extend((to_heading * sense.com_vel).to_array());
    let cp = sense.lift(sense.capture_point - sense.support_centre, 0.0);
    let cp = to_heading * cp;
    f.push(cp.x);
    f.push(cp.z);
    f.push(sense.capture_margin.min(1.0));
    for part in PuppetPart::ALL.iter().skip(1) {
        f.extend(rot6(targets.local[part.index()]));
    }
    debug_assert_eq!(f.len(), OBSERVATION_SIZE);
    PuppetObservation { features: f }
}

/// What a policy outputs.
#[derive(Clone, Debug)]
pub struct PuppetAction {
    /// Residual rotation per joint (axis-angle, parent frame), on top of the animation.
    pub residual: [Vec3; PART_COUNT],
    /// Stiffness multiplier per limb (in [`Limb::ALL`] order).
    pub stiffness: [f32; 5],
}

impl Default for PuppetAction {
    fn default() -> Self {
        Self {
            residual: [Vec3::ZERO; PART_COUNT],
            stiffness: [1.0; 5],
        }
    }
}

/// A controller mapping observations to actions (a neural network, a script…).
pub trait PuppetPolicy: Send + Sync + 'static {
    fn act(&mut self, observation: &PuppetObservation, action: &mut PuppetAction);
}

impl<F> PuppetPolicy for F
where
    F: FnMut(&PuppetObservation, &mut PuppetAction) + Send + Sync + 'static,
{
    fn act(&mut self, observation: &PuppetObservation, action: &mut PuppetAction) {
        self(observation, action)
    }
}

/// Runs a [`PuppetPolicy`] as a behaviour.
pub struct PolicyBehaviour<P: PuppetPolicy> {
    pub policy: P,
    pub priority: i32,
    action: PuppetAction,
}

impl<P: PuppetPolicy> PolicyBehaviour<P> {
    pub const NAME: &'static str = "policy";

    pub fn new(policy: P) -> Self {
        Self {
            policy,
            priority: priority::BALANCE + 5,
            action: PuppetAction::default(),
        }
    }
}

impl<P: PuppetPolicy> Behaviour for PolicyBehaviour<P> {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn update(&mut self, ctx: &BehaviourCtx, out: &mut PuppetRequests) -> BehaviourStatus {
        let obs = observe(ctx.sense, ctx.targets);
        self.action = PuppetAction::default();
        self.policy.act(&obs, &mut self.action);
        for part in PuppetPart::ALL.iter().skip(1) {
            let i = part.index();
            let limb = Limb::ALL.iter().position(|l| *l == part.limb()).unwrap_or(0);
            let q = Quat::from_scaled_axis(self.action.residual[i]) * ctx.targets.local[i];
            out.joint(*part, JointTarget::Local(q.normalize()), self.action.stiffness[limb], self.priority);
        }
        BehaviourStatus::Running
    }
}

/// Marks a puppet whose observations [`PuppetRecorder`] writes.
#[derive(Component, Default, Clone, Copy)]
pub struct RecordObservations;

#[cfg(feature = "ml")]
pub use recorder::*;

#[cfg(feature = "ml")]
mod recorder {
    use super::*;
    use crate::behaviour::PuppetRequests;
    use crate::state::PuppetState;
    use std::io::Write;

    /// Writes `{t, puppet, mode, obs, drive}` lines for [`RecordObservations`] puppets.
    #[derive(Resource)]
    pub struct PuppetRecorder {
        pub path: std::path::PathBuf,
        writer: Option<std::io::BufWriter<std::fs::File>>,
    }

    impl PuppetRecorder {
        pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
            Self {
                path: path.into(),
                writer: None,
            }
        }
    }

    #[derive(serde::Serialize)]
    struct Sample<'a> {
        t: f32,
        puppet: u64,
        mode: String,
        obs: &'a [f32],
        /// Resolved joint targets (quaternion xyzw) and stiffness.
        drive: Vec<[f32; 5]>,
    }

    pub fn record_observations(
        recorder: Option<ResMut<PuppetRecorder>>,
        puppets: Query<(Entity, &PuppetSense, &PuppetTargets, &PuppetRequests, &PuppetState), With<RecordObservations>>,
    ) {
        let Some(mut rec) = recorder else { return };
        if rec.writer.is_none() {
            match std::fs::File::create(&rec.path) {
                Ok(f) => rec.writer = Some(std::io::BufWriter::new(f)),
                Err(e) => {
                    warn_once!("puppet recorder: {}: {e}", rec.path.display());
                    return;
                }
            }
        }
        let Some(w) = rec.writer.as_mut() else { return };
        for (e, sense, targets, req, state) in &puppets {
            let obs = observe(sense, targets);
            let drive = req
                .drive
                .iter()
                .map(|d| {
                    let q = d.target;
                    [q.x, q.y, q.z, q.w, d.stiffness]
                })
                .collect();
            let sample = Sample {
                t: sense.time,
                puppet: e.to_bits(),
                mode: format!("{:?}", state.mode),
                obs: &obs.features,
                drive,
            };
            if let Ok(line) = serde_json::to_string(&sample) {
                let _ = writeln!(w, "{line}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_is_invariant_under_a_rotated_world() {
        let targets = PuppetTargets::default();
        let mut s = PuppetSense::default();
        let vel = Vec3::new(0.5, -0.7, 1.2);
        let offset = Vec3::new(0.2, 0.0, -0.3);
        s.com_vel = vel;
        s.capture_point = s.flat(offset);
        let expected = observe(&s, &targets);

        for rotation in [
            Quat::from_rotation_y(std::f32::consts::PI),
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        ] {
            s.heading = rotation * Vec3::NEG_Z;
            s.up = rotation * Vec3::Y;
            s.com_vel = rotation * vel;
            s.capture_point = s.flat(rotation * offset);
            for body in &mut s.bodies {
                body.rot = rotation;
            }
            let rotated = observe(&s, &targets);
            for (a, b) in expected.features.iter().zip(rotated.features) {
                assert!((a - b).abs() < 1e-5, "{a} vs {b}");
            }
        }
    }
}
