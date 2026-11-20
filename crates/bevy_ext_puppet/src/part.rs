//! Anatomy of the puppet: the 16 physical parts, how they map to skeleton
//! bones, their mass fractions, shapes, joint types and muscle coefficients.
//!
//! Parts are ordered parents-first, so iterating [`PuppetPart::ALL`] visits a
//! parent before any of its children (the write-back and the inertia sums rely
//! on it). The joint of a part is the one connecting it to its parent; the
//! pelvis is the root and has none.

use bevy::prelude::*;

/// Number of physical parts in a puppet.
pub const PART_COUNT: usize = 16;

/// One rigid body of the puppet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
#[repr(u8)]
pub enum PuppetPart {
    Pelvis,
    SpineLower,
    SpineUpper,
    Head,
    UpperArmL,
    LowerArmL,
    HandL,
    UpperArmR,
    LowerArmR,
    HandR,
    ThighL,
    CalfL,
    FootL,
    ThighR,
    CalfR,
    FootR,
}

/// Body side of a limb part.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
    pub fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
}

/// Limb groups. Behaviours usually address a whole limb.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
pub enum Limb {
    Spine,
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
}

impl Limb {
    pub const ALL: [Limb; 5] = [
        Limb::Spine,
        Limb::LeftArm,
        Limb::RightArm,
        Limb::LeftLeg,
        Limb::RightLeg,
    ];

    pub fn arm(side: Side) -> Self {
        match side {
            Side::Left => Limb::LeftArm,
            Side::Right => Limb::RightArm,
        }
    }

    pub fn leg(side: Side) -> Self {
        match side {
            Side::Left => Limb::LeftLeg,
            Side::Right => Limb::RightLeg,
        }
    }

    /// Parts of the limb, root first.
    pub fn parts(self) -> &'static [PuppetPart] {
        use PuppetPart::*;
        match self {
            Limb::Spine => &[Pelvis, SpineLower, SpineUpper, Head],
            Limb::LeftArm => &[UpperArmL, LowerArmL, HandL],
            Limb::RightArm => &[UpperArmR, LowerArmR, HandR],
            Limb::LeftLeg => &[ThighL, CalfL, FootL],
            Limb::RightLeg => &[ThighR, CalfR, FootR],
        }
    }
}

/// Collision shape of a part (in the part's body frame, see `rig`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PartShape {
    /// Capsule spanning the segment from the bone to its end point.
    Segment,
    /// Capsule spanning the two hip joints (pelvis) or the two shoulders (chest).
    Across,
    /// Flat box under the ankle, from heel to toe.
    FootBox,
}

/// How a part is jointed to its parent.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum JointKind {
    /// Ball-and-socket: a swing cone around an anatomically biased direction
    /// (`bias_forward` pitches the cone forward, `bias_outward` abducts it away
    /// from the midline) and a twist range.
    Ball {
        swing: f32,
        twist: f32,
        bias_forward: f32,
        bias_outward: f32,
    },
    /// Hinge around the knee/elbow flexion axis, `[min, max]` flexion.
    Hinge { min: f32, max: f32, flexion: Flexion },
}

/// Anatomical region of a body part (drives zone-specific hit, bump, and wound mechanics).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
pub enum BodyZone {
    Head,
    Chest,
    Abdomen,
    Pelvis,
    Arm(Side),
    Leg(Side),
}

/// Direction in which a hinge flexes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flexion {
    /// The child swings backwards (knee).
    Backward,
    /// The child swings forwards (elbow).
    Forward,
}

impl PuppetPart {
    pub const ALL: [PuppetPart; PART_COUNT] = [
        PuppetPart::Pelvis,
        PuppetPart::SpineLower,
        PuppetPart::SpineUpper,
        PuppetPart::Head,
        PuppetPart::UpperArmL,
        PuppetPart::LowerArmL,
        PuppetPart::HandL,
        PuppetPart::UpperArmR,
        PuppetPart::LowerArmR,
        PuppetPart::HandR,
        PuppetPart::ThighL,
        PuppetPart::CalfL,
        PuppetPart::FootL,
        PuppetPart::ThighR,
        PuppetPart::CalfR,
        PuppetPart::FootR,
    ];

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    #[inline]
    pub fn from_index(i: usize) -> Self {
        Self::ALL[i]
    }

    pub fn parent(self) -> Option<Self> {
        use PuppetPart::*;
        Some(match self {
            Pelvis => return None,
            SpineLower => Pelvis,
            SpineUpper => SpineLower,
            Head => SpineUpper,
            UpperArmL | UpperArmR => SpineUpper,
            LowerArmL => UpperArmL,
            HandL => LowerArmL,
            LowerArmR => UpperArmR,
            HandR => LowerArmR,
            ThighL | ThighR => Pelvis,
            CalfL => ThighL,
            FootL => CalfL,
            CalfR => ThighR,
            FootR => CalfR,
        })
    }

    /// The part whose bone marks the end of this part's segment.
    pub fn segment_child(self) -> Option<Self> {
        use PuppetPart::*;
        Some(match self {
            SpineLower => SpineUpper,
            SpineUpper => Head,
            UpperArmL => LowerArmL,
            LowerArmL => HandL,
            UpperArmR => LowerArmR,
            LowerArmR => HandR,
            ThighL => CalfL,
            CalfL => FootL,
            ThighR => CalfR,
            CalfR => FootR,
            _ => return None,
        })
    }

    pub fn limb(self) -> Limb {
        use PuppetPart::*;
        match self {
            Pelvis | SpineLower | SpineUpper | Head => Limb::Spine,
            UpperArmL | LowerArmL | HandL => Limb::LeftArm,
            UpperArmR | LowerArmR | HandR => Limb::RightArm,
            ThighL | CalfL | FootL => Limb::LeftLeg,
            ThighR | CalfR | FootR => Limb::RightLeg,
        }
    }

    pub fn side(self) -> Option<Side> {
        match self.limb() {
            Limb::LeftArm | Limb::LeftLeg => Some(Side::Left),
            Limb::RightArm | Limb::RightLeg => Some(Side::Right),
            Limb::Spine => None,
        }
    }

    pub fn foot(side: Side) -> Self {
        match side {
            Side::Left => PuppetPart::FootL,
            Side::Right => PuppetPart::FootR,
        }
    }

    pub fn hand(side: Side) -> Self {
        match side {
            Side::Left => PuppetPart::HandL,
            Side::Right => PuppetPart::HandR,
        }
    }

    pub fn thigh(side: Side) -> Self {
        match side {
            Side::Left => PuppetPart::ThighL,
            Side::Right => PuppetPart::ThighR,
        }
    }

    pub fn calf(side: Side) -> Self {
        match side {
            Side::Left => PuppetPart::CalfL,
            Side::Right => PuppetPart::CalfR,
        }
    }

    pub fn upper_arm(side: Side) -> Self {
        match side {
            Side::Left => PuppetPart::UpperArmL,
            Side::Right => PuppetPart::UpperArmR,
        }
    }

    pub fn lower_arm(side: Side) -> Self {
        match side {
            Side::Left => PuppetPart::LowerArmL,
            Side::Right => PuppetPart::LowerArmR,
        }
    }

    /// Whether `self` is `ancestor` or lies below it in the part tree.
    pub fn is_in_subtree_of(self, ancestor: Self) -> bool {
        let mut p = Some(self);
        while let Some(cur) = p {
            if cur == ancestor {
                return true;
            }
            p = cur.parent();
        }
        false
    }

    /// Segment mass fraction of the whole body (de Leva 1996, male; the
    /// trunk split in lower/middle/upper, the neck folded into the head).
    pub fn mass_fraction(self) -> f32 {
        use PuppetPart::*;
        match self {
            Pelvis => 0.1117,
            SpineLower => 0.1633,
            SpineUpper => 0.1596,
            Head => 0.0694,
            UpperArmL | UpperArmR => 0.0271,
            LowerArmL | LowerArmR => 0.0162,
            HandL | HandR => 0.0061,
            ThighL | ThighR => 0.1416,
            CalfL | CalfR => 0.0433,
            FootL | FootR => 0.0137,
        }
    }

    /// Capsule radius as a fraction of the standing height.
    pub fn radius_fraction(self) -> f32 {
        use PuppetPart::*;
        match self {
            Pelvis => 0.055,
            SpineLower => 0.060,
            SpineUpper => 0.058,
            Head => 0.055,
            UpperArmL | UpperArmR => 0.028,
            LowerArmL | LowerArmR => 0.024,
            HandL | HandR => 0.022,
            ThighL | ThighR => 0.040,
            CalfL | CalfR => 0.030,
            FootL | FootR => 0.025,
        }
    }

    pub fn shape(self) -> PartShape {
        use PuppetPart::*;
        match self {
            Pelvis | SpineUpper => PartShape::Across,
            FootL | FootR => PartShape::FootBox,
            _ => PartShape::Segment,
        }
    }

    /// Anatomical zone of this part (head, chest, abdomen, pelvis, arm, leg).
    pub fn zone(self) -> BodyZone {
        use PuppetPart::*;
        match self {
            Head => BodyZone::Head,
            SpineUpper => BodyZone::Chest,
            SpineLower => BodyZone::Abdomen,
            Pelvis => BodyZone::Pelvis,
            UpperArmL | LowerArmL | HandL => BodyZone::Arm(Side::Left),
            UpperArmR | LowerArmR | HandR => BodyZone::Arm(Side::Right),
            ThighL | CalfL | FootL => BodyZone::Leg(Side::Left),
            ThighR | CalfR | FootR => BodyZone::Leg(Side::Right),
        }
    }

    /// Joint between this part and its parent (`None` for the pelvis).
    pub fn joint_kind(self) -> Option<JointKind> {
        use PuppetPart::*;
        Some(match self {
            Pelvis => return None,
            // Lumbar spine: flexes forward (~40°), stops hyperextension (~20°).
            SpineLower => JointKind::Ball {
                swing: 0.52,
                twist: 0.32,
                bias_forward: 0.18,
                bias_outward: 0.0,
            },
            // Thoracic spine: greater axial rotation and forward crunch.
            SpineUpper => JointKind::Ball {
                swing: 0.56,
                twist: 0.50,
                bias_forward: 0.18,
                bias_outward: 0.0,
            },
            // Cervical spine: deep chin tuck to protect the skull, wide glance turn.
            Head => JointKind::Ball {
                swing: 0.80,
                twist: 1.15,
                bias_forward: 0.15,
                bias_outward: 0.0,
            },
            // Glenohumeral shoulder: biased forward & outward (full overhead/brace
            // reach without backward dislocation through the ribcage).
            UpperArmL | UpperArmR => JointKind::Ball {
                swing: 1.65,
                twist: 1.30,
                bias_forward: 0.42,
                bias_outward: 0.36,
            },
            // Elbow hinge: never hyperextends backwards.
            LowerArmL | LowerArmR => JointKind::Hinge {
                min: 0.0,
                max: 2.45,
                flexion: Flexion::Forward,
            },
            HandL | HandR => JointKind::Ball {
                swing: 1.05,
                twist: 0.65,
                bias_forward: 0.0,
                bias_outward: 0.0,
            },
            // Hip socket with anatomical iliofemoral ligament bias:
            // ~100° forward flexion (deep squat/lunge/tuck) vs ~51° backward
            // extension (allows full athletic backward stumble steps even when
            // the pelvis pitches rearward under a shove).
            ThighL | ThighR => JointKind::Ball {
                swing: 1.32,
                twist: 0.58,
                bias_forward: 0.42,
                bias_outward: 0.14,
            },
            // Knee hinge: tiny tolerance past straight, deep crouch flexion.
            CalfL | CalfR => JointKind::Hinge {
                min: -0.04,
                max: 2.40,
                flexion: Flexion::Backward,
            },
            // Ankle: wide enough cone (`0.85 rad`) so combined dorsiflexion/plantarflexion
            // + lateral roll in deep athletic crouches or wide recovery stances never binds.
            FootL | FootR => JointKind::Ball {
                swing: 0.85,
                twist: 0.35,
                bias_forward: 0.0,
                bias_outward: 0.0,
            },
        })
    }

    /// Relative axial-twist stiffness vs swing stiffness (anisotropic muscle tone).
    /// Upper-body joints (spine, neck, shoulders, wrists) yield more easily in
    /// axial twist than flexion for supple shoulder-slips and off-centre hit
    /// torsion; legs keep 1.0 so the revolute knee hinge stays twist-locked.
    pub fn twist_stiffness_scale(self) -> f32 {
        use PuppetPart::*;
        match self {
            Pelvis => 1.0,
            SpineLower | SpineUpper => 0.72,
            Head => 0.75,
            UpperArmL | UpperArmR => 0.68,
            LowerArmL | LowerArmR | CalfL | CalfR => 1.0, // hinges
            HandL | HandR => 0.65,
            ThighL | ThighR | FootL | FootR => 1.0, // twist-locked for knee hinge alignment
        }
    }

    /// Passive viscoelastic soft-tissue / tendon damping coefficient (relative scale).
    /// Remains active even when muscles are limp (`Dead`/`Relax`) so limbs settle
    /// with biological weight rather than rubbery pendulum oscillation.
    pub fn passive_damping_coeff(self) -> f32 {
        use PuppetPart::*;
        match self {
            Pelvis => 0.0,
            SpineLower | SpineUpper => 1.4,
            Head => 0.28,
            UpperArmL | UpperArmR => 0.22,
            LowerArmL | LowerArmR => 0.14,
            HandL | HandR => 0.04,
            ThighL | ThighR => 1.1,
            CalfL | CalfR => 0.75,
            FootL | FootR => 0.28,
        }
    }

    /// Muscle stiffness of the joint as a fraction of `M·g·H` (N·m/rad).
    /// Legs and trunk must hold the body weight; arms and head only themselves.
    pub fn stiffness_coeff(self) -> f32 {
        use PuppetPart::*;
        match self {
            Pelvis => 0.0,
            SpineLower | SpineUpper => 1.2,
            Head => 0.12,
            UpperArmL | UpperArmR => 0.10,
            LowerArmL | LowerArmR => 0.06,
            HandL | HandR => 0.015,
            ThighL | ThighR => 1.4,
            CalfL | CalfR => 1.4,
            FootL | FootR => 1.1,
        }
    }

    /// Torque ceiling of the joint as a fraction of `M·g·H`.
    pub fn max_torque_coeff(self) -> f32 {
        use PuppetPart::*;
        match self {
            Pelvis => 0.0,
            SpineLower | SpineUpper => 0.5,
            Head => 0.06,
            UpperArmL | UpperArmR => 0.12,
            LowerArmL | LowerArmR => 0.08,
            HandL | HandR => 0.02,
            ThighL | ThighR => 0.55,
            CalfL | CalfR => 0.55,
            FootL | FootR => 0.35,
        }
    }

    /// Accepted bone names, compared after [`normalize_bone_name`]. UE-style
    /// (the AiGameKit pipeline) first, then Mixamo, then common variants.
    pub fn bone_names(self) -> &'static [&'static str] {
        use PuppetPart::*;
        match self {
            Pelvis => &["pelvis", "hips", "hip", "root_pelvis"],
            SpineLower => &["spine_01", "spine01", "spine", "spine1_lower"],
            SpineUpper => &["spine_03", "spine03", "spine2", "chest", "upperchest", "spine_02", "spine1"],
            Head => &["neck_01", "neck01", "neck", "head"],
            UpperArmL => &["upperarm_l", "leftarm", "l_upperarm", "upperarm.l", "arm_l", "leftupperarm"],
            LowerArmL => &["lowerarm_l", "leftforearm", "l_forearm", "forearm.l", "forearm_l", "leftlowerarm"],
            HandL => &["hand_l", "lefthand", "l_hand", "hand.l"],
            UpperArmR => &["upperarm_r", "rightarm", "r_upperarm", "upperarm.r", "arm_r", "rightupperarm"],
            LowerArmR => &["lowerarm_r", "rightforearm", "r_forearm", "forearm.r", "forearm_r", "rightlowerarm"],
            HandR => &["hand_r", "righthand", "r_hand", "hand.r"],
            ThighL => &["thigh_l", "leftupleg", "l_thigh", "thigh.l", "upperleg_l", "leftupperleg"],
            CalfL => &["calf_l", "leftleg", "l_calf", "shin.l", "shin_l", "lowerleg_l", "leftlowerleg"],
            FootL => &["foot_l", "leftfoot", "l_foot", "foot.l"],
            ThighR => &["thigh_r", "rightupleg", "r_thigh", "thigh.r", "upperleg_r", "rightupperleg"],
            CalfR => &["calf_r", "rightleg", "r_calf", "shin.r", "shin_r", "lowerleg_r", "rightlowerleg"],
            FootR => &["foot_r", "rightfoot", "r_foot", "foot.r"],
        }
    }
}

/// Lower-case, strip the Mixamo/Blender armature prefixes.
pub fn normalize_bone_name(name: &str) -> String {
    let mut s = name.trim().to_ascii_lowercase();
    for prefix in ["mixamorig:", "mixamorig_", "mixamorig", "armature|", "armature/", "def-"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }
    s
}

/// Rank of `name` in the part's accepted names (lower is better).
pub fn bone_rank(part: PuppetPart, name: &str) -> Option<usize> {
    let n = normalize_bone_name(name);
    part.bone_names().iter().position(|c| *c == n)
}

/// Head end of the skull, for sizing the head capsule (bone names only).
pub fn is_head_bone(name: &str) -> bool {
    normalize_bone_name(name) == "head"
}

/// Toe/ball bones mark the front of the foot.
pub fn is_toe_bone(name: &str) -> bool {
    let n = normalize_bone_name(name);
    n.starts_with("ball") || n.contains("toe")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_are_ordered_parents_first() {
        for (i, p) in PuppetPart::ALL.iter().enumerate() {
            assert_eq!(p.index(), i);
            if let Some(parent) = p.parent() {
                assert!(parent.index() < i, "{p:?} before its parent");
            }
        }
    }

    #[test]
    fn mass_fractions_sum_to_one() {
        let s: f32 = PuppetPart::ALL.iter().map(|p| p.mass_fraction()).sum();
        assert!((s - 1.0).abs() < 1e-3, "{s}");
    }

    #[test]
    fn ue_and_mixamo_names_resolve() {
        for (name, part) in [
            ("pelvis", PuppetPart::Pelvis),
            ("mixamorig:Hips", PuppetPart::Pelvis),
            ("spine_01", PuppetPart::SpineLower),
            ("mixamorig:Spine2", PuppetPart::SpineUpper),
            ("neck_01", PuppetPart::Head),
            ("upperarm_l", PuppetPart::UpperArmL),
            ("mixamorig:RightForeArm", PuppetPart::LowerArmR),
            ("thigh_r", PuppetPart::ThighR),
            ("mixamorig:LeftLeg", PuppetPart::CalfL),
            ("foot_l", PuppetPart::FootL),
        ] {
            assert!(bone_rank(part, name).is_some(), "{name}");
        }
        assert!(bone_rank(PuppetPart::Pelvis, "Mesh").is_none());
        assert!(is_head_bone("Head"));
        assert!(is_toe_bone("ball_l"));
        assert!(is_toe_bone("mixamorig:LeftToeBase"));
    }

    #[test]
    fn subtree_membership() {
        assert!(PuppetPart::FootL.is_in_subtree_of(PuppetPart::ThighL));
        assert!(PuppetPart::FootL.is_in_subtree_of(PuppetPart::Pelvis));
        assert!(!PuppetPart::FootL.is_in_subtree_of(PuppetPart::ThighR));
        assert!(!PuppetPart::Pelvis.is_in_subtree_of(PuppetPart::SpineLower));
    }
}
