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

/// Override do estado escolhido pelo driver (`drive_player_animation`).
///
/// O jogo (Vael) escreve-o quando o tnua sabe algo que o movimento medido
/// não revela: a ação de salto em `FallSection`, um dash em curso, um
/// deslize de parede. `None` devolve a decisão ao driver (idle/walk/run/
/// água seguem o deslocamento real). Atualizado por frame.
#[derive(Debug, Clone, Copy, Default, Reflect, Component)]
#[type_path = "viber::animation"]
pub struct AnimStateOverride(pub Option<AnimState>);

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
    /// A nadar à superfície (em movimento) — ver [`water_state`].
    Swim,
    /// A boiar/tear água, parado. Substitui Jump/Fall enquanto o herói está
    /// submerso até ao peito.
    SwimIdle,
}

impl AnimState {
    /// Todos os estados, na ordem de [`Self::index`].
    pub const ALL: [AnimState; 7] = [
        AnimState::Idle,
        AnimState::Walk,
        AnimState::Run,
        AnimState::Jump,
        AnimState::Fall,
        AnimState::Swim,
        AnimState::SwimIdle,
    ];

    /// Índice estável do estado — a chave da LUT de nodes do
    /// [`CharacterAnimator`].
    pub fn index(self) -> usize {
        match self {
            AnimState::Idle => 0,
            AnimState::Walk => 1,
            AnimState::Run => 2,
            AnimState::Jump => 3,
            AnimState::Fall => 4,
            AnimState::Swim => 5,
            AnimState::SwimIdle => 6,
        }
    }

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
            AnimState::Swim => &["swim", "swimfwd", "swimforward", "swimfast"],
            AnimState::SwimIdle => &["swimidle", "tread", "treadwater", "float"],
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
            AnimState::Swim => &[AnimState::SwimIdle, AnimState::Walk, AnimState::Idle],
            AnimState::SwimIdle => &[AnimState::Swim, AnimState::Idle],
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
    resolve_normalized(&normalized, state)
}

/// [`resolve_clip`] sobre uma lista já normalizada — o caminho quente.
pub fn resolve_normalized(normalized: &[String], state: AnimState) -> Option<usize> {
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
    /// Nomes dos clips já normalizados, paralelo a `clip_names`.
    ///
    /// `resolve_clip` normalizava a lista INTEIRA em cada chamada, e
    /// `play_state` corre por personagem por frame — duas `String` por clip por
    /// frame. A lista é derivada no bind e não entra na reflexão.
    #[reflect(ignore)]
    pub normalized: Vec<String>,
    /// O node resolvido (com a cadeia de fallbacks) de cada [`AnimState`],
    /// indexado por [`AnimState::index`]. Uma leitura de array por frame.
    #[reflect(ignore)]
    pub state_nodes: [Option<AnimationNodeIndex>; AnimState::ALL.len()],
}

impl CharacterAnimator {
    /// Constrói o animador e as tabelas derivadas (nomes normalizados e o node
    /// de cada estado) — uma vez por bind; o caminho por frame é um índice.
    ///
    /// Os restantes campos ficam neutros: quem chama preenche o estado que
    /// sobrevive a um re-bind (`state`/`current`/`speed`/`last_pos`/`air_time`).
    pub fn new(
        clip_names: Vec<String>,
        nodes: Vec<AnimationNodeIndex>,
        durations: Vec<f32>,
        player: Entity,
        locomotion: Option<LocomotionBlendNodes>,
    ) -> Self {
        let normalized: Vec<String> = clip_names.iter().map(|name| normalize_clip_name(name)).collect();
        let mut state_nodes = [None; AnimState::ALL.len()];
        for state in AnimState::ALL {
            state_nodes[state.index()] = resolve_with_fallback(&clip_names, state)
                .and_then(|index| nodes.get(index).copied());
        }
        Self {
            clip_names,
            nodes,
            durations,
            player,
            state: None,
            current: None,
            action_time: 0.0,
            locked: false,
            air_time: 0.0,
            speed: 0.0,
            last_pos: None,
            locomotion,
            normalized,
            state_nodes,
        }
    }

    /// Recalcula as tabelas derivadas (só depois de mexer em `clip_names` ou
    /// `nodes`, o que o bind já faz).
    pub fn refresh_clip_tables(&mut self) {
        self.normalized = self
            .clip_names
            .iter()
            .map(|name| normalize_clip_name(name))
            .collect();
        for state in AnimState::ALL {
            self.state_nodes[state.index()] = resolve_with_fallback(&self.clip_names, state)
                .and_then(|index| self.nodes.get(index).copied());
        }
    }

    /// Graph node for a state, honouring the fallback chain.
    ///
    /// Era `resolve_with_fallback` (que normaliza a lista de clips) em cada
    /// chamada; agora é uma leitura da LUT calculada no bind.
    pub fn node_for(&self, state: AnimState) -> Option<AnimationNodeIndex> {
        self.state_nodes[state.index()]
    }

    /// Graph node for the first clip whose name satisfies `pred`, matched
    /// against the *normalized* name (já calculado no bind).
    pub fn node_matching(&self, pred: impl Fn(&str) -> bool) -> Option<AnimationNodeIndex> {
        self.normalized
            .iter()
            .position(|name| pred(name))
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

#[cfg(test)]
mod lut_tests {
    use super::*;

    /// A LUT por estado tem de dar exactamente o mesmo que a resolução com
    /// fallbacks — é o que permite `node_for` ser uma leitura de array no
    /// caminho por frame (antes normalizava a lista de clips inteira).
    #[test]
    fn the_state_lut_matches_full_resolution() {
        let names: Vec<String> = vec![
            "Animator3D_BreatheIdle".into(),
            "walk".into(),
            "RunForward".into(),
            "swordidle".into(),
        ];
        let nodes: Vec<AnimationNodeIndex> = (0..names.len())
            .map(|i| AnimationNodeIndex::new(i + 1))
            .collect();
        let animator = CharacterAnimator::new(
            names.clone(),
            nodes.clone(),
            vec![1.0; names.len()],
            Entity::PLACEHOLDER,
            None,
        );
        assert_eq!(animator.normalized[0], "breatheidle");
        assert_eq!(animator.normalized[2], "runforward");
        for state in AnimState::ALL {
            let expected = resolve_with_fallback(&names, state).map(|index| nodes[index]);
            assert_eq!(animator.node_for(state), expected, "{state:?}");
        }
        // `Idle` casa `breatheidle` (o prefixo `animator3d` sai) e não
        // `swordidle`, que só casaria por substring.
        assert_eq!(animator.node_for(AnimState::Idle), Some(AnimationNodeIndex::new(1)));
        assert_eq!(animator.state_nodes.len(), AnimState::ALL.len());
        assert_eq!(AnimState::ALL[AnimState::Swim.index()], AnimState::Swim);
    }

    /// `node_matching` compara contra os nomes já normalizados e não realoca.
    #[test]
    fn node_matching_uses_the_normalized_names() {
        let nodes = vec![AnimationNodeIndex::new(7)];
        let mut animator = CharacterAnimator::new(
            vec!["Animator3D_Wave".into()],
            nodes,
            vec![1.0],
            Entity::PLACEHOLDER,
            None,
        );
        assert_eq!(animator.node_matching(|name| name == "wave"), Some(AnimationNodeIndex::new(7)));
        // Depois de mexer nos clips, `refresh_clip_tables` volta a alinhar.
        animator.clip_names = vec!["jump".into()];
        animator.refresh_clip_tables();
        assert_eq!(animator.normalized, vec!["jump".to_string()]);
        assert_eq!(animator.node_for(AnimState::Jump), Some(AnimationNodeIndex::new(7)));
        assert_eq!(animator.node_for(AnimState::Idle), None);
    }
}
