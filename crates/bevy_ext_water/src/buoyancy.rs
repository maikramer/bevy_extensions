//! Buoyancy & floating rigid bodies on the hydrodynamic water domain.
//!
//! Consumes the host's [`crate::source::WaterSource`] (`domain_at`/`bed_y`) for the surface, bed and
//! current, plus the CPU mirror of the *drawn* Gerstner swell
//! ([`surface_height_at`]) for heave, and integrates a constrained rigid body:
//!
//! - **3 translation DOF** — gravity, Archimedes per hull column, quadratic
//!   hydrodynamic drag, river current advection and bed contact (beaching);
//! - **2 rotation DOF** — pitch and roll from the off-centre buoyancy of the
//!   columns; yaw stays authored/steered (a rowboat does not yaw from a
//!   displaced centre of buoyancy, the oars/helm do).
//!
//! The hull is a box sampled as a `3 x 3` grid of vertical columns. Each
//! column displaces `area * submerged_height` at its own XZ, so the restoring
//! moment (metacentric stability) falls out of the geometry instead of a
//! hand-tuned spring: a beamier hull rolls less, a top-heavy one capsizes.
//!
//! The result is published back into [`WaterKinematics`] so the existing
//! splash/wake pipeline (`water_fx`) sees boats exactly like swimmers.

use bevy::prelude::*;

use crate::mirror::surface_height_at;
use crate::environment::WaterEnvironment;
use crate::source::{WaterObstacles, WaterSource};
use crate::waves::{WaterKinematics, WaterWaveWorld};

#[cfg(feature = "avian")]
pub mod avian;

/// Density of the world's water (kg/m³) — slightly saline, like the sea.
pub const WATER_DENSITY: f32 = 1025.0;
/// Gravity used by the float integrator (m/s²).
pub const FLOAT_GRAVITY: f32 = 9.81;
/// Largest physics step; frames longer than this are split into substeps.
pub const MAX_SUBSTEP: f32 = 1.0 / 90.0;
/// Hard cap on substeps per frame (a 10 fps frame is still stable).
pub const MAX_SUBSTEPS: usize = 5;
/// Columns per hull axis (a `3 x 3` grid = 9 columns).
pub const HULL_COLUMNS: usize = 3;
/// Pitch/roll clamp (rad) — no boat should ever cartwheel from a wave.
pub const MAX_TILT: f32 = 0.55;

/// Configuration of a floating body (authored on the entity).
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct FloatingBody {
    /// Displacement mass in kg (hull + cargo + rider).
    pub mass: f32,
    /// Half extents of the displacement box, local axes: `x` = half beam,
    /// `y` = half height (keel to gunwale), `z` = half length.
    pub hull_half: Vec3,
    /// Local offset of the displacement box centre from the entity origin
    /// (the origin is conventionally the design waterline).
    pub hull_center: Vec3,
    /// Local centre of mass (keep it low: it sets the metacentric height).
    pub center_of_mass: Vec3,
    /// How much of the box volume actually displaces water (a V-hull is
    /// ~0.5, a barge ~0.9).
    pub displacement_factor: f32,
    /// Archimedes multiplier (1.0 = neutral; <1 sits deeper, >1 floats high).
    pub buoyancy_gain: f32,
    /// Linear drag per second (viscous, at low speed).
    pub linear_drag: f32,
    /// Quadratic drag (turbulent, dominates above ~1 m/s). Units 1/m.
    pub quadratic_drag: f32,
    /// Angular velocity damping (1/s), scaled by submersion.
    pub angular_drag: f32,
    /// Vertical (heave) damping while afloat, 1/s. The water grabs a hull far
    /// harder in Y than in XZ (radiation damping); without it a boat bobs for
    /// tens of seconds after a wave and the equilibrium test never settles.
    pub heave_drag: f32,
    /// Quadratic heave damping (1/m).
    pub heave_quadratic: f32,
    /// How strongly the drawn swell moves the hull (0 = glassy, 1 = normal).
    pub wave_response: f32,
    /// Horizontal speed cap (m/s) — safety valve for currents/impulses.
    pub max_speed: f32,
    /// Bed penetration spring (N/m) for beaching.
    pub ground_stiffness: f32,
    /// Bed penetration damper (N·s/m).
    pub ground_damping: f32,
    /// Bed friction coefficient (N per m of penetration per m/s).
    pub ground_friction: f32,
}

impl Default for FloatingBody {
    fn default() -> Self {
        // A small wooden crate / drifting debris.
        Self {
            mass: 40.0,
            hull_half: Vec3::new(0.32, 0.24, 0.42),
            hull_center: Vec3::new(0.0, -0.14, 0.0),
            center_of_mass: Vec3::new(0.0, -0.08, 0.0),
            displacement_factor: 0.82,
            buoyancy_gain: 1.0,
            linear_drag: 1.4,
            quadratic_drag: 1.9,
            angular_drag: 3.2,
            heave_drag: 5.0,
            heave_quadratic: 0.9,
            wave_response: 1.0,
            max_speed: 12.0,
            ground_stiffness: 14000.0,
            ground_damping: 1200.0,
            ground_friction: 3.0,
        }
    }
}

impl FloatingBody {
    /// Waterplane area used by the displacement columns (m²).
    pub fn wet_area(&self) -> f32 {
        let h = self.hull_half;
        (4.0 * h.x * h.z * self.displacement_factor).max(0.01)
    }

    /// Draft at rest in still water (m): the depth of the KEEL below the
    /// surface when the displaced volume balances [`Self::mass`]. The entity
    /// origin sits wherever the authored hull puts it
    /// (`draft = |hull_center.y − hull_half.y|` below the origin).
    pub fn equilibrium_draft(&self) -> f32 {
        (self.mass.max(0.0) / (WATER_DENSITY * self.wet_area() * self.buoyancy_gain.max(0.05)))
            .min(2.0 * self.hull_half.y)
    }

    /// Origin height above the water surface at rest (m): spawn a body at
    /// `surface + rest_height()` and it starts already in equilibrium.
    pub fn rest_height(&self) -> f32 {
        self.hull_half.y - self.hull_center.y - self.equilibrium_draft()
    }

    fn sanitized(mut self) -> Self {
        self.mass = self.mass.max(0.5);
        self.hull_half = self.hull_half.max(Vec3::splat(0.05));
        self.displacement_factor = self.displacement_factor.clamp(0.05, 1.0);
        self.buoyancy_gain = self.buoyancy_gain.clamp(0.05, 4.0);
        self.linear_drag = self.linear_drag.max(0.0);
        self.quadratic_drag = self.quadratic_drag.max(0.0);
        self.angular_drag = self.angular_drag.max(0.0);
        self.wave_response = self.wave_response.clamp(0.0, 2.0);
        self.max_speed = self.max_speed.max(0.5);
        self
    }
}

/// Runtime state of a floating body (integrated every frame).
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct FloatState {
    /// World-space linear velocity (m/s).
    pub velocity: Vec3,
    /// Heading around +Y (rad); authored or steered, not physics-driven.
    pub yaw: f32,
    /// Pitch around local X (rad).
    pub pitch: f32,
    /// Roll around local Z (rad).
    pub roll: f32,
    /// Pitch angular velocity (rad/s).
    pub pitch_vel: f32,
    /// Roll angular velocity (rad/s).
    pub roll_vel: f32,
    /// Fraction of the hull volume underwater (0..1), for FX/tuning.
    pub submerged: f32,
    /// True when any hull column is pressed against the bed.
    pub grounded: bool,
}

impl Default for FloatState {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            pitch_vel: 0.0,
            roll_vel: 0.0,
            submerged: 0.0,
            grounded: false,
        }
    }
}

impl FloatState {
    /// Rotation composed as yaw × pitch × roll (the boat convention).
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
            * Quat::from_rotation_x(self.pitch)
            * Quat::from_rotation_z(self.roll)
    }
}

/// Water column under one hull sample point.
#[derive(Debug, Clone, Copy)]
pub struct FloatSample {
    /// Water surface Y, `None` when the point is dry.
    pub surface_y: Option<f32>,
    /// Terrain bed Y, `None` outside the heightfield.
    pub bed_y: Option<f32>,
    /// Surface current at the point (m/s, XZ).
    pub flow: Vec2,
}

impl FloatSample {
    /// Dry land with no water column.
    pub fn dry(bed_y: Option<f32>) -> Self {
        Self {
            surface_y: None,
            bed_y,
            flow: Vec2::ZERO,
        }
    }

    /// Local water depth (m); `0.0` when dry.
    pub fn depth(&self) -> f32 {
        match (self.surface_y, self.bed_y) {
            (Some(s), Some(b)) => (s - b).max(0.0),
            (Some(_), None) => 1.5,
            _ => 0.0,
        }
    }
}

/// Forças hidrostáticas/hidrodinâmicas num instante, no referencial do mundo.
#[derive(Debug, Clone, Copy)]
pub struct FloatForces {
    /// Soma das forças (SEM gravidade — o integrador do host decide).
    pub force: Vec3,
    /// Torque em torno do centro de massa (mundo).
    pub torque: Vec3,
    /// Centro de massa em coordenadas de mundo.
    pub center_of_mass: Vec3,
    /// Fração do volume deslocado (0..1).
    pub submerged: f32,
    /// Alguma coluna a raspar no leito.
    pub grounded: bool,
    /// Tensor de inércia diagonal (kg·m²) da caixa.
    pub inertia: Vec3,
}

/// Acumula as forças de um corpo flutuante num passo.
///
/// `origin` é a origem da entidade; `rot` a rotação completa (o integrador
/// restrito usa yaw×pitch×roll; o adapter avian usa a rotação do corpo
/// rígido); `velocity`/`angular_velocity` são do corpo no mundo. A gravidade
/// NÃO entra aqui: o integrador próprio soma-a e o avian já a aplica.
pub fn hydrostatic_forces(
    body: &FloatingBody,
    origin: Vec3,
    rot: Quat,
    velocity: Vec3,
    angular_velocity: Vec3,
    sample: impl Fn(Vec2) -> FloatSample,
    wave: impl Fn(Vec2, f32) -> f32,
) -> FloatForces {
    let body = body.sanitized();
    let m = body.mass;
    let half = body.hull_half;
    let com_world = origin + rot * body.center_of_mass;
    let inertia = Vec3::new(
        m / 3.0 * (half.y * half.y + half.z * half.z),
        m / 3.0 * (half.x * half.x + half.z * half.z),
        m / 3.0 * (half.x * half.x + half.y * half.y),
    )
    .max(Vec3::splat(0.05));

    let n = HULL_COLUMNS;
    let columns = (n * n) as f32;
    let column_area = body.wet_area() / columns;
    let hull_height = 2.0 * half.y;

    let mut force = Vec3::ZERO;
    let mut torque = Vec3::ZERO;
    let mut submerged_fraction = 0.0_f32;
    let mut grounded = false;

    for iz in 0..n {
        for ix in 0..n {
            let u = (ix as f32 / (n - 1) as f32) * 2.0 - 1.0;
            let v = (iz as f32 / (n - 1) as f32) * 2.0 - 1.0;
            let local = body.hull_center + Vec3::new(u * half.x, 0.0, v * half.z);
            let world = origin + rot * local;
            let xz = world.xz();
            let s = sample(xz);
            let keel = world.y - half.y;

            // ── bed contact (beaching) ─────────────────────────────────
            if let Some(bed) = s.bed_y {
                if bed > keel {
                    let penetration = bed - keel;
                    let up = body.ground_stiffness * penetration
                        - body.ground_damping * velocity.y;
                    let up = up.max(0.0);
                    let r = world - com_world;
                    let f = Vec3::Y * up;
                    force += f;
                    torque += r.cross(f);
                    // Friction against the horizontal motion of the contact.
                    // `share` keeps the aggregate force independent of the
                    // column count (9 columns would otherwise mean 9x).
                    let share = 1.0 / columns;
                    let hv = Vec3::new(velocity.x, 0.0, velocity.z);
                    let friction =
                        -hv * (body.ground_friction * penetration.min(0.30) * m * share);
                    force += friction;
                    torque += r.cross(friction);
                    grounded = true;
                }
            }

            let Some(surface) = s.surface_y else {
                continue;
            };
            let depth = s.depth();
            let drawn = surface + wave(xz, depth) * body.wave_response;
            let submerged = (drawn - keel).clamp(0.0, hull_height);
            let frac = submerged / hull_height;
            submerged_fraction += frac / columns;
            if submerged <= 1e-4 {
                continue;
            }

            // ── Archimedes at the column centre ───────────────────────
            let displaced_volume = column_area * submerged;
            let buoy = Vec3::Y
                * (WATER_DENSITY * FLOAT_GRAVITY * displaced_volume * body.buoyancy_gain);
            let at = Vec3::new(world.x, keel + submerged * 0.5, world.z);
            let r = at - com_world;
            force += buoy;
            torque += r.cross(buoy);

            // ── hydrodynamic drag against the local current ───────────
            // Coeficientes por unidade de MASSA (1/s e 1/m): a força leva o
            // `m` para o tempo de resposta do casco não depender do peso.
            // Cada coluna é 1/9 do casco (`share`), senão o arrasto somava
            // nove vezes.
            let share = m / columns;
            let point_vel = velocity + angular_velocity.cross(at - com_world);
            let water_vel = Vec3::new(s.flow.x, 0.0, s.flow.y);
            let rel = point_vel - water_vel;
            let speed = rel.length();
            let drag = rel
                * (-body.linear_drag * frac * share
                    - body.quadratic_drag * frac * share * speed);
            force += drag;
            torque += (at - com_world).cross(drag);
        }
    }

    // Heave (Y) radiation damping — só com o casco molhado. Sem isto a mola
    // de Arquimedes oscila quase sem perdas (ζ≈0,02) e o barco passa
    // dezenas de segundos a boiar acima/abaixo do equilíbrio.
    if submerged_fraction > 0.01 {
        let vy = velocity.y;
        force.y -= m * (body.heave_drag * vy + body.heave_quadratic * vy.abs() * vy);
    }

    FloatForces {
        force,
        torque,
        center_of_mass: com_world,
        submerged: submerged_fraction.clamp(0.0, 1.0),
        grounded,
        inertia,
    }
}

/// One integration step of a floating body (restricted integrator: 3D
/// translation + pitch/roll; the yaw is authored/steered).
///
/// `sample` returns the water column at a world XZ; `wave` returns the drawn
/// swell height at `(xz, local_depth)`. Keeping both as closures lets the
/// engine systems feed the live terrain and lets tests run without an ECS.
pub fn step_float(
    body: &FloatingBody,
    state: &mut FloatState,
    position: &mut Vec3,
    dt: f32,
    sample: impl Fn(Vec2) -> FloatSample,
    wave: impl Fn(Vec2, f32) -> f32,
) {
    let body = body.sanitized();
    let m = body.mass;
    let dt = dt.clamp(0.0, MAX_SUBSTEP * 1.5);
    if dt <= 0.0 {
        return;
    }

    let rot = state.rotation();
    let angular = rot * Vec3::new(state.pitch_vel, 0.0, state.roll_vel);
    let f = hydrostatic_forces(&body, *position, rot, state.velocity, angular, sample, wave);
    let force = f.force + Vec3::NEG_Y * (m * FLOAT_GRAVITY);

    // ── integrate translation ─────────────────────────────────────────
    state.velocity += force / m * dt;
    let planar = Vec2::new(state.velocity.x, state.velocity.z);
    if planar.length() > body.max_speed {
        let scaled = planar.normalize() * body.max_speed;
        state.velocity.x = scaled.x;
        state.velocity.z = scaled.y;
    }
    state.velocity.y = state.velocity.y.clamp(-24.0, 16.0);
    *position += state.velocity * dt;

    // ── integrate pitch & roll (yaw stays authored/steered) ───────────
    let torque_local = rot.inverse() * f.torque;
    state.pitch_vel += torque_local.x / f.inertia.x * dt;
    state.roll_vel += torque_local.z / f.inertia.z * dt;
    let damp = (-body.angular_drag * (0.25 + 0.75 * f.submerged) * dt).exp();
    state.pitch_vel *= damp;
    state.roll_vel *= damp;
    state.pitch = (state.pitch + state.pitch_vel * dt).clamp(-MAX_TILT, MAX_TILT);
    state.roll = (state.roll + state.roll_vel * dt).clamp(-MAX_TILT, MAX_TILT);
    if state.pitch.abs() >= MAX_TILT {
        state.pitch_vel = 0.0;
    }
    if state.roll.abs() >= MAX_TILT {
        state.roll_vel = 0.0;
    }

    state.submerged = f.submerged;
    state.grounded = f.grounded;
}

/// Live wind/amplitude set shared by every float this frame.
#[derive(Debug, Clone, Copy)]
struct WaveEnv {
    wind: [f32; 2],
    strength: f32,
    amp: f32,
    time: f32,
}

fn wave_env(env: Option<&WaterEnvironment>, time: f32) -> WaveEnv {
    let env = env.cloned().unwrap_or_default();
    WaveEnv {
        wind: env.wind,
        strength: env.wind_strength,
        amp: env.wave_amp,
        time: if time > 0.0 { time } else { env.time },
    }
}

#[cfg(feature = "avian")]
type IntegratedHere = Without<avian::AvianFloating>;
#[cfg(not(feature = "avian"))]
type IntegratedHere = ();

/// Integrates every [`FloatingBody`] and mirrors the result into [`WaterKinematics`].
#[allow(clippy::type_complexity)]
fn step_floating_bodies<S: WaterSource>(
    time: Res<Time>,
    source: Option<Res<S>>,
    env: Option<Res<WaterEnvironment>>,
    obstacles: Option<Res<WaterObstacles>>,
    waves: Option<Res<WaterWaveWorld>>,
    mut floats: Query<(
        &mut Transform,
        &FloatingBody,
        &mut FloatState,
        Option<&mut WaterKinematics>,
    ), IntegratedHere>,
) {
    let Some(source) = source else {
        return;
    };
    let dt = time.delta_secs().min(0.05);
    if dt <= 0.0 {
        return;
    }
    let env = wave_env(env.as_deref(), time.elapsed_secs());
    let steps = ((dt / MAX_SUBSTEP).ceil() as usize).clamp(1, MAX_SUBSTEPS);
    let h = dt / steps as f32;

    for (mut transform, body, mut state, kinematics) in &mut floats {
        let mut position = transform.translation;
        for _ in 0..steps {
            let obstacle_list: &[crate::obstacles::WaterObstacle] = obstacles
                .as_deref()
                .map_or(&[], |o| o.obstacles.as_slice());
            let sample = |p: Vec2| -> FloatSample {
                let domain = source.domain_at(p, obstacle_list);
                let flow = domain
                    .map(|d| {
                        waves
                            .as_deref()
                            .map_or(d.flow, |w| w.effective_flow_at(p, d.flow))
                    })
                    .unwrap_or(Vec2::ZERO);
                let bed = source.bed_y(p.x, p.y);
                FloatSample {
                    surface_y: domain.map(|d| d.surface_y),
                    bed_y: bed,
                    flow,
                }
            };
            let wave = |p: Vec2, depth: f32| {
                surface_height_at(p, env.time, env.wind, env.strength, env.amp, depth)
            };
            step_float(body, &mut state, &mut position, h, sample, wave);
        }
        transform.translation = position;
        transform.rotation = state.rotation();
        if let Some(mut kin) = kinematics {
            kin.velocity = state.velocity;
        }
    }
}

/// Registers the buoyancy integrator. Generic: any entity with
/// [`FloatingBody`] + [`FloatState`] floats on the nearest water body.
pub struct WaterBuoyancyPlugin<S: WaterSource>(pub std::marker::PhantomData<S>);

impl<S: WaterSource> Default for WaterBuoyancyPlugin<S> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<S: WaterSource> Plugin for WaterBuoyancyPlugin<S> {
    fn build(&self, app: &mut App) {
        app.register_type::<FloatingBody>()
            .register_type::<FloatState>()
            .add_systems(Update, step_floating_bodies::<S>);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWBOAT: FloatingBody = FloatingBody {
        mass: 480.0,
        hull_half: Vec3::new(0.75, 0.30, 2.10),
        hull_center: Vec3::new(0.0, -0.24, 0.0),
        center_of_mass: Vec3::new(0.0, -0.18, 0.0),
        displacement_factor: 0.55,
        buoyancy_gain: 1.0,
        linear_drag: 1.2,
        quadratic_drag: 2.4,
        angular_drag: 3.0,
        heave_drag: 5.0,
        heave_quadratic: 0.9,
        wave_response: 1.0,
        max_speed: 9.0,
        ground_stiffness: 14000.0,
        ground_damping: 1200.0,
        ground_friction: 5.0,
    };

    fn lake(surface: f32, bed: f32) -> impl Fn(Vec2) -> FloatSample {
        move |_| FloatSample {
            surface_y: Some(surface),
            bed_y: Some(bed),
            flow: Vec2::ZERO,
        }
    }

    fn run(
        body: &FloatingBody,
        state: &mut FloatState,
        pos: &mut Vec3,
        seconds: f32,
        sample: impl Fn(Vec2) -> FloatSample,
        wave: impl Fn(Vec2, f32) -> f32,
    ) {
        let steps = (seconds / MAX_SUBSTEP).round() as usize;
        for _ in 0..steps {
            step_float(body, state, pos, MAX_SUBSTEP, &sample, &wave);
        }
    }

    #[test]
    fn still_water_settles_at_the_analytic_draft() {
        let mut state = FloatState::default();
        let mut pos = Vec3::new(0.0, 2.0, 0.0);
        run(&ROWBOAT, &mut state, &mut pos, 8.0, lake(10.0, 7.0), |_, _| 0.0);

        let expected = ROWBOAT.equilibrium_draft();
        let keel = pos.y + ROWBOAT.hull_center.y - ROWBOAT.hull_half.y;
        let draft = 10.0 - keel;
        assert!(
            (draft - expected).abs() < 0.03,
            "keel draft {draft} m, expected {expected}: {pos:?}"
        );
        assert!(
            state.velocity.length() < 0.05,
            "settled body should be at rest: {:?}",
            state.velocity
        );
        assert!(
            (state.submerged - 0.25).abs() < 0.15,
            "a 0.6 m hull at 0.14 m draft is ~23% submerged, got {}",
            state.submerged
        );
    }

    #[test]
    fn displaced_roll_rights_itself() {
        let mut state = FloatState {
            roll: 0.30,
            ..Default::default()
        };
        let mut pos = Vec3::new(0.0, 0.0, 0.0);
        run(
            &ROWBOAT,
            &mut state,
            &mut pos,
            4.0,
            lake(0.0, -4.0),
            |_, _| 0.0,
        );
        assert!(
            state.roll.abs() < 0.05,
            "metacentric righting should flatten the hull: {}",
            state.roll
        );
        assert!(state.pitch.abs() < 0.05, "no residual pitch");

        let mut top_heavy = ROWBOAT;
        top_heavy.center_of_mass = Vec3::new(0.0, 1.6, 0.0);
        let mut capsized = FloatState {
            roll: MAX_TILT,
            ..Default::default()
        };
        run(
            &top_heavy,
            &mut capsized,
            &mut pos,
            4.0,
            lake(0.0, -4.0),
            |_, _| 0.0,
        );
        assert!(
            capsized.roll.abs() > 0.20,
            "a top-heavy body must NOT right itself (GM negative): {}",
            capsized.roll
        );
    }

    #[test]
    fn current_advects_the_body_but_below_the_flow_speed() {
        let flow = Vec2::new(1.6, 0.0);
        let sample = move |_: Vec2| FloatSample {
            surface_y: Some(0.0),
            bed_y: Some(-4.0),
            flow,
        };
        let mut state = FloatState::default();
        let mut pos = Vec3::ZERO;
        run(&ROWBOAT, &mut state, &mut pos, 6.0, sample, |_, _| 0.0);

        assert!(
            state.velocity.x > 0.4,
            "the current must carry the hull: {:?}",
            state.velocity
        );
        assert!(
            state.velocity.x < flow.x,
            "hull drag keeps it below the current speed: {:?}",
            state.velocity
        );
        assert!(pos.x > 2.0, "it actually travels downstream: {pos:?}");
    }

    #[test]
    fn dry_body_falls_and_rests_on_the_bed() {
        let mut state = FloatState::default();
        let mut pos = Vec3::new(0.0, 5.0, 0.0);
        let sample = |_: Vec2| FloatSample::dry(Some(0.0));
        run(&ROWBOAT, &mut state, &mut pos, 6.0, sample, |_, _| 0.0);
        assert!(state.grounded, "bed contact must stop the fall");
        assert!(
            (pos.y - (0.0 + ROWBOAT.hull_half.y - ROWBOAT.hull_center.y)).abs() < 0.10,
            "keel rests on the bed, origin at {pos:?}"
        );
        assert!(state.velocity.y.abs() < 0.4, "no bounce: {:?}", state.velocity);
    }

    #[test]
    fn swell_heaves_the_hull_within_bounds() {
        let mut state = FloatState::default();
        let mut pos = Vec3::new(0.0, 0.0, 0.0);
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for i in 0..600 {
            let t = i as f32 * MAX_SUBSTEP;
            let wave = |_: Vec2, _: f32| 0.09 * (t * 2.2).sin();
            step_float(
                &ROWBOAT,
                &mut state,
                &mut pos,
                MAX_SUBSTEP,
                lake(0.0, -4.0),
                wave,
            );
            min_y = min_y.min(pos.y);
            max_y = max_y.max(pos.y);
        }
        assert!(
            max_y - min_y > 0.02,
            "a swell must move the hull: range {}",
            max_y - min_y
        );
        assert!(
            max_y - min_y < 0.60,
            "heave stays physical (no ejection): range {}",
            max_y - min_y
        );
        assert!(state.pitch.abs() < 0.2 && state.roll.abs() < 0.2);
    }

    #[test]
    fn wet_area_and_equilibrium_draft_are_consistent() {
        let body = ROWBOAT;
        let area = body.wet_area();
        assert!((area - (4.0 * 0.75 * 2.10 * 0.55)).abs() < 1e-5);
        let draft = body.equilibrium_draft();
        let displaced = WATER_DENSITY * area * draft;
        assert!(
            (displaced - body.mass).abs() < 1.0,
            "Archimedes balances the authored mass: {displaced} vs {}",
            body.mass
        );
    }
}
