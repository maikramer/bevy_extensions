//! Trampling: entidades que dobram a relva.
//!
//! O jogo marca o que deve achatar a relva com [`GrassTrampler`]. Com a
//! feature `avian` (default), um `Collider` avian na mesma entidade vira um
//! `GrassInteractorFromCollider` (a pegada segue a forma). Sem collider, um
//! [`GrassTramplerDisc`] dá a pegada manual (raio/força/altura).

use bevy::prelude::*;
use bevy_grass::{GrassInteractor, GrassInteractorFromCollider};

/// Marca uma entidade (herói, NPC, criatura, veículo) que dobra a relva.
#[derive(Component, Default, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_grass"]
pub struct GrassTrampler;

/// Pegada manual para quem não tem collider de física.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_grass"]
pub struct GrassTramplerDisc {
    /// Raio do disco achatado (m).
    pub radius: f32,
    /// Quão plana fica a relva no centro (0..1).
    pub strength: f32,
    /// Altura máxima acima do chão em que ainda afeta a relva.
    pub max_height: Option<f32>,
}

impl Default for GrassTramplerDisc {
    fn default() -> Self {
        Self {
            radius: 0.45,
            strength: 0.9,
            max_height: Some(1.0),
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<GrassTrampler>()
        .register_type::<GrassTramplerDisc>()
        .add_systems(
            Update,
            (attach_collider_interactors, attach_disc_interactors)
                .in_set(crate::GrassSystems::SyncSpecs),
        );
}

fn attach_collider_interactors(
    mut commands: Commands,
    colliders: Query<
        Entity,
        (
            With<GrassTrampler>,
            With<avian3d::prelude::Collider>,
            Without<GrassInteractorFromCollider>,
        ),
    >,
) {
    for entity in &colliders {
        commands
            .entity(entity)
            .insert(GrassInteractorFromCollider::default());
    }
}

fn attach_disc_interactors(
    mut commands: Commands,
    discs: Query<
        (Entity, &GrassTramplerDisc),
        (
            Without<GrassInteractor>,
            Without<avian3d::prelude::Collider>,
        ),
    >,
) {
    for (entity, disc) in &discs {
        commands.entity(entity).insert(GrassInteractor {
            radius: disc.radius,
            strength: disc.strength,
            max_height: disc.max_height,
        });
    }
}
