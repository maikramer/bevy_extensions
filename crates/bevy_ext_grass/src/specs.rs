//! Contrato autorado no `.bsn` e editável no Jackdaw.
//!
//! Estes componentes são a única parte da relva que o jogo autora:
//! [`GrassSettingsSpec`] (ligar/desligar, densidade, qualidade, sombras) e uma
//! lista de [`GrassStyleSpec`] (uma paleta por bioma, com plantas extra). O
//! plugin sincroniza-os para o [`GrassStyle`] do `bevy_grass` e para o
//! [`crate::GrassSettings`] de runtime; sem eles usa os defaults.
//!
//! Os `TypePath` são congelados em `bevy_ext_grass` para o editor, os saves e
//! o BRP continuarem a ver os mesmos tipos.

use bevy::color::Color;
use bevy::math::Vec2;
use bevy::prelude::*;
use bevy_grass::{
    GrassBlade, GrassQuality, GrassShadows, GrassSpecies, GrassStyle, GrassVariation, GroundTint,
};

/// Qualidade do `bevy_grass` autorada (mapeia para `GrassQuality`).
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default, Debug, Clone, PartialEq)]
#[type_path = "bevy_ext_grass"]
pub enum GrassQualitySpec {
    Low,
    #[default]
    Medium,
    High,
    Ultra,
}

impl From<GrassQualitySpec> for GrassQuality {
    fn from(value: GrassQualitySpec) -> Self {
        match value {
            GrassQualitySpec::Low => Self::Low,
            GrassQualitySpec::Medium => Self::Medium,
            GrassQualitySpec::High => Self::High,
            GrassQualitySpec::Ultra => Self::Ultra,
        }
    }
}

/// Settings globais da relva (um por cena/mundo, em `world/settings/grass.bsn`).
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone, PartialEq)]
#[type_path = "bevy_ext_grass"]
pub struct GrassSettingsSpec {
    pub enabled: bool,
    /// Multiplica a densidade do tier de qualidade.
    pub density_scale: f32,
    pub quality: GrassQualitySpec,
    /// Projeta sombra do sol (`Near`/`Off` do `bevy_grass`).
    pub shadows: bool,
    /// Multiplicador global do vento (as paletas têm o seu por bioma).
    pub wind_scale: f32,
}

impl Default for GrassSettingsSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            density_scale: 1.0,
            quality: GrassQualitySpec::Medium,
            shadows: true,
            wind_scale: 1.0,
        }
    }
}

impl GrassSettingsSpec {
    /// `GrassShadows` correspondente ao booleano autorado.
    #[must_use]
    pub fn shadows_mode(&self) -> GrassShadows {
        if self.shadows {
            GrassShadows::Near
        } else {
            GrassShadows::Off
        }
    }
}

/// Uma planta extra dentro de uma paleta (flor, trevo, espiga…).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default, Debug, Clone, PartialEq)]
#[type_path = "bevy_ext_grass"]
pub struct GrassPlantSpec {
    pub height: Vec2,
    pub width: f32,
    pub lean: f32,
    pub droop: f32,
    pub root_color: Color,
    pub tip_color: Color,
    /// Fração dos talos do campo que pertencem a esta planta (0..1).
    pub share: f32,
    /// Cresce em manchas em vez de espalhada (0..1).
    pub clustering: f32,
    pub head_length: f32,
    pub head_width: f32,
    pub head_color: Color,
}

impl Default for GrassPlantSpec {
    fn default() -> Self {
        Self {
            height: Vec2::new(0.32, 0.44),
            width: 0.03,
            lean: 0.15,
            droop: 0.0,
            root_color: Color::linear_rgb(0.30, 0.42, 0.16),
            tip_color: Color::linear_rgb(0.86, 0.84, 0.70),
            share: 0.03,
            clustering: 0.0,
            head_length: 0.0,
            head_width: 0.03,
            head_color: Color::linear_rgb(0.88, 0.84, 0.62),
        }
    }
}

/// Paleta de um bioma: o que o `bevy_grass` desenha e onde.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone, PartialEq)]
#[type_path = "bevy_ext_grass"]
pub struct GrassStyleSpec {
    /// Id do bioma (o `GrassFieldLayout`/`GrassTerrain` usa o mesmo).
    pub biome: u16,
    /// Talos por m² a densidade 100%.
    pub density: f32,
    /// Altura mais curta e mais alta (m).
    pub height: Vec2,
    /// Largura do talo à raiz (m).
    pub width: f32,
    /// Inclinação em repouso, fração da altura.
    pub lean: f32,
    /// Arco do talo, 0..1.
    pub droop: f32,
    pub root_color: Color,
    pub tip_color: Color,
    /// Cor seca (manchas, pontas secas).
    pub dry_color: Color,
    pub translucency: f32,
    pub specular: f32,
    /// Cor do chão que se vê na base dos talos.
    pub ground_color: Color,
    /// Quanto a base dos talos se funde com o chão (0..1).
    pub ground_amount: f32,
    /// Quanto o vento move este campo (1 = normal, >1 = crista exposta).
    pub wind_scale: f32,
    pub species: Vec<GrassPlantSpec>,
}

impl Default for GrassStyleSpec {
    fn default() -> Self {
        Self {
            biome: 0,
            density: 18.0,
            height: Vec2::new(0.33, 0.62),
            width: 0.056,
            lean: 0.15,
            droop: 0.22,
            root_color: Color::linear_rgb(0.045, 0.082, 0.032),
            tip_color: Color::linear_rgb(0.235, 0.310, 0.118),
            dry_color: Color::linear_rgb(0.370, 0.320, 0.140),
            translucency: 0.55,
            specular: 0.25,
            ground_color: Color::linear_rgb(0.12, 0.11, 0.05),
            ground_amount: 0.30,
            wind_scale: 1.0,
            species: Vec::new(),
        }
    }
}

/// Converte uma paleta autorada no `GrassStyle` que o renderer consome.
///
/// A variação (manchas, pontas secas, tufos, chão à mostra) mantém os valores
/// do look de referência; só o que é arte por bioma vem do spec.
#[must_use]
pub fn style_from_spec(spec: &GrassStyleSpec) -> GrassStyle {
    let base = GrassStyle::MEADOW;
    let blade = GrassBlade {
        height: spec.height,
        width: spec.width,
        lean: spec.lean,
        stiffness: 0.35,
        root_color: spec.root_color,
        tip_color: spec.tip_color,
        root_occlusion: 0.55,
        translucency: spec.translucency,
        specular: spec.specular,
        droop: spec.droop,
        ..base.blade
    };
    let species: Vec<GrassSpecies> = spec
        .species
        .iter()
        .map(|plant| GrassSpecies {
            share: plant.share,
            clustering: plant.clustering,
            blade: GrassBlade {
                height: plant.height,
                width: plant.width,
                lean: plant.lean,
                stiffness: 0.35,
                root_color: plant.root_color,
                tip_color: plant.tip_color,
                root_occlusion: 0.40,
                translucency: 0.55,
                specular: 0.20,
                droop: plant.droop,
                head_length: plant.head_length,
                head_width: plant.head_width,
                head_color: plant.head_color,
                ..base.blade
            },
        })
        .collect();
    GrassStyle {
        blade,
        density: spec.density.max(0.0),
        variation: GrassVariation {
            patch_color: spec.dry_color,
            patch_amount: 0.25,
            patch_size: 16.0,
            dry_tips: 0.26,
            dead_share: 0.04,
            tufts: 0.40,
            tuft_spacing: 0.55,
            bare_share: 0.06,
            bare_patch_size: 2.0,
            ..base.variation
        },
        ground_tint: GroundTint {
            color: spec.ground_color,
            amount: spec.ground_amount,
        },
        species: species.into(),
        ..base
    }
}

/// Regista os tipos autoráveis. Adicionar no jogo e na extensão do Jackdaw.
pub struct GrassSettingsSpecPlugin;

impl Plugin for GrassSettingsSpecPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<GrassQualitySpec>()
            .register_type::<GrassSettingsSpec>()
            .register_type::<GrassPlantSpec>()
            .register_type::<GrassStyleSpec>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_maps_to_upstream() {
        assert_eq!(GrassQuality::from(GrassQualitySpec::Low), GrassQuality::Low);
        assert_eq!(
            GrassQuality::from(GrassQualitySpec::Medium),
            GrassQuality::Medium
        );
        assert_eq!(
            GrassQuality::from(GrassQualitySpec::High),
            GrassQuality::High
        );
        assert_eq!(
            GrassQuality::from(GrassQualitySpec::Ultra),
            GrassQuality::Ultra
        );
    }

    #[test]
    fn style_spec_carries_colors_and_species() {
        let mut spec = GrassStyleSpec {
            biome: 3,
            density: 12.0,
            height: Vec2::new(0.2, 0.4),
            root_color: Color::linear_rgb(0.1, 0.2, 0.1),
            tip_color: Color::linear_rgb(0.3, 0.5, 0.2),
            ..Default::default()
        };
        spec.species.push(GrassPlantSpec {
            share: 0.05,
            head_length: 0.02,
            ..Default::default()
        });
        let style = style_from_spec(&spec);
        assert_eq!(style.density, 12.0);
        assert_eq!(style.blade.height, Vec2::new(0.2, 0.4));
        assert_eq!(style.blade.root_color, Color::linear_rgb(0.1, 0.2, 0.1));
        assert_eq!(style.species.len(), 1);
        assert_eq!(style.species[0].share, 0.05);
        assert_eq!(style.species[0].blade.head_length, 0.02);
    }

    #[test]
    fn settings_spec_shadow_mode() {
        let mut spec = GrassSettingsSpec::default();
        assert_eq!(spec.shadows_mode(), GrassShadows::Near);
        spec.shadows = false;
        assert_eq!(spec.shadows_mode(), GrassShadows::Off);
    }
}
