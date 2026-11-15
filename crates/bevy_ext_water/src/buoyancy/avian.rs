//! Adapter de flutuação para corpos rígidos [avian3d].
//!
//! Em vez do integrador restrito ([`crate::buoyancy::step_float`]), aplica as
//! MESMAS forças hidrostáticas a um `RigidBody` dinâmico do avian: o motor
//! integra translação/rotação, trata das colisões que o jogo tenha e o
//! resultado continua a alimentar `WaterKinematics` para a esteira.
//!
//! O corpo precisa de `RigidBody::Dynamic` e `GravityScale` normal (a
//! gravidade é do avian; [`crate::buoyancy::hydrostatic_forces`] devolve só
//! empuxo, arrasto e contacto com o leito).

use avian3d::prelude::{AngularVelocity, ConstantForce, ConstantTorque, LinearVelocity, RigidBody};
use bevy::prelude::*;

use crate::buoyancy::{FloatingBody, hydrostatic_forces};
use crate::environment::WaterEnvironment;
use crate::source::{WaterObstacles, WaterSource};
use crate::waves::{WaterKinematics, WaterWaveWorld};

/// Marca a entidade avian que deve flutuar (é o `FloatingBody` que define o
/// casco). O plugin insere `ConstantForce`/`ConstantTorque` se faltarem.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct AvianFloating;

/// Aplica as forças a cada corpo `AvianFloating` com `FloatingBody`.
#[allow(clippy::type_complexity)]
pub fn apply_avian_buoyancy<S: WaterSource>(
    time: Res<Time>,
    source: Option<Res<S>>,
    env: Option<Res<WaterEnvironment>>,
    obstacles: Option<Res<WaterObstacles>>,
    waves: Option<Res<WaterWaveWorld>>,
    mut bodies: Query<
        (
            &Transform,
            &LinearVelocity,
            &AngularVelocity,
            &FloatingBody,
            &mut ConstantForce,
            &mut ConstantTorque,
            Option<&mut WaterKinematics>,
        ),
        With<AvianFloating>,
    >,
) {
    let Some(source) = source else {
        return;
    };
    let wave_env = env.as_deref().cloned().unwrap_or_default();
    let t = wave_env.time.max(time.elapsed_secs());
    let obstacle_list: &[crate::obstacles::WaterObstacle] = obstacles
        .as_deref()
        .map_or(&[], |o| o.obstacles.as_slice());

    for (transform, linear, angular, body, mut force, mut torque, kin) in &mut bodies {
        let sample = |p: Vec2| {
            let domain = source.domain_at(p, obstacle_list);
            let flow = domain
                .map(|d| {
                    waves
                        .as_deref()
                        .map_or(d.flow, |w| w.effective_flow_at(p, d.flow))
                })
                .unwrap_or(Vec2::ZERO);
            crate::buoyancy::FloatSample {
                surface_y: domain.map(|d| d.surface_y),
                bed_y: source.bed_y(p.x, p.y),
                flow,
            }
        };
        let wave = |p: Vec2, depth: f32| {
            crate::mirror::surface_height_at(
                p,
                t,
                wave_env.wind,
                wave_env.wind_strength,
                wave_env.wave_amp,
                depth,
            )
        };
        let f = hydrostatic_forces(
            body,
            transform.translation,
            transform.rotation,
            linear.0,
            angular.0,
            sample,
            wave,
        );
        force.0 = f.force;
        torque.0 = f.torque;
        if let Some(mut kin) = kin {
            kin.velocity = linear.0;
        }
    }
}

/// Plugin do adapter avian: garante os acumuladores de força e agenda o
/// sistema depois do scheduler de física do avian (a força deste frame é a
/// que o passo seguinte consome).
pub struct AvianBuoyancyPlugin<S: WaterSource>(pub std::marker::PhantomData<S>);

impl<S: WaterSource> Default for AvianBuoyancyPlugin<S> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<S: WaterSource> Plugin for AvianBuoyancyPlugin<S> {
    fn build(&self, app: &mut App) {
        app.register_type::<AvianFloating>()
            .add_systems(Update, ensure_force_accumulators)
            .add_systems(
                Update,
                apply_avian_buoyancy::<S>
                    .after(ensure_force_accumulators)
                    .before(avian3d::schedule::PhysicsSystems::StepSimulation),
            );
    }
}

fn ensure_force_accumulators(
    mut commands: Commands,
    bodies: Query<
        Entity,
        (
            With<AvianFloating>,
            With<RigidBody>,
            Without<ConstantForce>,
        ),
    >,
) {
    for entity in &bodies {
        commands
            .entity(entity)
            .insert((ConstantForce::default(), ConstantTorque::default()));
    }
}
