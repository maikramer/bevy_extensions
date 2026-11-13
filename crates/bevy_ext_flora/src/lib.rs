//! Definições de flora autoradas na cena para o `bevy_flora`.
//!
//! Cada espécie é uma entidade do `.bsn` com [`FloraSpecies`]: o
//! `TreeParams` completo do `bevy_flora` (editável campo a campo no
//! inspector do Jackdaw), o número de variantes do pool e a chave
//! `prop-flora` que as estações do mundo usam. [`FloraSettings`] guarda as
//! distâncias de LOD/impostor e o look frio por altitude.

use bevy::prelude::*;
use bevy_flora::prelude::{TreeParams, TreePreset};

/// Uma espécie de árvore: um pool de `variants` árvores geradas de `params`.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_flora"]
pub struct FloraSpecies {
    /// Chave `prop-flora` das estações (ex.: `pine_dark`).
    pub key: String,
    /// Variantes no pool (sementes `seed + i`).
    pub variants: u32,
    /// Ganha o look frio (mais azul) acima de [`FloraSettings::cold_look_y`].
    pub cold_look: bool,
    /// Parâmetros completos da árvore.
    pub params: TreeParams,
}

impl Default for FloraSpecies {
    fn default() -> Self {
        Self {
            key: String::new(),
            variants: 4,
            cold_look: true,
            params: TreePreset::Oak.params(),
        }
    }
}

impl FloraSpecies {
    /// Espécie nova a partir de um preset do `bevy_flora`.
    pub fn from_preset(key: impl Into<String>, preset: TreePreset) -> Self {
        Self {
            key: key.into(),
            params: preset.params(),
            ..default()
        }
    }

    /// `TreeParams` da variante `index` (só a semente muda).
    pub fn variant_params(&self, index: u32) -> TreeParams {
        let mut params = self.params.clone();
        params.seed = params.seed.wrapping_add(index as i32);
        params
    }
}

/// Distâncias de LOD/impostor e o look por altitude, para todas as espécies.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_flora"]
pub struct FloraSettings {
    /// Distância (m) da troca para o LOD1.
    pub lod1_distance: f32,
    /// Distância (m) da troca para o LOD2.
    pub lod2_distance: f32,
    /// Distância (m) a partir da qual a árvore é um impostor (quad).
    pub impostor_distance: f32,
    /// Raio (m) à volta da câmara em que as árvores projetam sombra.
    pub shadow_radius: f32,
    /// Altura local (m) acima da qual o look frio entra.
    pub cold_look_y: f32,
}

impl Default for FloraSettings {
    /// Os valores que o Vael tinha em constantes.
    fn default() -> Self {
        Self {
            lod1_distance: 45.0,
            lod2_distance: 110.0,
            impostor_distance: 260.0,
            shadow_radius: 90.0,
            cold_look_y: 55.0,
        }
    }
}

/// Presets do `bevy_flora` com nome, para menus do editor.
pub fn presets() -> Vec<(&'static str, TreePreset)> {
    TreePreset::ALL
        .iter()
        .map(|preset| (preset.name(), *preset))
        .collect()
}

/// Regista os tipos para reflexão (jogo e editor).
pub struct FloraSettingsPlugin;

impl Plugin for FloraSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<FloraSpecies>()
            .register_type::<FloraSettings>();
    }
}
