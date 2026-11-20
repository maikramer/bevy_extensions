//! Interactive sandbox & automated showcase for `bevy_ext_puppet`.
//!
//! Run interactive mode:
//! ```text
//! cargo run -p bevy_ext_puppet --example puppet_sandbox --features sandbox --release
//! ```
//!
//! Run automated showcase of every behaviour (can pause/skip anytime with `Space`/`N`/`P`):
//! ```text
//! cargo run -p bevy_ext_puppet --example puppet_sandbox --features sandbox --release -- --demo
//! ```
//!
//! | Key | Action |
//! |---|---|
//! | Space | toggle automated showcase tour |
//! | N / P | next / previous showcase stage |
//! | 1 / 2 / 3 | push backwards (25 / 50 / 120 N·s) |
//! | 4 | push forwards toward the brace wall (85 N·s) |
//! | 5 | shove sideways (60 N·s) |
//! | B | glancing shoulder bump with orienting head glance |
//! | H / C / U / L | hit head / chest slash + wound clutch / gut jackknife / calf sweep |
//! | T | trip the right shin mid-stance |
//! | J | high drop (+3 m) to test airborne flail & fall break |
//! | A | activate (balance physically) |
//! | F | give up balance (fall) |
//! | K | kill (progressive dying collapse → limp ragdoll) |
//! | R | get up / revive (3-stage physical get-up) |
//! | W | toggle writhing when down |
//! | G | toggle debug gizmos (CoP, wall probe, virtual forces, gaze) |
//! | ← / → | orbit the camera |

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy_ext_puppet::prelude::*;
use bevy_ext_puppet::skeleton::spawn_test_skeleton;

fn main() {
    let auto_demo = std::env::args().any(|a| a == "--demo" || a == "-d");
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "bevy_ext_puppet — Demo Biomecânica".into(),
                    resolution: (1280, 760).into(),
                    ..default()
                }),
                ..default()
            }),
            PhysicsPlugins::default(),
            PuppetPlugin::default(),
            PuppetDebugPlugin,
        ))
        .insert_resource(ClearColor(Color::srgb(0.11, 0.13, 0.16)))
        .init_resource::<Orbit>()
        .init_resource::<EventFeed>()
        .insert_resource(DemoShowcase::new(auto_demo))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (dress_puppets, demo_director, controls, record_events, orbit_camera, hud).chain(),
        )
        .run();
}

#[derive(Component)]
struct Hero;

#[derive(Component)]
struct Hud;

#[derive(Component)]
struct DemoBanner;

#[derive(Resource)]
struct Orbit {
    yaw: f32,
    target_yaw: Option<f32>,
    dist: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            yaw: 2.55,
            target_yaw: None,
            dist: 3.9,
        }
    }
}

#[derive(Resource, Default)]
struct EventFeed {
    lines: Vec<(f32, String)>,
}

#[derive(Clone, Copy)]
enum StageAction {
    Bump,
    GutHit,
    ChestSlashWound,
    HeadHit,
    ComboHits,
    LegSweep,
    StrongPushBalance,
    RampStep,
    Trip,
    WallBrace,
    SidewaysFall,
    BackwardFallGetUp,
    HighDrop,
    DyingCollapse,
}

#[derive(Clone, Copy)]
struct DemoStage {
    title: &'static str,
    subtitle: &'static str,
    start_pos: Vec3,
    camera_yaw: f32,
    camera_dist: f32,
    duration: f32,
    action: StageAction,
}

const DEMO_STAGES: &[DemoStage] = &[
    DemoStage {
        title: "Esbarrao de Ombro + Olhar Orientador (BumpReaction)",
        subtitle: "Torcao axial suave na caixa toracica + cabeca gira para olhar quem esbarrou",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.55,
        camera_dist: 3.3,
        duration: 3.8,
        action: StageAction::Bump,
    },
    DemoStage {
        title: "Soco no Abdomen (HitReaction::Abdomen)",
        subtitle: "Dobra frontal tipo jackknife sobre o estomago + cotovelos fechados em guarda",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.20,
        camera_dist: 3.3,
        duration: 3.6,
        action: StageAction::GutHit,
    },
    DemoStage {
        title: "Corte no Peito + Mao na Ferida (HitReaction::Chest + ReachForWound)",
        subtitle: "Chicote cervical inercial + mao livre busca o ponto exato do ferimento",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.55,
        camera_dist: 3.3,
        duration: 4.0,
        action: StageAction::ChestSlashWound,
    },
    DemoStage {
        title: "Golpe na Cabeca (HitReaction::Head)",
        subtitle: "Chicote de pescoco + guarda defensiva das maos ao rosto",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.40,
        camera_dist: 3.2,
        duration: 3.6,
        action: StageAction::HeadHit,
    },
    DemoStage {
        title: "Combo Rapido -> Trauma Acumulado (Escalacao de Stagger)",
        subtitle: "3 golpes sub-limiar consecutivos acumulam trauma e rompem a base para passos",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.35,
        camera_dist: 3.8,
        duration: 5.0,
        action: StageAction::ComboHits,
    },
    DemoStage {
        title: "Varrimento na Canela (HitReaction::Leg)",
        subtitle: "Inibicao extensora + flexao brusca do joelho atingido desestabilizando a base",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.15,
        camera_dist: 3.6,
        duration: 4.5,
        action: StageAction::LegSweep,
    },
    DemoStage {
        title: "Empurrao Forte 65 N.s (Equilibrio IPM + VMC + ArmsWindmill)",
        subtitle: "Passos de captura adaptativos + forcas virtuais implicitas + moinho de bracos",
        start_pos: Vec3::ZERO,
        camera_yaw: 1.90,
        camera_dist: 4.2,
        duration: 5.2,
        action: StageAction::StrongPushBalance,
    },
    DemoStage {
        title: "Passo Adaptativo em Rampa Inclinada (Terreno + Foot Normals)",
        subtitle: "Sonda de terreno a frente eleva o arco do passo e adapta o pe a inclinacao",
        start_pos: Vec3::new(-2.8, 0.0, 0.0),
        camera_yaw: 1.85,
        camera_dist: 4.0,
        duration: 5.0,
        action: StageAction::RampStep,
    },
    DemoStage {
        title: "Tropeco em Movimento (Trip + Windmill)",
        subtitle: "Puxao na canela + projecao do tronco para frente com recuperacao fisica",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.10,
        camera_dist: 4.2,
        duration: 5.0,
        action: StageAction::Trip,
    },
    DemoStage {
        title: "Apoio na Parede (WallBrace + Arm VirtualForce)",
        subtitle: "Sonda deteta parede, estende maos com IK de cotovelo travado e protege a face",
        start_pos: Vec3::new(0.0, 0.0, -0.64),
        camera_yaw: 1.95,
        camera_dist: 3.6,
        duration: 5.2,
        action: StageAction::WallBrace,
    },
    DemoStage {
        title: "Queda Lateral (CatchFall Sideways + Endireitamento da Cabeca)",
        subtitle: "Braco inferior amortece a queda, superior protege costelas e cabeca inclina contra o chao",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.85,
        camera_dist: 4.2,
        duration: 5.5,
        action: StageAction::SidewaysFall,
    },
    DemoStage {
        title: "Queda de Costas + Levantar Fisico (CatchFall Backward + PhysicalGetUp)",
        subtitle: "Queixo no peito + rolamento em C + levantar fisico em 3 estagios (maos -> agachar -> de pe)",
        start_pos: Vec3::ZERO,
        camera_yaw: 1.95,
        camera_dist: 4.2,
        duration: 8.8,
        action: StageAction::BackwardFallGetUp,
    },
    DemoStage {
        title: "Queda Alta +3.2 m (AirborneFlail -> CatchFall -> BodyImpact -> GetUp)",
        subtitle: "Bracos e pernas agitam no ar, amortecem impacto no solo e o boneco se levanta",
        start_pos: Vec3::new(0.0, 3.2, 0.0),
        camera_yaw: 2.35,
        camera_dist: 5.2,
        duration: 8.5,
        action: StageAction::HighDrop,
    },
    DemoStage {
        title: "Colapso Progressivo de Morte (DyingCollapse + Amortecimento Passivo)",
        subtitle: "Joelhos cedem primeiro, coluna depois, bracos/cabeca por ultimo; tecido viscoelastico estabiliza",
        start_pos: Vec3::ZERO,
        camera_yaw: 2.15,
        camera_dist: 3.8,
        duration: 5.0,
        action: StageAction::DyingCollapse,
    },
];

#[derive(Resource)]
struct DemoShowcase {
    active: bool,
    index: usize,
    elapsed: f32,
    needs_reset: bool,
    fired_mask: u8,
}

impl DemoShowcase {
    fn new(active: bool) -> Self {
        Self {
            active,
            index: 0,
            elapsed: 0.0,
            needs_reset: active,
            fired_mask: 0,
        }
    }

    fn jump(&mut self, index: usize) {
        self.index = index % DEMO_STAGES.len();
        self.elapsed = 0.0;
        self.needs_reset = true;
        self.fired_mask = 0;
    }
}

fn spawn_hero(commands: &mut Commands, pos: Vec3, config: PuppetConfig) -> Entity {
    let hero = commands
        .spawn((
            Name::new("hero"),
            Hero,
            Puppet,
            config,
            Transform::from_translation(pos),
            Visibility::default(),
        ))
        .id();
    spawn_test_skeleton(commands, hero);
    hero
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    showcase: Res<DemoShowcase>,
) {
    let ground = materials.add(StandardMaterial {
        base_color: Color::srgb(0.28, 0.31, 0.35),
        perceptual_roughness: 0.92,
        ..default()
    });
    let accent = materials.add(StandardMaterial {
        base_color: Color::srgb(0.36, 0.42, 0.48),
        perceptual_roughness: 0.85,
        ..default()
    });
    let wall_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.46, 0.48, 0.54),
        perceptual_roughness: 0.82,
        ..default()
    });
    let ramp_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.34, 0.46, 0.42),
        perceptual_roughness: 0.86,
        ..default()
    });
    let crate_mat = materials.add(Color::srgb(0.58, 0.41, 0.24));

    // Floor + subtle reference grid strip.
    commands.spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 40.0),
        Mesh3d(meshes.add(Cuboid::new(40.0, 1.0, 40.0))),
        MeshMaterial3d(ground.clone()),
        Transform::from_xyz(0.0, -0.5, 0.0),
        Friction::new(0.92),
    ));
    // Brace wall in front of origin (-Z): front face at z = -1.42.
    commands.spawn((
        RigidBody::Static,
        Collider::cuboid(3.6, 2.6, 0.36),
        Mesh3d(meshes.add(Cuboid::new(3.6, 2.6, 0.36))),
        MeshMaterial3d(wall_mat),
        Transform::from_xyz(0.0, 1.3, -1.60),
        Friction::new(0.85),
    ));
    // Gentle 9° ascending ramp on the left (x = -2.8), flush at z = -0.16 rising toward -Z.
    let slope_angle = 9.0_f32.to_radians();
    let ramp_len = 5.5_f32;
    let ramp_thick = 0.4_f32;
    let ramp_rot = Quat::from_rotation_x(slope_angle);
    let ramp_n = ramp_rot * Vec3::Y;
    let ramp_tan = ramp_rot * Vec3::NEG_Z;
    let ramp_center =
        Vec3::new(-2.8, 0.0, -0.16) + ramp_tan * (ramp_len * 0.5) - ramp_n * (ramp_thick * 0.5);
    commands.spawn((
        RigidBody::Static,
        Collider::cuboid(2.4, ramp_thick, ramp_len),
        Mesh3d(meshes.add(Cuboid::new(2.4, ramp_thick, ramp_len))),
        MeshMaterial3d(ramp_mat),
        Transform::from_translation(ramp_center).with_rotation(ramp_rot),
        Friction::new(0.95),
    ));
    // Low trip curb off to the right (+X) and crates further back.
    commands.spawn((
        RigidBody::Static,
        Collider::cuboid(0.22, 0.16, 3.0),
        Mesh3d(meshes.add(Cuboid::new(0.22, 0.16, 3.0))),
        MeshMaterial3d(accent),
        Transform::from_xyz(1.85, 0.08, 0.2),
        Friction::new(0.9),
    ));
    for (i, p) in [Vec3::new(1.8, 0.2, 2.2), Vec3::new(-1.3, 0.15, 2.6)].into_iter().enumerate() {
        let size = 0.4 - i as f32 * 0.1;
        commands.spawn((
            RigidBody::Dynamic,
            Collider::cuboid(size, size, size),
            ColliderDensity(300.0),
            Mesh3d(meshes.add(Cuboid::new(size, size, size))),
            MeshMaterial3d(crate_mat.clone()),
            Transform::from_translation(p),
        ));
    }

    if !showcase.active {
        spawn_hero(&mut commands, Vec3::ZERO, PuppetConfig::default());
    }

    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.5, 9.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        PointLight {
            intensity: 35_000.0,
            range: 20.0,
            color: Color::srgb(0.85, 0.92, 1.0),
            ..default()
        },
        Transform::from_xyz(-3.5, 3.2, -2.0),
    ));
    commands.spawn((Camera3d::default(), Transform::from_xyz(2.5, 1.8, -3.2)));

    // Demo stage banner at top-left.
    commands.spawn((
        DemoBanner,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(22.0),
            ..default()
        },
        TextColor(Color::srgb(0.98, 0.88, 0.45)),
        Node {
            position_type: PositionType::Absolute,
            left: px(14.0),
            top: px(10.0),
            ..default()
        },
    ));
    // Detailed telemetry + controls HUD below banner.
    commands.spawn((
        Hud,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.90, 0.93, 0.96)),
        Node {
            position_type: PositionType::Absolute,
            left: px(14.0),
            top: px(70.0),
            ..default()
        },
    ));
}

/// Capsules on the bones once the rig exists, with distinct limb colors and a visor on the head.
fn dress_puppets(
    mut commands: Commands,
    rigs: Query<&PuppetRig, Added<PuppetRig>>,
    colliders: Query<&Collider>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for rig in &rigs {
        let head_mat = materials.add(Color::srgb(0.90, 0.76, 0.62));
        let visor_mat = materials.add(Color::srgb(0.14, 0.72, 0.88));
        let torso_mat = materials.add(Color::srgb(0.24, 0.42, 0.68));
        let arm_mat = materials.add(Color::srgb(0.78, 0.62, 0.48));
        let hand_mat = materials.add(Color::srgb(0.92, 0.48, 0.25));
        let leg_mat = materials.add(Color::srgb(0.22, 0.25, 0.34));
        let foot_mat = materials.add(Color::srgb(0.38, 0.26, 0.18));

        for r in &rig.parts {
            for (bone, _) in &r.path {
                commands.entity(*bone).insert(Visibility::default());
            }
        }
        for part in PuppetPart::ALL {
            let r = rig.part(part);
            let mat = match part {
                PuppetPart::Head => head_mat.clone(),
                PuppetPart::Pelvis | PuppetPart::SpineLower | PuppetPart::SpineUpper => {
                    torso_mat.clone()
                }
                PuppetPart::HandL | PuppetPart::HandR => hand_mat.clone(),
                PuppetPart::UpperArmL
                | PuppetPart::LowerArmL
                | PuppetPart::UpperArmR
                | PuppetPart::LowerArmR => arm_mat.clone(),
                PuppetPart::FootL | PuppetPart::FootR => foot_mat.clone(),
                _ => leg_mat.clone(),
            };
            let Ok(collider) = colliders.get(r.body) else { continue };
            let Some(compound) = collider.shape().as_compound() else { continue };
            for (pose, shape) in compound.shapes() {
                let mesh = if let Some(capsule) = shape.as_capsule() {
                    meshes.add(Capsule3d::new(capsule.radius, capsule.segment.length()))
                } else if let Some(cuboid) = shape.as_cuboid() {
                    meshes.add(Cuboid::from_size(2.0 * cuboid.half_extents))
                } else {
                    continue;
                };
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(mat.clone()),
                    Transform::from_translation(pose.translation.into()).with_rotation(pose.rotation),
                    ChildOf(r.bone),
                ));
            }
            // Add a cyan visor on the front of the Head so gaze & chin-tuck are super obvious!
            if part == PuppetPart::Head {
                commands.spawn((
                    Mesh3d(meshes.add(Cuboid::new(0.14, 0.045, 0.05))),
                    MeshMaterial3d(visor_mat.clone()),
                    Transform::from_xyz(0.0, 0.10, -0.095),
                    ChildOf(r.bone),
                ));
            }
        }
    }
}

/// Automated showcase director that resets the puppet cleanly between stages and fires each behaviour.
#[allow(clippy::type_complexity)]
fn demo_director(
    time: Res<Time>,
    mut commands: Commands,
    mut showcase: ResMut<DemoShowcase>,
    mut orbit: ResMut<Orbit>,
    existing: Query<Entity, With<Hero>>,
    ready_hero: Query<(Entity, &PuppetSense), (With<Hero>, With<PuppetRig>)>,
) {
    if !showcase.active {
        return;
    }
    let stage = DEMO_STAGES[showcase.index];

    if showcase.needs_reset {
        for e in &existing {
            commands.entity(e).despawn();
        }
        let mut cfg = PuppetConfig::default();
        match stage.action {
            StageAction::BackwardFallGetUp | StageAction::HighDrop => {
                cfg.getup_delay = 0.55;
                cfg.physical_getup = true;
            }
            StageAction::SidewaysFall => {
                cfg.getup_delay = 1.4;
                cfg.writhe_when_down = true;
            }
            StageAction::ChestSlashWound => {
                cfg.partial_hit_impulse = f32::INFINITY;
                cfg.reach_for_wound = true;
            }
            StageAction::DyingCollapse => {
                cfg.dying_collapse_time = 0.38;
            }
            _ => {}
        }
        spawn_hero(&mut commands, stage.start_pos, cfg);
        orbit.target_yaw = Some(stage.camera_yaw);
        orbit.dist = stage.camera_dist;
        showcase.needs_reset = false;
        showcase.elapsed = 0.0;
        showcase.fired_mask = 0;
        return;
    }

    let Ok((e, sense)) = ready_hero.single() else {
        return;
    };
    showcase.elapsed += time.delta_secs();
    let t = showcase.elapsed;
    let back = -sense.heading;
    let fwd = sense.heading;
    let right = fwd.cross(Vec3::Y).normalize_or(Vec3::X);
    let mut ec = commands.entity(e);

    match stage.action {
        StageAction::Bump => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                let sh = sense.body(PuppetPart::UpperArmR).pos;
                let bumper = sh + right * 0.85 + fwd * 0.45;
                ec.puppet_bump(
                    PuppetPart::UpperArmR,
                    (-right * 0.72 + back * 0.68).normalize() * 14.0,
                    Some(bumper),
                );
            }
        }
        StageAction::GutHit => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                let gut = sense.body(PuppetPart::SpineLower).pos;
                ec.puppet_hit_kind(
                    PuppetPart::SpineLower,
                    back * 22.0,
                    Some(gut + fwd * 0.10),
                    HitKind::Blunt,
                );
            }
        }
        StageAction::ChestSlashWound => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                let sh = sense.body(PuppetPart::UpperArmL).pos;
                ec.puppet_hit_kind(
                    PuppetPart::UpperArmL,
                    (right * 0.75 + back * 0.65).normalize() * 18.0,
                    Some(sh + fwd * 0.06),
                    HitKind::Slash,
                );
            }
        }
        StageAction::HeadHit => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                let head = sense.body(PuppetPart::Head).pos;
                ec.puppet_hit_kind(
                    PuppetPart::Head,
                    (back * 0.85 + right * 0.45).normalize() * 26.0,
                    Some(head + fwd * 0.09 - right * 0.05),
                    HitKind::Blunt,
                );
            }
        }
        StageAction::ComboHits => {
            for (bit, fire_t, side_s) in [(1u8, 0.45, 1.0_f32), (2, 0.70, -1.0), (4, 0.95, 1.0)] {
                if t >= fire_t && (showcase.fired_mask & bit) == 0 {
                    showcase.fired_mask |= bit;
                    let chest = sense.body(PuppetPart::SpineUpper).pos;
                    ec.puppet_hit_kind(
                        PuppetPart::SpineUpper,
                        (back * 0.88 + right * (side_s * 0.45)).normalize() * 21.0,
                        Some(chest + fwd * 0.10 - right * (side_s * 0.08)),
                        HitKind::Slash,
                    );
                }
            }
        }
        StageAction::LegSweep => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_hit_kind(PuppetPart::CalfL, back * 24.0, None, HitKind::Blunt);
            }
        }
        StageAction::StrongPushBalance => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_push(back * 65.0);
            }
        }
        StageAction::RampStep => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_push(fwd * 58.0);
            }
        }
        StageAction::Trip => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_trip(Side::Right, fwd * 38.0);
            }
        }
        StageAction::WallBrace => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_push(fwd * 78.0);
            }
        }
        StageAction::SidewaysFall => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_push(right * 175.0);
                ec.puppet(PuppetCommand::Fall);
            }
        }
        StageAction::BackwardFallGetUp => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_push(back * 175.0);
                ec.puppet(PuppetCommand::Fall);
            }
        }
        StageAction::HighDrop => {
            if t >= 0.12 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet(PuppetCommand::Activate);
            }
        }
        StageAction::DyingCollapse => {
            if t >= 0.45 && (showcase.fired_mask & 1) == 0 {
                showcase.fired_mask |= 1;
                ec.puppet_hit_kind(PuppetPart::SpineUpper, back * 18.0, None, HitKind::Blunt);
                ec.puppet_kill();
            }
        }
    }

    if t >= stage.duration {
        let next = (showcase.index + 1) % DEMO_STAGES.len();
        showcase.jump(next);
    }
}

#[allow(clippy::type_complexity)]
fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut showcase: ResMut<DemoShowcase>,
    hero: Query<(Entity, &PuppetSense, &PuppetRig), With<Hero>>,
    mut positions: Query<&mut Position>,
    mut configs: Query<&mut PuppetConfig, With<Hero>>,
    mut debug: ResMut<PuppetDebug>,
) {
    if keys.just_pressed(KeyCode::Space) {
        showcase.active = !showcase.active;
        if showcase.active {
            let idx = showcase.index;
            showcase.jump(idx);
        }
    }
    if keys.just_pressed(KeyCode::KeyN) {
        showcase.active = true;
        let next = (showcase.index + 1) % DEMO_STAGES.len();
        showcase.jump(next);
        return;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        showcase.active = true;
        let prev = (showcase.index + DEMO_STAGES.len() - 1) % DEMO_STAGES.len();
        showcase.jump(prev);
        return;
    }
    if keys.just_pressed(KeyCode::KeyG) {
        debug.enabled = !debug.enabled;
    }

    let Ok((e, sense, rig)) = hero.single() else { return };
    let back = -sense.heading;
    let side = sense.heading.cross(Vec3::Y);
    let mut ec = commands.entity(e);
    if keys.just_pressed(KeyCode::Digit1) {
        showcase.active = false;
        ec.puppet_push(back * 25.0);
    }
    if keys.just_pressed(KeyCode::Digit2) {
        showcase.active = false;
        ec.puppet_push(back * 50.0);
    }
    if keys.just_pressed(KeyCode::Digit3) {
        showcase.active = false;
        ec.puppet_push(back * 120.0);
    }
    if keys.just_pressed(KeyCode::Digit4) {
        showcase.active = false;
        ec.puppet_push(-back * 85.0);
    }
    if keys.just_pressed(KeyCode::Digit5) {
        showcase.active = false;
        ec.puppet_push(side * 60.0);
    }
    if keys.just_pressed(KeyCode::KeyB) {
        showcase.active = false;
        let sh = sense.body(PuppetPart::UpperArmR).pos;
        let bumper = sh + side * 0.65 + sense.heading * 0.35;
        ec.puppet_bump(
            PuppetPart::UpperArmR,
            (-side * 0.75 + back * 0.65).normalize() * 14.0,
            Some(bumper),
        );
    }
    if keys.just_pressed(KeyCode::KeyH) {
        showcase.active = false;
        let head = sense.body(PuppetPart::Head).pos;
        ec.puppet_hit_kind(
            PuppetPart::Head,
            back * 32.0,
            Some(head + sense.heading * 0.1),
            HitKind::Blunt,
        );
    }
    if keys.just_pressed(KeyCode::KeyC) {
        showcase.active = false;
        let chest = sense.body(PuppetPart::SpineUpper).pos;
        ec.puppet_hit_kind(
            PuppetPart::SpineUpper,
            (back * 0.8 + side * 0.6).normalize() * 24.0,
            Some(chest + sense.heading * 0.12 - side * 0.10),
            HitKind::Slash,
        );
    }
    if keys.just_pressed(KeyCode::KeyU) {
        showcase.active = false;
        let gut = sense.body(PuppetPart::SpineLower).pos;
        ec.puppet_hit_kind(
            PuppetPart::SpineLower,
            back * 22.0,
            Some(gut + sense.heading * 0.10),
            HitKind::Blunt,
        );
    }
    if keys.just_pressed(KeyCode::KeyL) {
        showcase.active = false;
        ec.puppet_hit_kind(PuppetPart::CalfL, back * 24.0, None, HitKind::Blunt);
    }
    if keys.just_pressed(KeyCode::KeyT) {
        showcase.active = false;
        ec.puppet_trip(Side::Right, sense.heading * 36.0);
    }
    if keys.just_pressed(KeyCode::KeyJ) {
        showcase.active = false;
        for part in &rig.parts {
            if let Ok(mut pos) = positions.get_mut(part.body) {
                pos.0 += Vec3::Y * 3.0;
            }
        }
        ec.puppet(PuppetCommand::Activate);
    }
    if keys.just_pressed(KeyCode::KeyA) {
        ec.puppet(PuppetCommand::Activate);
    }
    if keys.just_pressed(KeyCode::KeyF) {
        ec.puppet(PuppetCommand::Fall);
    }
    if keys.just_pressed(KeyCode::KeyK) {
        ec.puppet_kill();
    }
    if keys.just_pressed(KeyCode::KeyR) {
        ec.puppet(PuppetCommand::Revive);
    }
    if keys.just_pressed(KeyCode::KeyW)
        && let Ok(mut c) = configs.single_mut()
    {
        c.writhe_when_down = !c.writhe_when_down;
    }
}

fn record_events(
    time: Res<Time>,
    mut events: MessageReader<PuppetEvent>,
    mut feed: ResMut<EventFeed>,
) {
    let now = time.elapsed_secs();
    for ev in events.read() {
        let msg = match ev.kind {
            PuppetEventKind::ModeChanged { from, to } => format!("Mode {from:?} -> {to:?}"),
            PuppetEventKind::BodyImpact { part, impulse, speed, .. } => {
                format!("BodyImpact {part:?} ({impulse:.0} N.s, {speed:.1} m/s)")
            }
            PuppetEventKind::WallBraced { part, .. } => format!("WallBraced ({part:?})"),
            PuppetEventKind::StepTaken { side, .. } => format!("StepTaken ({side:?})"),
            PuppetEventKind::Tripped { side } => format!("Tripped ({side:?})"),
            PuppetEventKind::GettingUp { face_up } => {
                format!("GettingUp ({})", if face_up { "supine" } else { "prone" })
            }
            PuppetEventKind::RootMoved => continue,
        };
        feed.lines.push((now, msg));
    }
    feed.lines.retain(|(t, _)| now - *t < 3.2);
    if feed.lines.len() > 4 {
        let excess = feed.lines.len() - 4;
        feed.lines.drain(0..excess);
    }
}

fn orbit_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut orbit: ResMut<Orbit>,
    hero: Query<&PuppetSense, (With<Hero>, With<PuppetRig>)>,
    mut cam: Query<&mut Transform, With<Camera3d>>,
) {
    if keys.pressed(KeyCode::ArrowLeft) {
        orbit.target_yaw = None;
        orbit.yaw -= time.delta_secs() * 1.6;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        orbit.target_yaw = None;
        orbit.yaw += time.delta_secs() * 1.6;
    }
    if let Some(target) = orbit.target_yaw {
        let diff = (target - orbit.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        orbit.yaw += diff * (time.delta_secs() * 4.0).min(1.0);
    }
    let focus = hero
        .single()
        .map(|s| Vec3::new(s.com.x, s.com.y.clamp(0.45, 3.8), s.com.z))
        .unwrap_or(Vec3::new(0.0, 0.9, 0.0));
    let Ok(mut t) = cam.single_mut() else { return };
    let d = orbit.dist;
    let offset = Vec3::new(orbit.yaw.sin() * d, 0.95 + 0.12 * d, orbit.yaw.cos() * d);
    let want = Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y);
    t.translation = t.translation.lerp(want.translation, 0.10);
    t.rotation = t.rotation.slerp(want.rotation, 0.12);
}

#[allow(clippy::type_complexity)]
fn hud(
    showcase: Res<DemoShowcase>,
    feed: Res<EventFeed>,
    hero: Query<
        (&PuppetState, &PuppetSense, &PuppetBehaviours, &PuppetNeuroState, &PuppetConfig),
        With<Hero>,
    >,
    mut banner: Query<&mut Text, (With<DemoBanner>, Without<Hud>)>,
    mut text: Query<&mut Text, (With<Hud>, Without<DemoBanner>)>,
) {
    if let Ok(mut b) = banner.single_mut() {
        let stage = DEMO_STAGES[showcase.index];
        if showcase.active {
            b.0 = format!(
                "[{}/{}] {}\n{}",
                showcase.index + 1,
                DEMO_STAGES.len(),
                stage.title,
                stage.subtitle,
            );
        } else {
            b.0 = "Modo Interativo Manual  (pressione Espaco para retomar o Tour Automatico)"
                .to_string();
        }
    }
    let (Ok((state, sense, behaviours, neuro, config)), Ok(mut text)) =
        (hero.single(), text.single_mut())
    else {
        return;
    };
    let wall_str = sense
        .nearby_wall
        .map(|w| format!("{:.2}m", w.distance))
        .unwrap_or_else(|| "none".into());
    let events_str = if feed.lines.is_empty() {
        "-".to_string()
    } else {
        feed.lines.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("  |  ")
    };
    text.0 = format!(
        "mode {:?} ({:.1}s)   behaviours {:?}\n\
         feet {}   margin {:+.2}m   tilt {:.0}deg   clear {:.2}m   wall {}   trauma {:.2}   combo {}   writhe {}\n\
         events: {}\n\n\
         Space demo on/off   N/P prox/ant cena   G gizmos   <-/-> orbitar camera\n\
         Manual: 1/2/3 empurrao  4 parede  5 lado  B esbarrao  H/C/U/L cabeca/peito/gut/perna  T tropeco  J queda alta  K morte  R levantar",
        state.mode,
        state.time_in_mode,
        behaviours.names(),
        sense.feet_down(),
        sense.capture_margin.min(9.99),
        sense.trunk_tilt.to_degrees(),
        sense.ground_clearance,
        wall_str,
        neuro.trauma,
        neuro.hit_combo,
        config.writhe_when_down,
        events_str,
    );
}
