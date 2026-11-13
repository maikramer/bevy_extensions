//! Material descriptor and the procedural look presets.
//!
//! The gradient material cannot sample albedo/normal maps, so the world's
//! texture URLs are used as a *semantic hint*: wood gets planks and grain,
//! stone coarse noise, cobblestone a finer one. Authors can also name the
//! preset explicitly (`preset="wood"`). `bevy_ext_gradmesh` turns this into a
//! `bevy_gradmesh::Style`; this module stays render-free.

use std::hash::{DefaultHasher, Hash, Hasher};

/// Hand-picked surface treatment used in place of the sampled maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Preset {
    /// Derive from the texture URL, or [`Preset::Flat`] when there is none.
    #[default]
    Auto,
    /// Planks along local X plus grain.
    Wood,
    /// Coarse rock noise.
    Stone,
    /// Finer, higher-contrast noise (cobbles).
    Cobblestone,
    /// Very fine, low-contrast noise.
    Metal,
    /// Loose vertical weave/stripes.
    Cloth,
    /// No procedural pattern (flat gradient-material colour).
    Flat,
}

impl Preset {
    /// Parses the `preset`/`style` attribute. Unknown names fall back to
    /// `Auto` (the caller can warn).
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "auto" => Preset::Auto,
            "wood" | "planks" | "madeira" => Preset::Wood,
            "stone" | "rock" | "pedra" | "mountain_stone" => Preset::Stone,
            "cobble" | "cobblestone" | "road" | "paving" | "calçada" => Preset::Cobblestone,
            "metal" | "iron" | "steel" | "ferro" => Preset::Metal,
            "cloth" | "fabric" | "banner" | "tecido" => Preset::Cloth,
            "flat" | "none" | "plain" => Preset::Flat,
            _ => return None,
        })
    }

    /// Resolves [`Preset::Auto`] against the sampled-map URL.
    pub fn resolve(self, material: &MaterialSpec) -> Self {
        if self != Preset::Auto {
            return self;
        }
        let Some(url) = material
            .texture
            .as_deref()
            .or(material.normal_map.as_deref())
        else {
            return Preset::Flat;
        };
        let url = url.to_ascii_lowercase();
        if url.contains("wood") || url.contains("plank") || url.contains("timber") {
            Preset::Wood
        } else if url.contains("cobble") || url.contains("road") || url.contains("paving") {
            Preset::Cobblestone
        } else if url.contains("stone") || url.contains("rock") || url.contains("cliff") {
            Preset::Stone
        } else if url.contains("metal") || url.contains("iron") || url.contains("steel") {
            Preset::Metal
        } else if url.contains("cloth") || url.contains("fabric") || url.contains("banner") {
            Preset::Cloth
        } else if material.texture.is_some() || material.normal_map.is_some() {
            // A map exists but we do not know its semantics: keep the old
            // subtle grain from the tile size.
            Preset::Auto
        } else {
            Preset::Flat
        }
    }
}

/// Material fields preserved from the authored DSL.
///
/// This mirrors the fields `viber::recipes::MaterialSpec` had before the
/// migration and adds [`MaterialSpec::preset`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialSpec {
    pub base_color: Option<[f32; 3]>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    /// 0..=1; below 1 switches to the transparent blend mode.
    pub opacity: Option<f32>,
    pub emissive: Option<[f32; 3]>,
    /// Albedo map. Not sampled by the gradient material: used as a preset
    /// hint and reported as dropped by the game spawn.
    pub texture: Option<String>,
    /// Metres per texture repeat (`texture-tile-size`).
    pub texture_tile: Option<f32>,
    /// Normal map. Same contract as [`MaterialSpec::texture`].
    pub normal_map: Option<String>,
    /// Optional surface preset; `None` derives it from the map URL.
    pub preset: Option<Preset>,
}

/// Stable-enough fingerprint for per-material caches (the editor preview and
/// the game need the same dedup key without hashing floats directly).
pub fn material_fingerprint(material: &MaterialSpec) -> u64 {
    let mut hasher = DefaultHasher::new();
    format!("{material:?}").hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_become_presets() {
        let wood = MaterialSpec {
            texture: Some("/assets/textures/wood_planks/albedo.ktx2".into()),
            ..Default::default()
        };
        assert_eq!(wood.preset.unwrap_or_default().resolve(&wood), Preset::Wood);

        let stone = MaterialSpec {
            texture: Some("/assets/textures/mountain_stone/albedo.ktx2".into()),
            ..Default::default()
        };
        assert_eq!(
            stone.preset.unwrap_or_default().resolve(&stone),
            Preset::Stone
        );
    }

    #[test]
    fn explicit_preset_wins_over_the_url() {
        let material = MaterialSpec {
            texture: Some("/assets/textures/mountain_stone/albedo.ktx2".into()),
            preset: Some(Preset::Wood),
            ..Default::default()
        };
        assert_eq!(
            material.preset.unwrap_or_default().resolve(&material),
            Preset::Wood
        );
    }

    #[test]
    fn fingerprint_changes_with_the_look() {
        let a = MaterialSpec {
            base_color: Some([0.5, 0.25, 0.1]),
            ..Default::default()
        };
        let b = MaterialSpec {
            base_color: Some([0.5, 0.25, 0.1]),
            ..Default::default()
        };
        let c = MaterialSpec {
            base_color: Some([0.9, 0.9, 0.9]),
            ..Default::default()
        };
        assert_eq!(material_fingerprint(&a), material_fingerprint(&b));
        assert_ne!(material_fingerprint(&a), material_fingerprint(&c));
    }
}
