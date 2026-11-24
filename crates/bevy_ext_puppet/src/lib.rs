//! # bevy_ext_puppet — dynamic motion synthesis for humanoids
//!
//! An Euphoria-style character physics layer for Bevy + Avian. A character
//! marked [`Puppet`] gets a physical body (16 rigid parts, anatomical joint
//! limits) whose **muscles** track the animation. The body senses its balance
//! (centre of mass, capture point, support polygon, contacts) and a network of
//! **behaviours** reacts to the world: balance with recovery steps, stagger,
//! windmill the arms, break the fall with the hands, writhe, get up.
//!
//! ```ignore
//! app.add_plugins((PhysicsPlugins::default(), PuppetPlugin::default()));
//! commands.spawn((SceneRoot(hero), Puppet));            // the rig builds itself
//! commands.entity(hero).puppet_hit(PuppetPart::SpineUpper, impulse, Some(point));
//! commands.entity(hero).puppet_push(Vec3::new(0.0, 0.0, 120.0));
//! commands.entity(hero).puppet_kill();
//! ```
//!
//! Frame layout:
//!
//! | Schedule | Set | Work |
//! |---|---|---|
//! | `PostUpdate` | [`PuppetSystems::Build`] | spawn bodies/joints once the skeleton exists |
//! | `PostUpdate` | [`PuppetSystems::Capture`] | after the animation: animated target pose |
//! | `PostUpdate` | [`PuppetSystems::WriteBack`] | blend physics into the bone locals |
//! | physics schedule (`FixedPostUpdate`) | [`PuppetSystems::Control`] | before the solver: commands → sense → state → behaviours → muscles |

use avian3d::dynamics::solver::xpbd::{XpbdSolverSystems, prepare_xpbd_joint, solve_xpbd_joint};
use avian3d::dynamics::solver::schedule::SubstepSolverSystems;
use avian3d::prelude::{PhysicsSchedule, PhysicsSystems, SolverSystems, SubstepSchedule};
use bevy::app::AnimationSystems;
use bevy::ecs::schedule::{InternedScheduleLabel, ScheduleLabel};
use bevy::prelude::*;

pub mod behaviour;
pub mod drive;
mod contact;
#[cfg(feature = "gizmos")]
pub mod debug;
pub mod ik;
pub mod lifecycle;
pub mod muscle;
pub mod neuro;
pub mod part;
pub mod policy;
pub mod pose;
pub mod rig;
pub mod sense;
mod skin;
pub mod skeleton;
pub mod state;

pub mod prelude {
    #[cfg(feature = "gizmos")]
    pub use crate::debug::{PuppetDebug, PuppetDebugPlugin};
    pub use crate::behaviour::{
        ArmsWindmill, Behaviour, BehaviourCtx, BehaviourStatus, BodyBalance, BodyWrithe,
        BumpReaction, CatchFall, HitKind, HitReaction, JointTarget, PhysicalGetUp,
        PuppetBehaviours, PuppetRequests, PuppetSignal, ReachForWound, Relax, StaggerFall,
        VirtualChain, VirtualForce, WallBrace, priority,
    };
    pub use crate::lifecycle::{PuppetIdleDismantle, PuppetLifecycleExt};
    pub use crate::neuro::{GazeTarget, PuppetNeuroState, WoundMemory};
    pub use crate::part::{BodyZone, Limb, PuppetPart, Side};
    pub use crate::policy::{PolicyBehaviour, PuppetAction, PuppetObservation, PuppetPolicy};
    pub use crate::pose::{PuppetBlend, PuppetTargets};
    pub use crate::rig::{Puppet, PuppetBody, PuppetConfig, PuppetRig, PuppetRigError};
    pub use crate::sense::{PuppetSense, StanceState, SurfaceProbe};
    pub use crate::state::{
        PuppetCommand, PuppetCommandsExt, PuppetEvent, PuppetEventKind, PuppetMode, PuppetState,
    };
    pub use crate::{PuppetPlugin, PuppetSystems};
}

pub use prelude::*;

/// System sets of the puppet pipeline.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PuppetSystems {
    Build,
    Capture,
    WriteBack,
    /// In the physics schedule, before the simulation step.
    Control,
}

/// Adds the puppet pipeline. Add Avian's `PhysicsPlugins` **before** it (the
/// muscles are registered in Avian's solver schedules), with the same
/// schedule (`FixedPostUpdate` by default).
pub struct PuppetPlugin {
    pub physics_schedule: InternedScheduleLabel,
}

impl Default for PuppetPlugin {
    fn default() -> Self {
        Self {
            physics_schedule: FixedPostUpdate.intern(),
        }
    }
}

impl PuppetPlugin {
    pub fn new(physics_schedule: impl ScheduleLabel) -> Self {
        Self {
            physics_schedule: physics_schedule.intern(),
        }
    }
}

impl Plugin for PuppetPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<rig::Puppet>()
            .register_type::<rig::PuppetConfig>()
            .register_type::<state::PuppetState>()
            .register_type::<pose::PuppetBlend>()
            .register_type::<lifecycle::PuppetIdleDismantle>()
            .add_message::<state::PuppetEvent>();

        app.configure_sets(
            PostUpdate,
            (
                PuppetSystems::Build,
                PuppetSystems::Capture.after(AnimationSystems),
                PuppetSystems::WriteBack,
            )
                .chain()
                .before(TransformSystems::Propagate),
        );
        app.add_systems(
            PostUpdate,
            (
                rig::build_puppet_rigs.in_set(PuppetSystems::Build),
                pose::capture_targets.in_set(PuppetSystems::Capture),
                pose::write_back_pose.in_set(PuppetSystems::WriteBack),
                lifecycle::dismantle_idle_puppets.after(PuppetSystems::WriteBack),
            ),
        );
        let schedule = self.physics_schedule;
        app.configure_sets(
            schedule,
            PuppetSystems::Control
                .after(PhysicsSystems::Prepare)
                .before(PhysicsSystems::StepSimulation),
        );
        app.add_systems(
            schedule,
            (
                state::process_inbox,
                sense::sense_puppets,
                behaviour::run_behaviours,
                state::update_states,
                skin::switch_fall_colliders,
                muscle::apply_muscles,
            )
                .chain()
                .in_set(PuppetSystems::Control),
        );
        app.add_systems(
            schedule,
            contact::clamp_joint_separation.after(PhysicsSystems::StepSimulation),
        );
        app.add_systems(
            PhysicsSchedule,
            contact::resolve_floor_overlap
                .after(SolverSystems::Finalize)
                .before(SolverSystems::StoreContactImpulses),
        );
        // The muscles are XPBD constraints solved with the joints.
        app.add_systems(
            PhysicsSchedule,
            prepare_xpbd_joint::<drive::MuscleJoint>
                .in_set(SolverSystems::PreSubstep)
                .ambiguous_with_all(),
        );
        app.add_systems(
            SubstepSchedule,
            (
                solve_xpbd_joint::<drive::MuscleJoint>
                    .in_set(XpbdSolverSystems::SolveUserConstraints)
                    .ambiguous_with_all(),
                drive::damp_muscles
                    .in_set(SubstepSolverSystems::Damping)
                    .ambiguous_with_all(),
            ),
        );
        #[cfg(feature = "ml")]
        app.add_systems(
            schedule,
            policy::record_observations
                .after(PuppetSystems::Control)
                .before(PhysicsSystems::StepSimulation),
        );
    }
}
