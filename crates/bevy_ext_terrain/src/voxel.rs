//! Bevy scene components for voxel terrain.
//!
//! One `VoxelTerrain` entity owns the heightfield and the extent; each
//! authored feature is a child entity with its own component, so the editor
//! can select it, gizmo it and undo it independently. The components are
//! `Reflect` and use stable type paths (`bevy_ext_terrain::voxel`) because
//! those strings are part of the `.bsn` wire format.

use bevy::prelude::*;
use bevy_voxel_terrain::arch::ArchProfile;
use bevy_voxel_terrain::cave::{CaveChamberSpec, CaveShaftSpec};
use bevy_voxel_terrain::{
    ArchSpec, BridgeSpec, BridgeStyle, CaveSpec, HeightField, VoxelField, VoxelMod,
};

fn v2(p: [f32; 2]) -> Vec2 {
    Vec2::new(p[0], p[1])
}

fn from_v2(p: Vec2) -> [f32; 2] {
    [p.x, p.y]
}

/// The voxel terrain entity: a heightfield plus the 3D mods authored as
/// children.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub struct VoxelTerrain {
    /// `.ahgt` path relative to the project root; empty means the component
    /// only carries the authored features (the editor still needs a grid).
    pub heightmap: String,
    /// World span on XZ (meters).
    pub world_size: f32,
    /// Height of a fully-white `.ahgt` sample (meters).
    pub max_height: f32,
    /// Heightfield sample spacing used by the base term (meters).
    pub cell_size: f32,
    /// Transvoxel cell size (meters). Usually 1.0.
    pub voxel_size: f32,
    /// Optional material path/name for the preview and the runtime.
    pub material: String,
}

impl VoxelTerrain {
    /// Builds a [`VoxelField`] over `base` with the given feature children.
    pub fn build_field(
        &self,
        base: &dyn HeightField,
        features: &[&dyn VoxelFeature],
    ) -> VoxelField {
        let mut mods: Vec<Box<dyn VoxelMod>> = Vec::new();
        for feature in features {
            mods.extend(feature.build(base));
        }
        VoxelField::new(mods, self.world_size.max(1.0), self.cell_size.max(0.01))
    }
}

/// A component that contributes CSG mods to the voxel field.
pub trait VoxelFeature: Send + Sync {
    /// Human-readable kind for diagnostics/the editor.
    fn feature_name(&self) -> &'static str;
    /// Builds the mods against the already-sampled heightfield.
    fn build(&self, base: &dyn HeightField) -> Vec<Box<dyn VoxelMod>>;
}

/// Builds the field from a terrain component and already-collected features.
pub fn build_voxel_field(
    terrain: &VoxelTerrain,
    base: &dyn HeightField,
    features: &[&dyn VoxelFeature],
) -> VoxelField {
    terrain.build_field(base, features)
}

/// A room inside a cave (`<Chamber>`).
#[derive(Reflect, Default, Clone, Debug)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub struct VoxelChamber {
    pub name: String,
    /// World XZ of the room's centre.
    pub at: [f32; 2],
    pub radius: f32,
    pub height: f32,
    /// `None` inherits the cave's depth.
    pub depth: Option<f32>,
}

impl VoxelChamber {
    fn to_spec(&self) -> CaveChamberSpec {
        CaveChamberSpec {
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            at: v2(self.at),
            radius: self.radius,
            height: self.height,
            depth: self.depth,
        }
    }
}

/// A chimney to the surface (`<Shaft>`).
#[derive(Reflect, Default, Clone, Debug)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub struct VoxelShaft {
    pub name: String,
    pub at: [f32; 2],
    pub radius: f32,
    pub depth: Option<f32>,
}

impl VoxelShaft {
    fn to_spec(&self) -> CaveShaftSpec {
        CaveShaftSpec {
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            at: v2(self.at),
            radius: self.radius,
            depth: self.depth,
        }
    }
}

/// A subtractive cave tube (`<Cave>`).
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub struct VoxelCave {
    pub name: String,
    /// Tunnel centreline in world XZ.
    pub path: Vec<[f32; 2]>,
    /// Tube radius profile; empty means the core default.
    pub radius: Vec<f32>,
    pub depth: f32,
    pub open_ends: bool,
    pub mouth_flare: f32,
    pub mouth_fraction: f32,
    pub chambers: Vec<VoxelChamber>,
    pub shafts: Vec<VoxelShaft>,
    pub mound_height: f32,
    pub mound_radius: f32,
    pub mouth_lift: bool,
}

impl Default for VoxelCave {
    fn default() -> Self {
        let spec = CaveSpec::default();
        Self {
            name: String::new(),
            path: Vec::new(),
            radius: spec.radius,
            depth: spec.depth,
            open_ends: spec.open_ends,
            mouth_flare: spec.mouth_flare,
            mouth_fraction: spec.mouth_fraction,
            chambers: Vec::new(),
            shafts: Vec::new(),
            mound_height: spec.mound_height,
            mound_radius: spec.mound_radius,
            mouth_lift: spec.mouth_lift,
        }
    }
}

impl VoxelCave {
    pub fn to_spec(&self) -> CaveSpec {
        CaveSpec {
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            path: self.path.iter().copied().map(v2).collect(),
            radius: if self.radius.is_empty() {
                CaveSpec::default().radius
            } else {
                self.radius.clone()
            },
            depth: self.depth,
            open_ends: self.open_ends,
            mouth_flare: self.mouth_flare,
            mouth_fraction: self.mouth_fraction,
            chambers: self.chambers.iter().map(VoxelChamber::to_spec).collect(),
            shafts: self.shafts.iter().map(VoxelShaft::to_spec).collect(),
            mound_height: self.mound_height,
            mound_radius: self.mound_radius,
            mouth_lift: self.mouth_lift,
        }
    }
}

impl VoxelFeature for VoxelCave {
    fn feature_name(&self) -> &'static str {
        "cave"
    }

    fn build(&self, base: &dyn HeightField) -> Vec<Box<dyn VoxelMod>> {
        self.to_spec().build(base)
    }
}

/// The arch profile, mirroring [`ArchProfile`].
#[derive(Reflect, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub enum VoxelArchProfile {
    #[default]
    Portal,
    Natural,
}

impl From<VoxelArchProfile> for ArchProfile {
    fn from(profile: VoxelArchProfile) -> Self {
        match profile {
            VoxelArchProfile::Portal => ArchProfile::Portal,
            VoxelArchProfile::Natural => ArchProfile::Natural,
        }
    }
}

/// A structural arch (`<Arch>`), or a viaduct when `spans > 1`.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub struct VoxelArch {
    pub name: String,
    pub at: [f32; 2],
    pub path: Vec<[f32; 2]>,
    pub span: Option<f32>,
    pub height: f32,
    pub thickness: f32,
    pub depth: f32,
    pub yaw: f32,
    pub spans: u32,
    pub profile: VoxelArchProfile,
}

impl Default for VoxelArch {
    fn default() -> Self {
        let spec = ArchSpec::default();
        Self {
            name: String::new(),
            at: from_v2(spec.at),
            path: Vec::new(),
            span: spec.span,
            height: spec.height,
            thickness: spec.thickness,
            depth: spec.depth,
            yaw: spec.yaw,
            spans: spec.spans,
            profile: VoxelArchProfile::Portal,
        }
    }
}

impl VoxelArch {
    pub fn to_spec(&self) -> ArchSpec {
        ArchSpec {
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            at: v2(self.at),
            path: self.path.iter().copied().map(v2).collect(),
            span: self.span,
            height: self.height,
            thickness: self.thickness,
            depth: self.depth,
            yaw: self.yaw,
            spans: self.spans.max(1),
            profile: self.profile.into(),
        }
    }
}

impl VoxelFeature for VoxelArch {
    fn feature_name(&self) -> &'static str {
        "arch"
    }

    fn build(&self, base: &dyn HeightField) -> Vec<Box<dyn VoxelMod>> {
        self.to_spec().build(base)
    }
}

/// The bridge style, mirroring [`BridgeStyle`].
#[derive(Reflect, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub enum VoxelBridgeStyle {
    #[default]
    Stone,
    Natural,
}

impl From<VoxelBridgeStyle> for BridgeStyle {
    fn from(style: VoxelBridgeStyle) -> Self {
        match style {
            VoxelBridgeStyle::Stone => BridgeStyle::Stone,
            VoxelBridgeStyle::Natural => BridgeStyle::Natural,
        }
    }
}

/// A deck across a valley or river (`<Bridge>`).
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_terrain::voxel"]
pub struct VoxelBridge {
    pub name: String,
    /// Deck centreline in world XZ.
    pub path: Vec<[f32; 2]>,
    pub width: f32,
    pub rise: f32,
    pub thickness: f32,
    pub style: VoxelBridgeStyle,
    pub spans: Option<u32>,
    pub pier_width: f32,
    pub parapet: f32,
    pub clearance: f32,
}

impl Default for VoxelBridge {
    fn default() -> Self {
        let spec = BridgeSpec::default();
        Self {
            name: String::new(),
            path: Vec::new(),
            width: spec.width,
            rise: spec.rise,
            thickness: spec.thickness,
            style: VoxelBridgeStyle::Stone,
            spans: spec.spans,
            pier_width: spec.pier_width,
            parapet: spec.parapet,
            clearance: spec.clearance,
        }
    }
}

impl VoxelBridge {
    pub fn to_spec(&self) -> BridgeSpec {
        BridgeSpec {
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            path: self.path.iter().copied().map(v2).collect(),
            width: self.width,
            rise: self.rise,
            thickness: self.thickness,
            style: self.style.into(),
            spans: self.spans,
            pier_width: self.pier_width,
            parapet: self.parapet,
            clearance: self.clearance,
        }
    }
}

impl VoxelFeature for VoxelBridge {
    fn feature_name(&self) -> &'static str {
        "bridge"
    }

    fn build(&self, base: &dyn HeightField) -> Vec<Box<dyn VoxelMod>> {
        self.to_spec().build(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Flat(f32);

    impl HeightField for Flat {
        fn sample(&self, _world_x: f32, _world_z: f32) -> f32 {
            self.0
        }

        fn sample_normal(&self, _world_x: f32, _world_z: f32, _epsilon: f32) -> Vec3 {
            Vec3::Y
        }

        fn max_height(&self) -> f32 {
            self.0.max(1.0)
        }
    }

    #[test]
    fn cave_component_roundtrips_to_core_spec() {
        let cave = VoxelCave {
            name: "gruta".into(),
            path: vec![[0.0, 0.0], [20.0, 4.0]],
            radius: vec![2.5, 4.0],
            depth: 9.0,
            open_ends: true,
            mouth_flare: 1.4,
            mouth_fraction: 0.2,
            chambers: vec![VoxelChamber {
                name: "sala".into(),
                at: [10.0, 2.0],
                radius: 6.0,
                height: 5.0,
                depth: Some(12.0),
            }],
            shafts: vec![VoxelShaft {
                name: "chaminé".into(),
                at: [5.0, 0.0],
                radius: 1.5,
                depth: None,
            }],
            mound_height: 6.0,
            mound_radius: 18.0,
            mouth_lift: true,
        };
        let spec = cave.to_spec();
        assert_eq!(spec.name.as_deref(), Some("gruta"));
        assert_eq!(spec.path[1], Vec2::new(20.0, 4.0));
        assert_eq!(spec.radius, vec![2.5, 4.0]);
        assert_eq!(spec.chambers[0].radius, 6.0);
        assert_eq!(spec.shafts[0].name.as_deref(), Some("chaminé"));
        assert!(spec.mouth_lift);
    }

    #[test]
    fn field_has_a_cave_mod() {
        let terrain = VoxelTerrain {
            world_size: 128.0,
            max_height: 40.0,
            cell_size: 1.0,
            voxel_size: 1.0,
            ..Default::default()
        };
        let cave = VoxelCave {
            path: vec![[-20.0, 0.0], [20.0, 0.0]],
            depth: 10.0,
            open_ends: false,
            ..Default::default()
        };
        let base = Flat(0.0);
        let field = build_voxel_field(&terrain, &base, &[&cave]);
        assert!(!field.is_flat(), "a cave has to create at least one mod");
        assert!(!field.mods().is_empty());
    }
}
