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

use avian3d::prelude::{
    AngularVelocity, ConstantForce, ConstantTorque, LinearVelocity, Position, RigidBody,
    RigidBodyDisabled, Rotation,
};
use bevy::prelude::*;

use crate::buoyancy::{FloatState, FloatingBody, hydrostatic_forces};
use crate::environment::WaterEnvironment;
use crate::source::{WaterObstacles, WaterSource};
use crate::waves::WaterKinematics;

/// Marca a entidade avian que deve flutuar (é o `FloatingBody` que define o
/// casco). O plugin insere `ConstantForce`/`ConstantTorque` se faltarem.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
#[require(ConstantForce, ConstantTorque)]
pub struct AvianFloating;

/// Aplica as forças a cada corpo `AvianFloating` com `FloatingBody`.
#[allow(clippy::type_complexity)]
pub fn apply_avian_buoyancy<S: WaterSource>(
    time: Res<Time>,
    source: Option<Res<S>>,
    env: Option<Res<WaterEnvironment>>,
    obstacles: Option<Res<WaterObstacles>>,
    mut bodies: Query<
        (
            &RigidBody,
            Has<RigidBodyDisabled>,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &FloatingBody,
            &mut ConstantForce,
            &mut ConstantTorque,
            Option<&mut WaterKinematics>,
            Option<&mut FloatState>,
        ),
        With<AvianFloating>,
    >,
) {
    let wave_env = env.as_deref().cloned().unwrap_or_default();
    let t = time.elapsed_secs();
    let obstacle_list: &[crate::obstacles::WaterObstacle] =
        obstacles.as_deref().map_or(&[], |o| o.obstacles.as_slice());

    for (
        rigid_body,
        disabled,
        position,
        rotation,
        linear,
        angular,
        body,
        mut force,
        mut torque,
        kin,
        state,
    ) in &mut bodies
    {
        let Some(source) = source.as_deref() else {
            force.0 = Vec3::ZERO;
            torque.0 = Vec3::ZERO;
            continue;
        };
        let sample = |p: Vec2| {
            let domain = source.domain_at(p, obstacle_list);
            let flow = domain.map_or(Vec2::ZERO, |d| d.flow);
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
            body, position.0, rotation.0, linear.0, angular.0, sample, wave,
        );
        let local_spin = rotation.0.inverse() * angular.0;
        // Avian accumulates constant forces while a static hull has no solver
        // body. Feeding buoyancy during that time launches it on release.
        if rigid_body.is_dynamic() && !disabled {
            force.0 = f.force;
            let damping = f.inertia * local_spin * (body.angular_drag * f.submerged.sqrt());
            torque.0 = f.torque - rotation.0 * damping;
        } else {
            force.0 = Vec3::ZERO;
            torque.0 = Vec3::ZERO;
        }
        if let Some(mut state) = state {
            let (yaw, pitch, roll) = rotation.0.to_euler(EulerRot::YXZ);
            *state = FloatState {
                velocity: linear.0,
                yaw,
                pitch,
                roll,
                pitch_vel: local_spin.x,
                roll_vel: local_spin.z,
                submerged: f.submerged,
                grounded: f.grounded,
            };
        }
        if let Some(mut kin) = kin {
            kin.velocity = linear.0;
        }
    }
}

/// Atualiza as forças uma vez por passo fixo, antes da integração do Avian.
pub struct AvianBuoyancyPlugin<S: WaterSource>(pub std::marker::PhantomData<S>);

impl<S: WaterSource> Default for AvianBuoyancyPlugin<S> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<S: WaterSource> Plugin for AvianBuoyancyPlugin<S> {
    fn build(&self, app: &mut App) {
        app.register_type::<AvianFloating>()
            .add_systems(FixedUpdate, apply_avian_buoyancy::<S>);
    }
}
