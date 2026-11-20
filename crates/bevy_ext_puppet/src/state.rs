//! The puppet's high-level state machine, commands, reflexes, and events.
//!
//! ```text
//!  Animated ──hit/bump/activate──▶ Active ──lost balance──▶ Falling ──still──▶ Down
//!     ▲                              │                                           │
//!     └──────────recovered───────────┘           GettingUp ◀──getup delay────────┘
//!     └──────────────────────────────────────────────┘
//!  any ──kill──▶ Dead ──revive──▶ GettingUp
//! ```
//!
//! * **Animated**: the animation drives; bodies are pinned to it (they still
//!   collide and can be bumped or hit). Light hits/bumps run partial upper-body
//!   physics while keeping locomotion intact; rapid consecutive hits compound
//!   `trauma` and naturally escalate to `Active` stagger.
//! * **Active**: physics drives the visible pose; balance runs with automatic
//!   `WallBrace` near walls, organic `ArmsWindmill` on momentum spikes, and
//!   `ReachForWound` after heavy hits.
//! * **Falling**: multi-phase `CatchFall` (airborne flail, protective roll tuck,
//!   or directional fall-breaking with twist-locked arm IK) + `WallBrace`.
//! * **Down**: lying; optionally writhing (`BodyWrithe`); gets up after a delay.
//! * **GettingUp**: biomechanical multi-stage `PhysicalGetUp` (head righting →
//!   hand push-up → lunge tuck → rise) smoothly blending into animation.
//! * **Dead**: progressive anatomical neuromuscular collapse (`Relax`) settling
//!   under passive soft-tissue damping.

use avian3d::prelude::*;
use bevy::ecs::message::{Message, MessageWriter};
use bevy::prelude::*;

use crate::behaviour::{
    ArmsWindmill, Behaviour, BodyBalance, BodyWrithe, BumpReaction, CatchFall, HitKind,
    HitReaction, PhysicalGetUp, PuppetBehaviours, PuppetRequests, PuppetSignal, ReachForWound,
    Relax, StaggerFall, WallBrace, priority,
};
use crate::ik::smooth;
use crate::neuro::PuppetNeuroState;
use crate::part::{BodyZone, PART_COUNT, PuppetPart, Side};
use crate::pose::{PuppetBlend, PuppetTargets};
use crate::rig::{PuppetConfig, PuppetRig};
use crate::sense::PuppetSense;

/// High-level mode of a puppet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub enum PuppetMode {
    #[default]
    Animated,
    Active,
    Falling,
    Down,
    GettingUp,
    Dead,
}

impl PuppetMode {
    pub(crate) fn uses_fall_contacts(self) -> bool {
        matches!(self, Self::Falling | Self::Down | Self::GettingUp | Self::Dead)
    }
}

/// State of the state machine (and the pin weights it commands).
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Default)]
pub struct PuppetState {
    pub mode: PuppetMode,
    /// Seconds in the current mode.
    pub time_in_mode: f32,
    /// Current pin weight per part.
    pub pins: [f32; PART_COUNT],
    /// Lying face up when the last get-up started.
    pub face_up: bool,
    stillness: f32,
    getup_requested: bool,
}

impl Default for PuppetState {
    fn default() -> Self {
        Self {
            mode: PuppetMode::Animated,
            time_in_mode: 0.0,
            pins: [1.0; PART_COUNT],
            face_up: false,
            stillness: 0.0,
            getup_requested: false,
        }
    }
}

/// A command for a puppet, queued with [`PuppetCommandsExt`].
pub enum PuppetCommand {
    /// A blunt hit: impulse (N·s) on a part, at a world point (default: its origin).
    Impulse {
        part: PuppetPart,
        impulse: Vec3,
        point: Option<Vec3>,
    },
    /// A typed anatomical hit (blunt/slash/pierce/blast) with multi-zone mechanics.
    Hit {
        part: PuppetPart,
        impulse: Vec3,
        point: Option<Vec3>,
        kind: HitKind,
    },
    /// Glancing shoulder/crowd bump (ribcage slip-twist + orienting gaze toward `bumper_pos`).
    Bump {
        part: PuppetPart,
        impulse: Vec3,
        bumper_pos: Option<Vec3>,
    },
    /// Trip / leg sweep: buckles or snags a leg and pitches the body forward into a stumble.
    Trip {
        side: Side,
        impulse: Vec3,
    },
    /// Direct the puppet's head/gaze toward a world point for `duration` seconds.
    LookAt {
        point: Vec3,
        duration: f32,
    },
    /// Shove the whole body (impulse spread over the trunk) and stagger.
    Push { impulse: Vec3 },
    Start(Box<dyn Behaviour>),
    Stop(&'static str),
    /// Leave the animation and balance physically.
    Activate,
    /// Give up balance and fall.
    Fall,
    Kill,
    Revive,
    GetUp,
}

/// Commands waiting for the next physics step.
#[derive(Component, Default)]
pub struct PuppetInbox(pub Vec<PuppetCommand>);

/// Queue commands on a puppet entity.
pub trait PuppetCommandsExt {
    fn puppet(&mut self, command: PuppetCommand) -> &mut Self;

    fn puppet_hit(&mut self, part: PuppetPart, impulse: Vec3, point: Option<Vec3>) -> &mut Self {
        self.puppet(PuppetCommand::Impulse { part, impulse, point })
    }
    fn puppet_hit_kind(
        &mut self,
        part: PuppetPart,
        impulse: Vec3,
        point: Option<Vec3>,
        kind: HitKind,
    ) -> &mut Self {
        self.puppet(PuppetCommand::Hit { part, impulse, point, kind })
    }
    fn puppet_bump(
        &mut self,
        part: PuppetPart,
        impulse: Vec3,
        bumper_pos: Option<Vec3>,
    ) -> &mut Self {
        self.puppet(PuppetCommand::Bump { part, impulse, bumper_pos })
    }
    fn puppet_trip(&mut self, side: Side, impulse: Vec3) -> &mut Self {
        self.puppet(PuppetCommand::Trip { side, impulse })
    }
    fn puppet_look_at(&mut self, point: Vec3, duration: f32) -> &mut Self {
        self.puppet(PuppetCommand::LookAt { point, duration })
    }
    fn puppet_push(&mut self, impulse: Vec3) -> &mut Self {
        self.puppet(PuppetCommand::Push { impulse })
    }
    fn puppet_start(&mut self, behaviour: impl Behaviour) -> &mut Self {
        self.puppet(PuppetCommand::Start(Box::new(behaviour)))
    }
    fn puppet_kill(&mut self) -> &mut Self {
        self.puppet(PuppetCommand::Kill)
    }
}

impl PuppetCommandsExt for EntityCommands<'_> {
    fn puppet(&mut self, command: PuppetCommand) -> &mut Self {
        self.queue(move |mut e: EntityWorldMut| {
            if let Some(mut inbox) = e.get_mut::<PuppetInbox>() {
                inbox.0.push(command);
            }
        });
        self
    }
}

/// What happened to a puppet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PuppetEventKind {
    ModeChanged { from: PuppetMode, to: PuppetMode },
    /// Getting up starts: play the matching get-up clip now.
    GettingUp { face_up: bool },
    /// The puppet entity was moved under the body (recovery or get-up).
    RootMoved,
    /// A body part slammed into the world with significant impulse & speed (audio/VFX/camera shake).
    BodyImpact {
        part: PuppetPart,
        point: Vec3,
        normal: Vec3,
        impulse: f32,
        speed: f32,
    },
    /// A recovery balance step touched down.
    StepTaken {
        side: Side,
        point: Vec3,
    },
    /// Hands contacted a wall or obstacle while bracing.
    WallBraced {
        part: PuppetPart,
        point: Vec3,
        normal: Vec3,
    },
    /// A foot/leg caught an obstacle or was swept mid-stride.
    Tripped {
        side: Side,
    },
}

#[derive(Message, Clone, Copy, Debug)]
pub struct PuppetEvent {
    pub puppet: Entity,
    pub kind: PuppetEventKind,
}

fn enter(
    state: &mut PuppetState,
    to: PuppetMode,
    puppet: Entity,
    events: &mut MessageWriter<PuppetEvent>,
) {
    if state.mode == to {
        return;
    }
    let from = state.mode;
    state.mode = to;
    state.time_in_mode = 0.0;
    state.stillness = 0.0;
    events.write(PuppetEvent {
        puppet,
        kind: PuppetEventKind::ModeChanged { from, to },
    });
}

/// Applies queued commands (impulses go straight to the bodies).
#[allow(clippy::type_complexity)]
pub fn process_inbox(
    mut puppets: Query<(
        Entity,
        &mut PuppetInbox,
        &mut PuppetState,
        &mut PuppetBehaviours,
        &mut PuppetNeuroState,
        &PuppetConfig,
        Option<&PuppetRig>,
        Option<&PuppetSense>,
    )>,
    mut forces: Query<Forces>,
    mut events: MessageWriter<PuppetEvent>,
) {
    for (puppet, mut inbox, mut state, mut behaviours, mut neuro, config, rig, sense) in &mut puppets {
        let Some(rig) = rig else { continue };
        let Some(sense) = sense else { continue };
        // Wait until at least one physics step has settled so Avian populates
        // body transforms and foot ground contacts before the first impulse.
        if sense.steps < 1 {
            continue;
        }
        for command in inbox.0.drain(..) {
            match command {
                PuppetCommand::Impulse { part, impulse, point } => {
                    apply_hit_command(
                        puppet,
                        &mut state,
                        &mut behaviours,
                        &mut neuro,
                        config,
                        rig,
                        sense,
                        &mut forces,
                        &mut events,
                        part,
                        impulse,
                        point,
                        HitKind::Blunt,
                    );
                }
                PuppetCommand::Hit { part, impulse, point, kind } => {
                    apply_hit_command(
                        puppet,
                        &mut state,
                        &mut behaviours,
                        &mut neuro,
                        config,
                        rig,
                        sense,
                        &mut forces,
                        &mut events,
                        part,
                        impulse,
                        point,
                        kind,
                    );
                }
                PuppetCommand::Bump { part, impulse, bumper_pos } => {
                    // Spread glancing bump 70% on struck shoulder/part + 30% on upper spine
                    // so the ribcage turns cleanly without snapping the shoulder joint.
                    if part == PuppetPart::SpineUpper || part == PuppetPart::SpineLower {
                        if let Ok(mut f) = forces.get_mut(rig.part(part).body) {
                            f.apply_linear_impulse(impulse);
                        }
                    } else {
                        if let Ok(mut f) = forces.get_mut(rig.part(part).body) {
                            f.apply_linear_impulse(impulse * 0.70);
                        }
                        if let Ok(mut f) = forces.get_mut(rig.part(PuppetPart::SpineUpper).body) {
                            f.apply_linear_impulse(impulse * 0.30);
                        }
                    }
                    let mag = impulse.length();
                    let intensity = (mag / (rig.total_mass * 0.65)).clamp(0.15, 1.0);
                    if let Some(bp) = bumper_pos {
                        neuro.set_gaze(bp, priority::GAZE + 2, 0.70, sense.time);
                    }
                    if matches!(state.mode, PuppetMode::Animated | PuppetMode::Active) && config.react_to_hits {
                        let full = state.mode == PuppetMode::Active
                            || (config.partial_hit_impulse.is_finite() && mag >= config.partial_hit_impulse);
                        if full {
                            enter(&mut state, PuppetMode::Active, puppet, &mut events);
                        }
                        if let Some(existing) = behaviours.get_mut::<BumpReaction>(BumpReaction::NAME) {
                            existing.stack(part, impulse, bumper_pos, intensity);
                            if full {
                                existing.partial = false;
                            }
                        } else {
                            let mut b = BumpReaction::new(part, impulse, intensity)
                                .with_bumper_pos(bumper_pos)
                                .with_delay(config.reaction_delay * 0.70);
                            if full {
                                b = b.full_body();
                            }
                            behaviours.start(Box::new(b));
                        }
                    }
                }
                PuppetCommand::Trip { side, impulse } => {
                    let calf = PuppetPart::calf(side);
                    let dir = impulse.reject_from(sense.up).normalize_or(sense.heading);
                    let mag = impulse.length().max(24.0);
                    // Snag the lower leg backward/sideways while pitching the chest forward!
                    if let Ok(mut f) = forces.get_mut(rig.part(calf).body) {
                        f.apply_linear_impulse(-dir * (mag * 0.40));
                    }
                    if let Ok(mut f) = forces.get_mut(rig.part(PuppetPart::SpineUpper).body) {
                        f.apply_linear_impulse(dir * (mag * 0.85));
                    }
                    if matches!(state.mode, PuppetMode::Animated | PuppetMode::Active) {
                        enter(&mut state, PuppetMode::Active, puppet, &mut events);
                        let sev = (mag / (rig.total_mass * 1.1)).clamp(0.45, 1.0);
                        behaviours.start(Box::new(
                            HitReaction::new(calf, dir, sev).with_delay(0.02),
                        ));
                        behaviours.start(Box::new(ArmsWindmill::default()));
                        events.write(PuppetEvent {
                            puppet,
                            kind: PuppetEventKind::Tripped { side },
                        });
                    }
                }
                PuppetCommand::LookAt { point, duration } => {
                    neuro.set_gaze(point, priority::GAZE + 1, duration, sense.time);
                }
                PuppetCommand::Push { impulse } => {
                    let parts = [PuppetPart::Pelvis, PuppetPart::SpineLower, PuppetPart::SpineUpper];
                    for p in parts {
                        if let Ok(mut f) = forces.get_mut(rig.part(p).body) {
                            f.apply_linear_impulse(impulse / parts.len() as f32);
                        }
                    }
                    if matches!(state.mode, PuppetMode::Animated | PuppetMode::Active) {
                        enter(&mut state, PuppetMode::Active, puppet, &mut events);
                        let mass = rig.total_mass.max(1.0);
                        let dv = impulse.length() / mass;
                        if dv > 1.2 {
                            behaviours.stop(BodyBalance::NAME);
                            behaviours.start(Box::new(StaggerFall::new(impulse)));
                        }
                        if dv > 0.55 {
                            behaviours.start(Box::new(ArmsWindmill::default()));
                        }
                    }
                }
                PuppetCommand::Start(b) => behaviours.start(b),
                PuppetCommand::Stop(name) => behaviours.stop(name),
                PuppetCommand::Activate => {
                    if state.mode == PuppetMode::Animated {
                        enter(&mut state, PuppetMode::Active, puppet, &mut events);
                    }
                }
                PuppetCommand::Fall => {
                    if matches!(state.mode, PuppetMode::Animated | PuppetMode::Active) {
                        enter(&mut state, PuppetMode::Falling, puppet, &mut events);
                    }
                }
                PuppetCommand::Kill => {
                    behaviours.clear();
                    behaviours.start(Box::new(Relax::with_collapse(config.dying_collapse_time)));
                    enter(&mut state, PuppetMode::Dead, puppet, &mut events);
                }
                PuppetCommand::Revive | PuppetCommand::GetUp => {
                    if matches!(state.mode, PuppetMode::Dead | PuppetMode::Down) {
                        behaviours.clear();
                        enter(&mut state, PuppetMode::Down, puppet, &mut events);
                        // The get-up itself starts from `update_states` (it needs the sense).
                        state.getup_requested = true;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_hit_command(
    puppet: Entity,
    state: &mut PuppetState,
    behaviours: &mut PuppetBehaviours,
    neuro: &mut PuppetNeuroState,
    config: &PuppetConfig,
    rig: &PuppetRig,
    sense: &PuppetSense,
    forces: &mut Query<Forces>,
    events: &mut MessageWriter<PuppetEvent>,
    part: PuppetPart,
    impulse: Vec3,
    point: Option<Vec3>,
    kind: HitKind,
) {
    if let Ok(mut f) = forces.get_mut(rig.part(part).body) {
        match point {
            Some(p) => f.apply_linear_impulse_at_point(impulse, p),
            None => f.apply_linear_impulse(impulse),
        }
    }
    let body = sense.body(part);
    let local_offset = point
        .map(|p| body.rot.inverse() * (p - body.pos))
        .unwrap_or(Vec3::ZERO);
    let mag = impulse.length();
    let severity = (mag / (rig.total_mass * 0.90)).clamp(0.0, 1.0);
    let trauma = neuro.record_hit(part, local_offset, impulse, severity, sense.time);

    let escalate_active = config.partial_hit_impulse.is_finite()
        && (mag >= config.partial_hit_impulse
            || trauma >= 0.65
            || (matches!(part.zone(), BodyZone::Leg(_)) && severity >= 0.32));

    match state.mode {
        PuppetMode::Animated if config.react_to_hits => {
            let partial = !escalate_active;
            if !partial {
                enter(state, PuppetMode::Active, puppet, events);
                if severity > 0.30 || trauma > 0.70 {
                    behaviours.start(Box::new(ArmsWindmill::default()));
                }
            }
            if config.reach_for_wound
                && severity >= 0.24
                && !matches!(part.zone(), BodyZone::Leg(_) | BodyZone::Pelvis)
            {
                behaviours.start(Box::new(ReachForWound::new(part, local_offset, severity)));
            }
            if let Some(existing) = behaviours.get_mut::<HitReaction>(HitReaction::NAME) {
                existing.stack(part, impulse, local_offset, kind, severity, partial);
            } else {
                let mut hr = HitReaction::new(part, impulse, severity)
                    .with_kind(kind)
                    .with_local_offset(local_offset)
                    .with_delay(config.reaction_delay);
                if partial {
                    hr = hr.partial();
                }
                behaviours.start(Box::new(hr));
            }
        }
        PuppetMode::Active => {
            if let Some(existing) = behaviours.get_mut::<HitReaction>(HitReaction::NAME) {
                existing.stack(part, impulse, local_offset, kind, severity, false);
            } else {
                behaviours.start(Box::new(
                    HitReaction::new(part, impulse, severity)
                        .with_kind(kind)
                        .with_local_offset(local_offset)
                        .with_delay(config.reaction_delay),
                ));
            }
            if config.reach_for_wound
                && severity >= 0.24
                && !matches!(part.zone(), BodyZone::Leg(_) | BodyZone::Pelvis)
            {
                behaviours.start(Box::new(ReachForWound::new(part, local_offset, severity)));
            }
        }
        _ => {}
    }
}

/// Mode transitions, automatic reflexes, impact events, and pin/blend weights.
#[allow(clippy::type_complexity)]
pub fn update_states(
    mut puppets: Query<(
        Entity,
        &mut PuppetState,
        &mut PuppetBehaviours,
        &mut PuppetBlend,
        &mut PuppetNeuroState,
        &PuppetRequests,
        &PuppetSense,
        &PuppetRig,
        &PuppetConfig,
        &PuppetTargets,
    )>,
    mut transforms: Query<&mut Transform>,
    parents: Query<&ChildOf>,
    mut events: MessageWriter<PuppetEvent>,
) {
    for (puppet, mut state, mut behaviours, mut blend, mut neuro, req, sense, rig, config, targets) in
        &mut puppets
    {
        let dt = sense.dt;
        state.time_in_mode += dt;

        // ---- Emit debounced `BodyImpact` events & process behaviour signals --
        let thresh = config.impact_event_threshold;
        if thresh.is_finite() && thresh > 0.0 && state.mode != PuppetMode::Animated {
            for part in PuppetPart::ALL {
                let i = part.index();
                let is_foot = matches!(part, PuppetPart::FootL | PuppetPart::FootR);
                let min_speed = if is_foot { 1.6 } else { 0.70 };
                if sense.contacts[i]
                    && sense.contact_impulses[i] >= thresh
                    && sense.contact_speeds[i] >= min_speed
                    && neuro.impact_cooldown[i] <= 0.0
                {
                    neuro.impact_cooldown[i] = 0.22;
                    neuro.fatigue = (neuro.fatigue + 0.08).clamp(0.0, 1.0);
                    events.write(PuppetEvent {
                        puppet,
                        kind: PuppetEventKind::BodyImpact {
                            part,
                            point: sense.contact_points[i],
                            normal: sense.contact_normals[i],
                            impulse: sense.contact_impulses[i],
                            speed: sense.contact_speeds[i],
                        },
                    });
                }
            }
        }

        let mut lost = false;
        let mut recovered = false;
        let mut need_windmill = false;
        for &sig in &req.signals {
            match sig {
                PuppetSignal::LostBalance => lost = true,
                PuppetSignal::Recovered => recovered = true,
                PuppetSignal::NeedWindmill => need_windmill = true,
                PuppetSignal::StepTaken { side, point } => {
                    neuro.fatigue = (neuro.fatigue + 0.07).clamp(0.0, 1.0);
                    events.write(PuppetEvent {
                        puppet,
                        kind: PuppetEventKind::StepTaken { side, point },
                    });
                }
                PuppetSignal::Tripped { side } => {
                    neuro.fatigue = (neuro.fatigue + 0.14).clamp(0.0, 1.0);
                    events.write(PuppetEvent {
                        puppet,
                        kind: PuppetEventKind::Tripped { side },
                    });
                }
                PuppetSignal::WallBraced { part, point, normal } => {
                    events.write(PuppetEvent {
                        puppet,
                        kind: PuppetEventKind::WallBraced { part, point, normal },
                    });
                }
            }
        }

        let is_calm_animated = state.mode == PuppetMode::Animated && behaviours.active.is_empty();
        neuro.tick(dt, sense.time, is_calm_animated);

        let speed = sense.com_vel.length();
        let still = speed < 0.3 && sense.body_touching();
        state.stillness = if still { state.stillness + dt } else { 0.0 };

        let balancing = behaviours.has(BodyBalance::NAME) || behaviours.has(StaggerFall::NAME);
        match state.mode {
            PuppetMode::Animated => {}
            PuppetMode::Active => {
                if config.wall_brace && sense.nearby_wall.is_some() {
                    behaviours.start_if_absent(Box::new(WallBrace::default()));
                }
                if need_windmill {
                    behaviours.start_if_absent(Box::new(ArmsWindmill::default()));
                }
                if lost || (!balancing && sense.trunk_tilt > 1.0) || sense.is_high_fall(rig.height) {
                    enter(&mut state, PuppetMode::Falling, puppet, &mut events);
                } else if recovered && state.time_in_mode > 0.3 {
                    behaviours.stop(BodyBalance::NAME);
                    behaviours.stop(StaggerFall::NAME);
                    behaviours.stop(ArmsWindmill::NAME);
                    behaviours.stop(WallBrace::NAME);
                    behaviours.stop(ReachForWound::NAME);
                    if move_root(puppet, rig, sense, targets, &mut transforms, &parents) {
                        events.write(PuppetEvent { puppet, kind: PuppetEventKind::RootMoved });
                    }
                    enter(&mut state, PuppetMode::Animated, puppet, &mut events);
                } else if config.auto_balance && !balancing {
                    behaviours.start(Box::new(BodyBalance::default()));
                }
            }
            PuppetMode::Falling => {
                behaviours.stop(BodyBalance::NAME);
                behaviours.stop(StaggerFall::NAME);
                behaviours.stop(ArmsWindmill::NAME);
                behaviours.stop(ReachForWound::NAME);
                behaviours.start_if_absent(Box::new(CatchFall::default()));
                if config.wall_brace && sense.nearby_wall.is_some() {
                    behaviours.start_if_absent(Box::new(WallBrace::default()));
                }
                if (state.time_in_mode > 0.4 && state.stillness > 0.3) || state.time_in_mode > 5.0 {
                    behaviours.stop(CatchFall::NAME);
                    behaviours.stop(WallBrace::NAME);
                    enter(&mut state, PuppetMode::Down, puppet, &mut events);
                }
            }
            PuppetMode::Down => {
                behaviours.stop(CatchFall::NAME);
                behaviours.stop(WallBrace::NAME);
                if config.writhe_when_down {
                    behaviours.start_if_absent(Box::new(BodyWrithe::default()));
                }
                let auto = config.getup_delay >= 0.0 && state.time_in_mode >= config.getup_delay;
                if (auto && state.stillness > 0.3) || state.getup_requested {
                    state.getup_requested = false;
                    behaviours.clear();
                    // Lying on the back when the chest faces up.
                    state.face_up = sense.face_up_dot > 0.0;
                    if config.move_root_on_getup
                        && move_root(puppet, rig, sense, targets, &mut transforms, &parents)
                    {
                        events.write(PuppetEvent { puppet, kind: PuppetEventKind::RootMoved });
                    }
                    let face_up = state.face_up;
                    if config.physical_getup {
                        behaviours.start(Box::new(PhysicalGetUp::new(face_up, config.getup_duration)));
                    }
                    enter(&mut state, PuppetMode::GettingUp, puppet, &mut events);
                    events.write(PuppetEvent { puppet, kind: PuppetEventKind::GettingUp { face_up } });
                }
            }
            PuppetMode::GettingUp => {
                if state.time_in_mode >= config.getup_duration {
                    behaviours.stop(PhysicalGetUp::NAME);
                    enter(&mut state, PuppetMode::Animated, puppet, &mut events);
                }
            }
            PuppetMode::Dead => {
                behaviours.start_if_absent(Box::new(Relax::with_collapse(config.dying_collapse_time)));
            }
        }

        // ---- Pins and visible blend per mode --------------------------------
        let (pin_goal, phys_goal, pelvis_pin) = match state.mode {
            PuppetMode::Animated => (1.0, 0.0, 1.0),
            PuppetMode::Active => (0.0, 1.0, config.balance_assist),
            PuppetMode::Falling | PuppetMode::Down | PuppetMode::Dead => (0.0, 1.0, 0.0),
            PuppetMode::GettingUp => {
                let u = (state.time_in_mode / config.getup_duration.max(0.05)).clamp(0.0, 1.0);
                let p = smooth(u);
                // When `PhysicalGetUp` is driving the limbs early, keep physics visually
                // dominant longer (`0..0.55`) before blending seamlessly into animation!
                let phys = if config.physical_getup {
                    1.0 - smooth((u - 0.35) / 0.65)
                } else {
                    1.0 - p
                };
                (p, phys, p)
            }
        };
        let rate = if state.mode == PuppetMode::GettingUp {
            f32::INFINITY
        } else {
            dt / config.blend_time.max(1e-3)
        };
        for part in PuppetPart::ALL {
            let i = part.index();
            let goal = if part == PuppetPart::Pelvis { pelvis_pin.max(pin_goal) } else { pin_goal };
            state.pins[i] = if goal < state.pins[i] {
                goal
            } else {
                approach(state.pins[i], goal, rate)
            };
            let want_w = phys_goal.max(req.physics_weight[i]);
            let w_step = if want_w > blend.physics_weight[i] {
                rate.max(dt * 30.0)
            } else {
                rate.max(dt * 6.0)
            };
            blend.physics_weight[i] = approach(blend.physics_weight[i], want_w, w_step);
        }
    }
}

fn approach(x: f32, goal: f32, step: f32) -> f32 {
    if (goal - x).abs() <= step { goal } else { x + (goal - x).signum() * step }
}

/// Moves the puppet entity so the animated pose lands under the body: pelvis
/// over pelvis, facing the body's heading, feet on the ground. Only for
/// puppets without a parent (in world space).
fn move_root(
    puppet: Entity,
    rig: &PuppetRig,
    sense: &PuppetSense,
    targets: &PuppetTargets,
    transforms: &mut Query<&mut Transform>,
    parents: &Query<&ChildOf>,
) -> bool {
    if parents.get(puppet).is_ok() {
        return false;
    }
    let Ok(mut root) = transforms.get_mut(puppet) else { return false };
    let up = sense.up;
    let pelvis_rot = targets.rot(PuppetPart::Pelvis);
    let anim_heading = (pelvis_rot * rig.part(PuppetPart::Pelvis).forward_local)
        .reject_from(up)
        .normalize_or(sense.heading);
    let yaw = crate::rig::yaw_between(anim_heading, sense.heading, up);
    let anim_pelvis = targets.pos(PuppetPart::Pelvis);
    let offset = (yaw * (anim_pelvis - root.translation)).reject_from(up);
    let body_pelvis = sense.body(PuppetPart::Pelvis).pos;
    let raw_ground_shift = sense.ground
        - (targets.pos(PuppetPart::FootL).dot(up).min(targets.pos(PuppetPart::FootR).dot(up))
            - rig.ankle_height);
    let ground_shift = raw_ground_shift.clamp(-0.30, 0.60);
    let flat_target = (body_pelvis - offset).reject_from(up);
    let height = root.translation.dot(up) + ground_shift;
    root.translation = flat_target + up * height;
    root.rotation = (yaw * root.rotation).normalize();
    true
}
