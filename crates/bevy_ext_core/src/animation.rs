//! Contrato de animação de personagens — os tipos que atravessam a fronteira
//! entre os sistemas de animação (monólito), o ragdoll/IK
//! (`bevy_ext_physics`) e a folhagem (`bevy_ext_engine`).
//!
//! Extraído de `viber/src/animation.rs`: tudo o que os consumidores de fora
//! do monólito leem ou escrevem. Os SISTEMAS (bind, drive, freeze) continuam
//! no monólito — aqui vive só o contrato. Os TypePath históricos estão
//! congelados com `#[type_path = "viber::animation"]`.

use bevy::animation::graph::{AnimationGraphHandle, AnimationNodeIndex};
use bevy::prelude::*;

/// Nós do blend 1D de locomoção, quando o ficheiro tem idle, walk e run distintos.
#[derive(Debug, Clone, Copy, Reflect)]
#[type_path = "viber::animation"]
pub struct LocomotionBlendNodes {
    pub blend: AnimationNodeIndex,
    pub idle: AnimationNodeIndex,
    pub walk: AnimationNodeIndex,
    pub run: AnimationNodeIndex,
}

/// A gameplay animation state, independent of what the file calls its clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Reflect)]
#[type_path = "viber::animation"]
pub enum AnimState {
    #[default]
    Idle,
    Walk,
    Run,
    Jump,
    Fall,
}

impl AnimState {
    /// Clip names to look for, best first.
    ///
    /// Every entry is already normalized (see [`normalize_clip_name`]). The
    /// lists are deliberately broad: the same state is spelled differently by
    /// the hero rig, the retargeted creature rigs and hand-authored props.
    pub fn candidates(self) -> &'static [&'static str] {
        match self {
            AnimState::Idle => &["idle", "breatheidle", "idlebreathe", "stand", "idle01"],
            AnimState::Walk => &["walk", "walkforward", "walking"],
            AnimState::Run => &["run", "sprint", "runforward", "jog"],
            AnimState::Jump => &["jump", "jumpup", "jumpstart"],
            AnimState::Fall => &["fall", "falling", "jumpair", "jump"],
        }
    }

    /// States tried in order when this one has no clip in the file.
    ///
    /// A rig without `run` should jog with `walk` rather than freeze, and a rig
    /// without airborne clips should keep its ground pose.
    pub fn fallbacks(self) -> &'static [AnimState] {
        match self {
            AnimState::Idle => &[],
            AnimState::Walk => &[AnimState::Run, AnimState::Idle],
            AnimState::Run => &[AnimState::Walk, AnimState::Idle],
            AnimState::Jump => &[AnimState::Fall, AnimState::Idle],
            AnimState::Fall => &[AnimState::Jump, AnimState::Idle],
        }
    }
}

/// Lowercases a clip name and drops everything that is not a letter or digit,
/// plus the pipeline's tool prefixes.
///
/// `"Animator3D_BreatheIdle"` and `"breathe idle"` both become `"breatheidle"`,
/// so one candidate list matches every rig the pipeline produces.
pub fn normalize_clip_name(name: &str) -> String {
    let lowered: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    // Retarget output is prefixed with the tool name; `mixamocom` shows up in
    // third-party rigs that went through the same path.
    for prefix in ["animator3d", "mixamocom", "armature"] {
        if let Some(rest) = lowered.strip_prefix(prefix)
            && !rest.is_empty()
        {
            return rest.to_string();
        }
    }
    lowered
}

/// Chooses the clip index for a state out of a file's clip names.
///
/// Exact normalized matches win over substring matches, so a file containing
/// both `idle` and `swordidle` picks `idle` for [`AnimState::Idle`] instead of
/// whichever happened to be first.
pub fn resolve_clip(names: &[String], state: AnimState) -> Option<usize> {
    let normalized: Vec<String> = names.iter().map(|n| normalize_clip_name(n)).collect();
    for candidate in state.candidates() {
        if let Some(i) = normalized.iter().position(|n| n == candidate) {
            return Some(i);
        }
    }
    for candidate in state.candidates() {
        if let Some(i) = normalized.iter().position(|n| n.contains(candidate)) {
            return Some(i);
        }
    }
    None
}

/// Resolves a state through its fallbacks, so a missing clip degrades instead
/// of leaving the character in its bind pose.
pub fn resolve_with_fallback(names: &[String], state: AnimState) -> Option<usize> {
    resolve_clip(names, state).or_else(|| {
        state
            .fallbacks()
            .iter()
            .find_map(|next| resolve_clip(names, *next))
    })
}

/// A bound character: clip names plus the graph node for each one, and the
/// small amount of per-character playback state the drivers need.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::animation"]
pub struct CharacterAnimator {
    /// Clip names as the file spells them, in file order.
    pub clip_names: Vec<String>,
    /// Graph node per clip, parallel to `clip_names`.
    pub nodes: Vec<AnimationNodeIndex>,
    /// Clip length in seconds, parallel to `clip_names`. Drives how long a
    /// one-shot action owns the rig.
    pub durations: Vec<f32>,
    /// The entity carrying the `AnimationPlayer` (a scene descendant).
    pub player: Entity,
    /// Locomotion state the motion driver last asked for. Kept across actions
    /// so the hysteresis in the motion drivers has memory.
    pub state: Option<AnimState>,
    /// Graph node actually asserted on the player. The clip is only replayed
    /// when this changes, which is what keeps playback continuous.
    pub current: Option<AnimationNodeIndex>,
    /// Seconds left in the one-shot action that owns the rig, if any.
    pub action_time: f32,
    /// A terminal clip (death) holds the last pose forever.
    pub locked: bool,
    /// Seconds spent off the ground (airborne debounce).
    pub air_time: f32,
    /// Low-passed planar speed (m/s) measured from real displacement.
    pub speed: f32,
    /// Previous world position, for that measurement.
    pub last_pos: Option<Vec3>,
    /// Blend idle/walk/run, quando o ficheiro tem os três clips.
    pub locomotion: Option<LocomotionBlendNodes>,
}

impl CharacterAnimator {
    /// Graph node for a state, honouring the fallback chain.
    pub fn node_for(&self, state: AnimState) -> Option<AnimationNodeIndex> {
        resolve_with_fallback(&self.clip_names, state).and_then(|i| self.nodes.get(i).copied())
    }

    /// Graph node for the first clip whose name satisfies `pred`, matched
    /// against the *normalized* name.
    pub fn node_matching(&self, pred: impl Fn(&str) -> bool) -> Option<AnimationNodeIndex> {
        self.clip_names
            .iter()
            .position(|name| pred(&normalize_clip_name(name)))
            .and_then(|i| self.nodes.get(i).copied())
    }

    /// Length in seconds of the clip behind `node`.
    pub fn duration_of(&self, node: AnimationNodeIndex) -> f32 {
        self.nodes
            .iter()
            .position(|n| *n == node)
            .and_then(|i| self.durations.get(i).copied())
            .unwrap_or(0.0)
    }

    /// True while a one-shot or terminal clip owns the rig.
    pub fn is_busy(&self) -> bool {
        self.locked || self.action_time > 0.0
    }
}

/// The graph handle of a rig parked by the distance cull (`freeze_distant_animation`).
///
/// Bevy's `animate_targets` evaluates every curve of every bone of every
/// player each frame — `paused` included, it only stops the clock — and the
/// written bone transforms then dirty `transform_propagate` and the skin
/// upload. Taking the `AnimationGraphHandle` away is what makes a rig cost
/// nothing: its targets bail on the first lookup and the player's clock
/// stops, so the clip resumes where it left off when the handle comes back.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::animation"]
pub struct FrozenAnimation(pub AnimationGraphHandle);
