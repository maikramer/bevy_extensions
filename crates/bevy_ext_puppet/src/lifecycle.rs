//! Building puppets on demand and taking them apart again.
//!
//! A puppet costs 16 bodies, 15 joints and 17 muscles in the solver. A game
//! with dozens of characters only needs the ones being hit, falling or dying:
//! insert [`Puppet`] when that happens (the rig is seeded from the current
//! animated pose, as Euphoria seeds its body from the running animation),
//! and let [`PuppetIdleDismantle`] (or [`PuppetLifecycleExt::dismantle_puppet`])
//! remove it once the animation is back in charge.

use bevy::prelude::*;

use crate::behaviour::{PuppetBehaviours, PuppetRequests};
use crate::neuro::PuppetNeuroState;
use crate::pose::{PuppetBlend, PuppetTargets};
use crate::rig::{Puppet, PuppetParts, PuppetRig, PuppetRigError, PuppetRigPending};
use crate::sense::PuppetSense;
use crate::state::{PuppetInbox, PuppetMode, PuppetState};

/// Removes the puppet from `puppet`: despawns its bodies, joints and muscles,
/// gives the bones back to the animation and removes the runtime components
/// (the [`PuppetConfig`](crate::rig::PuppetConfig) stays, so inserting
/// [`Puppet`] again rebuilds it with the same tuning).
pub fn dismantle(world: &mut World, puppet: Entity) {
    if world.get_entity(puppet).is_err() {
        return;
    }
    // Bones still holding a written physics pose get their animated local back.
    let restore: Vec<(Entity, Transform)> = match (
        world.get::<PuppetRig>(puppet),
        world.get::<PuppetTargets>(puppet),
    ) {
        (Some(rig), Some(targets)) => rig
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                targets.written[i]?;
                let current = world.get::<Transform>(p.bone)?;
                Some((p.bone, targets.animated_local(p.part, *current)))
            })
            .collect(),
        _ => Vec::new(),
    };
    for (bone, anim) in restore {
        if let Some(mut t) = world.get_mut::<Transform>(bone) {
            t.set_if_neq(anim);
        }
    }
    let parts: Vec<Entity> = world
        .get::<PuppetParts>(puppet)
        .map(|p| p.entities().to_vec())
        .unwrap_or_default();
    for part in parts {
        if let Ok(e) = world.get_entity_mut(part) {
            e.despawn();
        }
    }
    if let Ok(mut e) = world.get_entity_mut(puppet) {
        e.remove::<(
            Puppet,
            PuppetRig,
            PuppetTargets,
            PuppetSense,
            PuppetNeuroState,
            PuppetRequests,
            PuppetBehaviours,
            PuppetInbox,
            PuppetState,
            PuppetBlend,
            PuppetRigPending,
            PuppetRigError,
            PuppetIdleDismantle,
        )>();
    }
}

/// `commands.dismantle_puppet(entity)`.
pub trait PuppetLifecycleExt {
    fn dismantle_puppet(&mut self, puppet: Entity);
}

impl PuppetLifecycleExt for Commands<'_, '_> {
    fn dismantle_puppet(&mut self, puppet: Entity) {
        self.queue(move |world: &mut World| dismantle(world, puppet));
    }
}

/// Dismantle the puppet after `after` seconds back in the animated state with
/// nothing running (no behaviours, no visible physics).
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
pub struct PuppetIdleDismantle {
    pub after: f32,
    pub idle: f32,
}

impl PuppetIdleDismantle {
    pub fn after(secs: f32) -> Self {
        Self { after: secs, idle: 0.0 }
    }
}

pub fn dismantle_idle_puppets(
    time: Res<Time>,
    mut commands: Commands,
    mut puppets: Query<(
        Entity,
        &mut PuppetIdleDismantle,
        &PuppetState,
        &PuppetBehaviours,
        &PuppetBlend,
        &PuppetInbox,
    ), With<PuppetRig>>,
) {
    for (e, mut idle, state, behaviours, blend, inbox) in &mut puppets {
        if state.mode != PuppetMode::Animated
            || !behaviours.active.is_empty()
            || blend.any()
            || !inbox.0.is_empty()
        {
            idle.idle = 0.0;
            continue;
        }
        idle.idle += time.delta_secs();
        if idle.idle >= idle.after {
            commands.dismantle_puppet(e);
        }
    }
}
