//! `MaterialSpec` → `bevy_gradmesh::Style`, including the look presets.

use bevy::prelude::*;
use bevy_ext_scene::primitives::{MaterialSpec, Preset};
use bevy_gradmesh::Style;

/// Builds the `bevy_gradmesh` style for a primitive.
///
/// Colour, opacity, roughness, metallic and emissive are preserved. The
/// sampled maps become procedural [`Preset`] patterns:
///
/// * wood → planks (stripes along local X) + grain;
/// * stone → coarse noise;
/// * cobblestone → finer, stronger noise;
/// * metal → very fine low-contrast noise;
/// * cloth → loose vertical stripes.
pub fn style(material: &MaterialSpec) -> Style {
    let [red, green, blue] = material.base_color.unwrap_or([1.0, 1.0, 1.0]);
    let color = Color::srgb(red, green, blue);
    let mut style = match material.opacity {
        Some(alpha) if alpha < 1.0 => Style::transparent(color).with_alpha(alpha.clamp(0.0, 1.0)),
        _ => Style::opaque(color),
    };
    if let Some(roughness) = material.roughness {
        style = style.with_perceptual_roughness(roughness.clamp(0.0, 1.0));
    }
    if let Some(metallic) = material.metallic {
        style = style.with_metallic(metallic.clamp(0.0, 1.0));
    }
    if let Some([red, green, blue]) = material.emissive {
        style = style.with_emissive(LinearRgba::rgb(red, green, blue));
    }
    apply_preset(&mut style, material);
    style
}

fn apply_preset(style: &mut Style, material: &MaterialSpec) {
    let preset = material.preset.unwrap_or_default().resolve(material);
    match preset {
        Preset::Wood => {
            // Planks run along the local X of a beam/board.
            *style = style.clone().with_stripes(Vec3::X, 1.2, 0.10);
            *style = style.clone().with_noise(2.0, 0.06);
        }
        Preset::Stone => {
            *style = style.clone().with_noise(1.1, 0.10);
        }
        Preset::Cobblestone => {
            *style = style.clone().with_noise(2.6, 0.14);
        }
        Preset::Metal => {
            *style = style.clone().with_noise(6.0, 0.04);
        }
        Preset::Cloth => {
            *style = style.clone().with_stripes(Vec3::Y, 3.0, 0.06);
        }
        Preset::Flat => {}
        Preset::Auto => {
            // Unknown map semantics: one noise cell per authored tile.
            let tile = material
                .texture_tile
                .filter(|tile| *tile > 0.0)
                .unwrap_or(1.0);
            *style = style.clone().with_noise(1.0 / tile, 0.10);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_gradmesh::BlendMode;

    #[test]
    fn material_fields_are_preserved() {
        let material = MaterialSpec {
            base_color: Some([0.5, 0.25, 0.1]),
            metallic: Some(0.6),
            roughness: Some(0.35),
            opacity: Some(0.25),
            emissive: Some([1.0, 0.5, 0.0]),
            ..Default::default()
        };
        let style = style(&material);
        let color = style.color.to_srgba();
        assert!((color.red - 0.5).abs() < 1e-6);
        assert!((color.green - 0.25).abs() < 1e-6);
        assert!((color.blue - 0.1).abs() < 1e-6);
        assert!((color.alpha - 0.25).abs() < 1e-6);
        assert_eq!(style.blend_mode, BlendMode::Transparent);
        assert_eq!(style.perceptual_roughness, Some(0.35));
        assert_eq!(style.metallic, 0.6);
        assert_eq!(style.emissive, Some(LinearRgba::rgb(1.0, 0.5, 0.0)));
    }

    #[test]
    fn maps_become_presets() {
        let wood = MaterialSpec {
            texture: Some("/assets/textures/wood_planks/albedo.ktx2".into()),
            ..Default::default()
        };
        let wood_style = style(&wood);
        assert!(wood_style.stripes.is_some(), "wood → planks");
        assert!(wood_style.noise.is_some(), "wood → grain");

        let stone = MaterialSpec {
            texture: Some("/assets/textures/mountain_stone/albedo.ktx2".into()),
            ..Default::default()
        };
        let stone_style = style(&stone);
        assert!(stone_style.noise.is_some());
        assert!(stone_style.stripes.is_none(), "stone is not striped");
    }

    #[test]
    fn explicit_preset_wins_over_the_url() {
        let material = MaterialSpec {
            texture: Some("/assets/textures/mountain_stone/albedo.ktx2".into()),
            preset: Some(Preset::Wood),
            ..Default::default()
        };
        assert!(style(&material).stripes.is_some());
    }

    #[test]
    fn flat_preset_leaves_the_surface_plain() {
        let material = MaterialSpec {
            preset: Some(Preset::Flat),
            ..Default::default()
        };
        let style = style(&material);
        assert!(style.noise.is_none());
        assert!(style.stripes.is_none());
    }
}
