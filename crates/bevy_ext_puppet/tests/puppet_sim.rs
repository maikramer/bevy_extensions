//! Headless simulations of the test skeleton on flat ground, at a fixed 60 Hz.
#![allow(clippy::field_reassign_with_default)]

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ext_puppet::prelude::*;
use bevy_ext_puppet::skeleton::spawn_test_skeleton_world;

const HZ: f64 = 60.0;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        PhysicsPlugins::default(),
        PuppetPlugin::default(),
    ));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / HZ)));
    app.insert_resource(Time::<Fixed>::from_hz(HZ));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(60.0, 1.0, 60.0),
        Transform::from_xyz(0.0, -0.5, 0.0),
        Friction::new(0.9),
    ));
    if let Some(n) = std::env::var("PUPPET_SUBSTEPS").ok().and_then(|v| v.parse().ok()) {
        app.insert_resource(SubstepCount(n));
    }
    app.finish();
    app.cleanup();
    app
}

fn spawn_puppet(app: &mut App, config: PuppetConfig) -> Entity {
    let root = app
        .world_mut()
        .spawn((Puppet, config, Transform::default()))
        .id();
    spawn_test_skeleton_world(app.world_mut(), root);
    root
}

fn run(app: &mut App, secs: f32) {
    for _ in 0..(secs * HZ as f32).round() as usize {
        app.update();
    }
}

fn sense(app: &App, e: Entity) -> PuppetSense {
    app.world().get::<PuppetSense>(e).expect("sense").clone()
}

fn mode(app: &App, e: Entity) -> PuppetMode {
    app.world().get::<PuppetState>(e).expect("state").mode
}

/// Runs `secs`, collecting the modes visited.
fn run_modes(app: &mut App, e: Entity, secs: f32) -> Vec<PuppetMode> {
    let mut modes = vec![mode(app, e)];
    for _ in 0..(secs * HZ as f32).round() as usize {
        app.update();
        let m = mode(app, e);
        if modes.last() != Some(&m) {
            modes.push(m);
        }
    }
    modes
}

fn command(app: &mut App, e: Entity, c: PuppetCommand) {
    app.world_mut()
        .get_mut::<bevy_ext_puppet::state::PuppetInbox>(e)
        .expect("inbox")
        .0
        .push(c);
}

#[test]
fn rig_builds_sixteen_bodies_with_the_body_mass() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.1);
    let rig = app.world().get::<PuppetRig>(e).expect("rig built").clone();
    assert_eq!(rig.parts.len(), 16);
    assert_eq!(rig.parts.iter().filter(|p| p.joint.is_some()).count(), 15);
    assert!((rig.height - 1.8).abs() < 0.1, "height {}", rig.height);
    let s = sense(&app, e);
    assert!((s.total_mass - 70.0).abs() < 1.0, "mass {}", s.total_mass);
}

#[test]
fn animated_puppet_holds_the_pose() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 3.0);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
    let s = sense(&app, e);
    assert_eq!(mode(&app, e), PuppetMode::Animated);
    assert!(
        (s.pelvis_height - rig.pelvis_height).abs() < 0.05,
        "pelvis {} vs {}",
        s.pelvis_height,
        rig.pelvis_height
    );
    assert_eq!(s.feet_down(), 2, "both feet on the ground");
}

#[test]
fn active_puppet_balances_unassisted() {
    let mut app = app();
    let cfg = PuppetConfig {
        upright_assist: 0.0,
        balance_assist: 0.0,
        ..default()
    };
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.5);
    command(&mut app, e, PuppetCommand::Activate);
    let mut min_pelvis = f32::INFINITY;
    let mut modes = vec![];
    for _ in 0..(6.0 * HZ) as usize {
        app.update();
        let s = sense(&app, e);
        min_pelvis = min_pelvis.min(s.pelvis_height);
        let m = mode(&app, e);
        if modes.last() != Some(&m) {
            modes.push(m);
        }
    }
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
    assert!(!modes.contains(&PuppetMode::Falling), "fell: {modes:?}");
    assert!(min_pelvis > 0.8 * rig.pelvis_height, "sank to {min_pelvis}");
}

/// Pushes up to ~0.45 m/s are absorbed by the planted feet, the hips and
/// the arms without stepping.
#[test]
fn small_push_is_recovered() {
    for push in [Vec3::new(0.0, 0.0, 25.0), Vec3::new(0.0, 0.0, -25.0), Vec3::new(25.0, 0.0, 0.0)] {
        let mut app = app();
        let e = spawn_puppet(&mut app, PuppetConfig::default());
        run(&mut app, 0.5);
        command(&mut app, e, PuppetCommand::Push { impulse: push });
        let modes = run_modes(&mut app, e, 5.0);
        assert!(!modes.contains(&PuppetMode::Falling), "{push}: fell: {modes:?}");
        assert_eq!(mode(&app, e), PuppetMode::Animated, "{push}: {modes:?}");
    }
}

/// A push the feet cannot absorb makes the puppet step.
#[test]
fn medium_push_takes_a_step() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.5);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
    let feet = |app: &App| {
        [PuppetPart::FootL, PuppetPart::FootR]
            .map(|f| app.world().get::<Position>(rig.part(f).body).unwrap().0)
    };
    let start = feet(&app);
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, 50.0) });
    let mut lifted = false;
    let mut moved = 0.0f32;
    for _ in 0..(1.0 * HZ) as usize {
        app.update();
        let s = sense(&app, e);
        lifted |= s.feet_down() < 2 && mode(&app, e) == PuppetMode::Active;
        let now = feet(&app);
        for k in 0..2 {
            moved = moved.max((now[k] - start[k]).with_y(0.0).length());
        }
    }
    assert!(lifted, "no foot left the ground");
    assert!(moved > 0.15, "the feet barely moved: {moved}");
}

#[test]
fn big_push_falls_and_lands() {
    let mut app = app();
    let mut cfg = PuppetConfig::default();
    cfg.getup_delay = -1.0;
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.5);
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, 300.0) });
    let modes = run_modes(&mut app, e, 6.0);
    assert!(modes.contains(&PuppetMode::Falling), "{modes:?}");
    assert_eq!(mode(&app, e), PuppetMode::Down, "{modes:?}");
}

#[test]
fn forward_fall_lands_on_the_hands_first() {
    let mut app = app();
    let mut cfg = PuppetConfig::default();
    cfg.getup_delay = -1.0;
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.5);
    // Forward is -Z.
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, -250.0) });
    command(&mut app, e, PuppetCommand::Fall);
    let (mut hands, mut head) = (None, None);
    for f in 0..(4.0 * HZ) as usize {
        app.update();
        let s = sense(&app, e);
        if hands.is_none()
            && (s.touching(PuppetPart::HandL)
                || s.touching(PuppetPart::HandR)
                || s.touching(PuppetPart::LowerArmL)
                || s.touching(PuppetPart::LowerArmR))
        {
            hands = Some(f);
        }
        if head.is_none() && s.touching(PuppetPart::Head) {
            head = Some(f);
        }
    }
    let hands = hands.expect("the hands touched the ground");
    assert!(head.is_none_or(|h| hands < h), "hands {hands} head {head:?}");
}

#[test]
fn killed_puppet_goes_limp_and_lies_down() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.5);
    command(&mut app, e, PuppetCommand::Kill);
    run(&mut app, 4.0);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
    let s = sense(&app, e);
    assert_eq!(mode(&app, e), PuppetMode::Dead);
    assert!(s.pelvis_height < 0.5 * rig.pelvis_height, "still up: {}", s.pelvis_height);
}

#[test]
fn fallen_puppet_gets_up() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.5);
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, 300.0) });
    let modes = run_modes(&mut app, e, 10.0);
    assert!(modes.contains(&PuppetMode::Down), "{modes:?}");
    assert!(modes.contains(&PuppetMode::GettingUp), "{modes:?}");
    assert_eq!(mode(&app, e), PuppetMode::Animated, "{modes:?}");
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
    let s = sense(&app, e);
    assert!((s.pelvis_height - rig.pelvis_height).abs() < 0.12, "pelvis {}", s.pelvis_height);
}

#[test]
fn bones_follow_the_bodies_when_active() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.5);
    command(&mut app, e, PuppetCommand::Kill);
    run(&mut app, 2.0);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
    // After propagation the pelvis bone sits on the pelvis body.
    let body = app.world().get::<Position>(rig.part(PuppetPart::Pelvis).body).unwrap().0;
    let bone = app.world().get::<GlobalTransform>(rig.part(PuppetPart::Pelvis).bone).unwrap().translation();
    assert!((body - bone).length() < 0.05, "bone {bone:?} body {body:?}");
}

#[test]
fn strong_push_recovers_with_stepping_and_vmc() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.5);
    // 70 N·s (~1.0 m/s on a 70 kg puppet) requires IPM stepping + VMC braking.
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, 70.0) });
    let mut stepped = false;
    for _ in 0..(4.5 * HZ) as usize {
        app.update();
        let s = sense(&app, e);
        if s.feet_down() < 2 {
            stepped = true;
        }
        if mode(&app, e) == PuppetMode::Animated {
            break;
        }
    }
    assert!(stepped, "expected a recovery step on a 70 N·s push");
    assert_eq!(mode(&app, e), PuppetMode::Animated, "puppet should recover upright after 70 N·s push");
}

#[test]
fn multidirectional_pushes_recover() {
    let cases = [
        ("back +Z", Vec3::new(0.0, 0.0, 45.0)),
        ("front -Z", Vec3::new(0.0, 0.0, -45.0)),
        ("right +X", Vec3::new(40.0, 0.0, 0.0)),
        ("left -X", Vec3::new(-40.0, 0.0, 0.0)),
    ];
    for (label, impulse) in cases {
        let mut app = app();
        let e = spawn_puppet(&mut app, PuppetConfig::default());
        run(&mut app, 0.5);
        command(&mut app, e, PuppetCommand::Push { impulse });
        run(&mut app, 4.5);
        assert_eq!(
            mode(&app, e),
            PuppetMode::Animated,
            "expected full recovery for {label} push ({impulse:?})"
        );
    }
}

#[test]
fn centroidal_sense_reports_momentum_stance_and_capture_region() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.5);
    command(&mut app, e, PuppetCommand::Activate);
    run(&mut app, 0.25);
    let s0 = sense(&app, e);
    assert_eq!(s0.stance, StanceState::Double);
    assert!(s0.capture_region_radius > 0.03, "flywheel capture radius {}", s0.capture_region_radius);
    assert!(s0.com_inertia.x_axis.x > 1.0 && s0.com_inertia.z_axis.z > 1.0);
    assert!(s0.foot_plant_quality(Side::Left) > 0.5);
    assert!(s0.foot_plant_quality(Side::Right) > 0.5);

    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, 50.0) });
    let mut peak_l = 0.0_f32;
    let mut saw_single_or_flight = false;
    for _ in 0..(1.2 * HZ) as usize {
        app.update();
        let s = sense(&app, e);
        peak_l = peak_l.max(s.angular_momentum.length());
        if matches!(s.stance, StanceState::Left | StanceState::Right | StanceState::Flight) {
            saw_single_or_flight = true;
        }
    }
    assert!(peak_l > 0.5, "expected non-trivial centroidal angular momentum, peak={peak_l}");
    assert!(saw_single_or_flight, "expected stance transition during 50 N·s recovery");
}

#[test]
fn lateral_limb_hit_recovers_cleanly() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.5);
    command(
        &mut app,
        e,
        PuppetCommand::Impulse {
            part: PuppetPart::UpperArmR,
            impulse: Vec3::new(-18.0, 0.0, 12.0),
            point: None,
        },
    );
    let modes = run_modes(&mut app, e, 4.0);
    assert!(!modes.contains(&PuppetMode::Falling), "moderate hit should not topple puppet: {modes:?}");
    assert_eq!(mode(&app, e), PuppetMode::Animated, "{modes:?}");
}

/// `cargo test --release --test puppet_sim trace -- --ignored --nocapture`
/// (`PUPPET_TRACE_PUSH=z` sets the push along +Z, 0 = just activate).
#[test]
#[ignore]
fn trace() {
    let mut app = app();
    let mut cfg = PuppetConfig::default();
    cfg.getup_delay = -1.0;
    cfg.auto_balance = std::env::var("PUPPET_TRACE_NOBAL").is_err();
    if let Some(f) = std::env::var("PUPPET_FOOT").ok().and_then(|v| v.parse().ok()) {
        cfg.foot_mass_scale = f;
    }
    if let Some(f) = std::env::var("PUPPET_STRENGTH").ok().and_then(|v| v.parse().ok()) {
        cfg.strength = f;
    }
    if let Some(a) = std::env::var("PUPPET_TRACE_ASSIST").ok().and_then(|v| v.parse().ok()) {
        cfg.upright_assist = a;
    }
    if let Some(a) = std::env::var("PUPPET_FRICTION").ok().and_then(|v| v.parse().ok()) {
        cfg.friction = a;
    }
    if let Some(a) = std::env::var("PUPPET_TRACE_STEPS").ok().and_then(|v| v.parse().ok()) {
        cfg.max_steps = a;
    }
    let e = spawn_puppet(&mut app, cfg);
    let idle = std::env::var("PUPPET_TRACE_IDLE").is_ok();
    if !idle {
        run(&mut app, 0.5);
    }
    let push: f32 = std::env::var("PUPPET_TRACE_PUSH").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let push_x: f32 = std::env::var("PUPPET_TRACE_PUSH_X").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    if idle {
    } else if push == 0.0 && push_x == 0.0 {
        command(&mut app, e, PuppetCommand::Activate);
    } else {
        command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(push_x, 0.0, push) });
    }
    let secs: f32 = std::env::var("PUPPET_TRACE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(3.0);
    for f in 0..(secs * HZ as f32) as usize {
        app.update();
        let every: usize = std::env::var("PUPPET_TRACE_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
        if f % every == 0 && app.world().get::<PuppetRig>(e).is_some() {
            let s = sense(&app, e);
            let b = app.world().get::<PuppetBehaviours>(e).unwrap().names();
            println!(
                "{f:4} {:?} pel={:.3} tilt={:.2} feet={} marg={:+.3} cp=({:+.2},{:+.2}) sup=({:+.2},{:+.2}) com=({:+.2},{:.2},{:+.2}) v=({:+.2},{:+.2},{:+.2}) {:?}",
                mode(&app, e), s.pelvis_height, s.trunk_tilt, s.feet_down(), s.capture_margin,
                s.capture_point.x, s.capture_point.y, s.support_centre.x, s.support_centre.y,
                s.com.x, s.com.y, s.com.z, s.com_vel.x, s.com_vel.y, s.com_vel.z, b
            );
            if std::env::var("PUPPET_TRACE_SEP").is_ok() {
                let rig = app.world().get::<PuppetRig>(e).unwrap();
                let mut line = String::from("      sep:");
                for part in PuppetPart::ALL.iter().skip(1) {
                    let p = part.parent().unwrap();
                    let anchor = s.body(p).pos + s.body(p).rot * rig.part(*part).anchor_in_parent;
                    line += &format!(" {:?}={:.3}", part, (s.body(*part).pos - anchor).length());
                }
                println!("{line}");
                let ys: Vec<String> = PuppetPart::ALL.iter().map(|p| format!("{:.2}", s.body(*p).com.y)).collect();
                println!("      y: {}", ys.join(" "));
                let tilt: Vec<String> = PuppetPart::ALL.iter().map(|p| {
                    let r = rig.part(*p);
                    format!("{:.2}", (s.body(*p).rot * r.up_local).angle_between(Vec3::Y))
                }).collect();
                println!("      tilt: {}", tilt.join(" "));
            }
            if std::env::var("PUPPET_TRACE_CONTACTS").is_ok() {
                let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
                let feet = [rig.part(PuppetPart::FootL).body, rig.part(PuppetPart::FootR).body];
                let mut sys = app.world_mut().register_system(move |c: Collisions| {
                    let mut out = String::from("      contacts:");
                    for f in feet {
                        for pair in c.collisions_with(f) {
                            for m in &pair.manifolds {
                                for p in &m.points {
                                    out += &format!(" ({:+.3},{:+.3},{:+.3} pen {:.3} imp {:.2})", p.point.x, p.point.y, p.point.z, p.penetration, p.normal_impulse);
                                }
                            }
                        }
                    }
                    println!("{out}");
                });
                let _ = app.world_mut().run_system(sys);
                let _ = &mut sys;
            }
            if std::env::var("PUPPET_TRACE_LEGS").is_ok() {
                let rig = app.world().get::<PuppetRig>(e).unwrap();
                let mut line = String::from("      legs:");
                for (name, th, ca, fo) in [("L", PuppetPart::ThighL, PuppetPart::CalfL, PuppetPart::FootL), ("R", PuppetPart::ThighR, PuppetPart::CalfR, PuppetPart::FootR)] {
                    let dt = s.body(th).rot * rig.part(th).axis_local;
                    let dc = s.body(ca).rot * rig.part(ca).axis_local;
                    let f = s.body(fo).pos;
                    line += &format!(" {name}: knee={:.2} foot=({:+.2},{:.2},{:+.2}) down={}", dt.angle_between(dc), f.x, f.y, f.z, s.touching(fo));
                }
                println!("{line}");
            }
            if std::env::var("PUPPET_TRACE_JOINTS").is_ok() {
                let t = app.world().get::<PuppetTargets>(e).unwrap();
                let mut line = String::from("      err:");
                for part in PuppetPart::ALL.iter().skip(1) {
                    let p = part.parent().unwrap();
                    let cur = s.body(p).rot.inverse() * s.body(*part).rot;
                    let err = cur.angle_between(t.local[part.index()]);
                    line += &format!(" {:?}={:.2}", part, err);
                }
                println!("{line}");
            }
        }
    }
}

fn joint_errors(app: &App, e: Entity) -> Vec<(PuppetPart, f32)> {
    let s = sense(app, e);
    let t = app.world().get::<PuppetTargets>(e).unwrap();
    PuppetPart::ALL
        .iter()
        .skip(1)
        .map(|part| {
            let p = part.parent().unwrap();
            let cur = s.body(p).rot.inverse() * s.body(*part).rot;
            (*part, cur.angle_between(t.local[part.index()]))
        })
        .collect()
}

#[derive(Component)]
struct ServoTarget(Quat);

fn servo(mut q: Query<(Forces, &ServoTarget)>) {
    for (mut f, t) in &mut q {
        let rot = f.rotation();
        let w = f.angular_velocity();
        let err = (t.0 * rot.inverse()).to_scaled_axis();
        f.apply_torque((err * 20.0 - w * 4.0) * 0.1);
    }
}

#[test]
fn single_body_servo_converges() {
    let mut app = app();
    app.insert_resource(Gravity(Vec3::ZERO));
    app.add_systems(FixedPostUpdate, servo.before(PhysicsSystems::StepSimulation).after(PhysicsSystems::Prepare));
    let target = Quat::from_rotation_z(0.5);
    let b = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.3),
            Mass(1.0),
            AngularInertia::new(Vec3::splat(0.1)),
            Transform::from_xyz(0.0, 5.0, 0.0),
            ServoTarget(target),
        ))
        .id();
    run(&mut app, 4.0);
    let r = app.world().get::<Rotation>(b).unwrap().0;
    assert!(r.angle_between(target) < 0.05, "angle {}", r.angle_between(target));
}

#[test]
fn muscle_joint_reaches_its_target() {
    use bevy_ext_puppet::drive::MuscleJoint;
    let mut app = app();
    app.insert_resource(Gravity(Vec3::ZERO));
    let parent = app
        .world_mut()
        .spawn((RigidBody::Static, Transform::from_xyz(0.0, 5.0, 0.0)))
        .id();
    let child = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::capsule(0.05, 0.4),
            Transform::from_xyz(0.0, 5.0, 0.0),
        ))
        .id();
    let target = Quat::from_rotation_z(0.5) * Quat::from_rotation_x(0.3);
    let mut m = MuscleJoint::new(parent, child);
    m.target = target;
    m.stiffness = 50.0;
    m.damping = 2.0;
    m.max_torque = 1000.0;
    app.world_mut().spawn(m);
    app.world_mut().spawn(SphericalJoint::new(parent, child));
    for f in 0..240 {
        app.update();
        if f % 40 == 0 {
            let r = app.world().get::<Rotation>(child).unwrap().0;
            eprintln!("{f} angle to target {:.3}", r.angle_between(target));
        }
    }
    let r = app.world().get::<Rotation>(child).unwrap().0;
    assert!(r.angle_between(target) < 0.02, "angle {}", r.angle_between(target));
}

#[test]
fn muscles_settle_after_a_kick_in_zero_gravity() {
    let mut app = app();
    app.insert_resource(Gravity(Vec3::ZERO));
    let ground: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<RigidBody>>()
        .iter(app.world())
        .collect();
    for g in ground {
        app.world_mut().despawn(g);
    }
    let mut cfg = PuppetConfig::default();
    cfg.auto_balance = false;
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.2);
    command(&mut app, e, PuppetCommand::Activate);
    command(
        &mut app,
        e,
        PuppetCommand::Impulse { part: PuppetPart::HandL, impulse: Vec3::new(0.0, 0.0, 2.0), point: None },
    );
    run(&mut app, 0.3);
    let sensed = sense(&app, e);
    assert_eq!(sensed.up, Vec3::Y);
    assert_eq!(sensed.g, 0.0);
    assert!(sensed.trunk_tilt.is_finite());
    let peak_errs = joint_errors(&app, e);
    let peak = peak_errs.iter().map(|(_, a)| *a).fold(0.0f32, f32::max);
    run(&mut app, 1.5);
    let errs = joint_errors(&app, e);
    let worst = errs.iter().map(|(_, a)| *a).fold(0.0f32, f32::max);
    assert!(peak < 1.0, "kick blew up: {peak} ({peak_errs:?})");
    assert!(worst < 0.05, "did not settle: {errs:?}");
}

#[test]
fn puppet_is_dismantled_when_idle_and_can_be_rebuilt() {
    use bevy_ext_puppet::lifecycle::PuppetIdleDismantle;
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    app.world_mut().entity_mut(e).insert(PuppetIdleDismantle::after(0.5));
    run(&mut app, 0.3);
    let bodies = |app: &mut App| app.world_mut().query::<&PuppetBody>().iter(app.world()).count();
    assert_eq!(bodies(&mut app), 16);
    run(&mut app, 1.0);
    assert_eq!(bodies(&mut app), 0, "idle puppet not dismantled");
    assert!(app.world().get::<PuppetRig>(e).is_none());
    // Hit again: rebuild on demand from the current pose and react.
    app.world_mut().entity_mut(e).insert(Puppet);
    run(&mut app, 0.1);
    assert_eq!(bodies(&mut app), 16);
    command(&mut app, e, PuppetCommand::Kill);
    run(&mut app, 3.0);
    assert_eq!(mode(&app, e), PuppetMode::Dead);
}

#[test]
fn pending_kill_survives_immediate_idle_dismantle() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    app.world_mut().entity_mut(e).insert(PuppetIdleDismantle::after(0.0));
    app.world_mut().commands().entity(e).puppet_kill();
    run(&mut app, 0.2);
    assert_eq!(mode(&app, e), PuppetMode::Dead);
}

#[test]
fn partial_pelvis_animation_preserves_the_authored_channels() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.1);
    let part = app.world().get::<PuppetRig>(e).unwrap().part(PuppetPart::Pelvis).clone();
    let rest = *app.world().get::<Transform>(part.bone).unwrap();
    command(&mut app, e, PuppetCommand::Kill);
    run(&mut app, 2.0);
    let fallen = *app.world().get::<Transform>(part.bone).unwrap();
    assert!(fallen.translation.distance(rest.translation) > 0.1);

    let rotation = Quat::from_rotation_y(0.3);
    app.world_mut().get_mut::<Transform>(part.bone).unwrap().rotation = rotation;
    app.update();
    let targets = app.world().get::<PuppetTargets>(e).unwrap();
    assert!(targets.pos(PuppetPart::Pelvis).distance(rest.translation) < 1e-4);
    assert!(targets.rot(PuppetPart::Pelvis).angle_between(rotation) < 1e-3);

    // A scale channel can also change between the last write and dismantling.
    let scale = Vec3::splat(1.01);
    app.world_mut().get_mut::<Transform>(part.bone).unwrap().scale = scale;
    bevy_ext_puppet::lifecycle::dismantle(app.world_mut(), e);
    let restored = app.world().get::<Transform>(part.bone).unwrap();
    assert_eq!(restored.translation, rest.translation);
    assert_eq!(restored.rotation, rotation);
    assert_eq!(restored.scale, scale);
}

#[test]
fn support_coordinates_round_trip_for_tilted_gravity() {
    let point = Vec3::new(1.2, -3.0, 2.5);
    for up in [Vec3::Y, Vec3::X, Vec3::NEG_Y, Vec3::Z, point.normalize()] {
        let sense = PuppetSense { up, ..default() };
        let flat = sense.flat(point);
        assert!(point.distance(sense.lift(flat, point.dot(up))) < 1e-5);
        assert!((flat.length() - point.reject_from(up).length()).abs() < 1e-5);
    }
}

#[test]
fn foot_weight_tuning_preserves_the_total_body_mass() {
    use bevy_ext_puppet::rig::part_mass_fraction;
    let nominal: f32 = PuppetPart::ALL.iter().map(|p| p.mass_fraction()).sum();
    for scale in [1.0, 4.0, 10.0, 100.0] {
        let sum: f32 = PuppetPart::ALL.iter().map(|p| part_mass_fraction(*p, scale)).sum();
        assert!((sum - nominal).abs() < 1e-5, "foot scale {scale} gives mass fraction {sum}");
    }
}

#[test]
fn sideways_gravity_supports_both_feet() {
    let mut app = app();
    let rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let up = rotation * Vec3::Y;
    app.insert_resource(Gravity(-up * 9.81));
    let mut ground = app.world_mut().query_filtered::<&mut Transform, With<Collider>>();
    let mut floor = ground.single_mut(app.world_mut()).unwrap();
    floor.translation = up * -0.5;
    floor.rotation = rotation;
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    app.world_mut().get_mut::<Transform>(e).unwrap().rotation = rotation;
    run(&mut app, 1.0);

    let s = sense(&app, e);
    assert_eq!(s.stance, StanceState::Double);
    assert!(s.capture_margin < -0.01, "margin {}", s.capture_margin);
    assert!(s.up.distance(up) < 1e-5);
    assert!(s.trunk_tilt < 0.1);
}

#[test]
fn skinned_hand_stays_above_a_raised_ramp_during_a_fall() {
    use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
    use bevy::mesh::{PrimitiveTopology, VertexAttributeValues};
    use bevy_ext_puppet::skeleton::TEST_SKELETON;

    let mut app = app();
    let slope = Quat::from_rotation_z(0.12);
    let up = slope * Vec3::Y;
    let surface = Vec3::Y * 2.0;
    let world = app.world_mut();
    let mut floors = world.query_filtered::<&mut Transform, With<Collider>>();
    *floors.single_mut(world).unwrap() =
        Transform::from_translation(surface - up * 0.5).with_rotation(slope);
    // A trigger just above the ramp must not hold up the fallen mesh.
    world.spawn((
        RigidBody::Static,
        Sensor,
        Collider::cuboid(8.0, 0.1, 8.0),
        Transform::from_translation(surface + up * 0.15).with_rotation(slope),
    ));
    let root = world.spawn((
        Puppet,
        PuppetConfig { getup_delay: -1.0, ..default() },
        Transform::from_translation(surface).with_rotation(slope),
    )).id();
    let bones = spawn_test_skeleton_world(world, root);
    let wrist = TEST_SKELETON.iter().position(|b| b.0 == "hand_l").unwrap();
    let bind = Mat4::from_translation(-TEST_SKELETON[wrist].2);
    // A bulky glove extends beyond the wrist capsule in every direction.
    let mut glove = Vec::new();
    for x in [-0.14, 0.14] {
        for y in [-0.22, 0.03] {
            for z in [-0.11, 0.11] {
                glove.push(Vec3::new(x, y, z));
            }
        }
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, glove.iter()
        .map(|p| (*p + TEST_SKELETON[wrist].2).to_array()).collect::<Vec<_>>());
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(vec![[0; 4]; glove.len()]));
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, vec![[1.0, 0.0, 0.0, 0.0]; glove.len()]);
    world.init_resource::<Assets<Mesh>>();
    world.init_resource::<Assets<SkinnedMeshInverseBindposes>>();
    let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let inverse_bindposes = world.resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
        .add(SkinnedMeshInverseBindposes::from(vec![bind]));
    world.spawn((Mesh3d(mesh), SkinnedMesh { joints: vec![bones[wrist]], inverse_bindposes }, ChildOf(root)));
    run(&mut app, 0.4);
    command(&mut app, root, PuppetCommand::Push { impulse: Vec3::NEG_Z * 200.0 });
    command(&mut app, root, PuppetCommand::Fall);
    let mut lowest = f32::INFINITY;
    for _ in 0..180 {
        app.update();
        let wrist_world = app.world().get::<GlobalTransform>(bones[wrist]).unwrap();
        for &p in &glove {
            lowest = lowest.min((wrist_world.transform_point(p) - surface).dot(up));
        }
    }
    assert!(lowest.abs() < 0.015, "lowest glove vertex relative to the ramp: {lowest}");
}

/// Helper: drains all `PuppetEvent` messages accumulated during simulation steps.
fn run_collecting_events(app: &mut App, secs: f32) -> Vec<PuppetEvent> {
    let mut out = Vec::new();
    let mut cursor = app.world().resource::<Messages<PuppetEvent>>().get_cursor();
    for _ in 0..(secs * HZ as f32).round() as usize {
        app.update();
        let msgs = app.world().resource::<Messages<PuppetEvent>>();
        out.extend(cursor.read(msgs).copied());
    }
    out
}

#[test]
fn bump_turns_torso_glances_and_recovers_cleanly() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.3);

    let bumper_pos = Vec3::new(0.85, 1.45, -0.45);
    command(
        &mut app,
        e,
        PuppetCommand::Bump {
            part: PuppetPart::UpperArmR,
            impulse: Vec3::new(-9.0, 0.0, 7.0),
            bumper_pos: Some(bumper_pos),
        },
    );

    let mut max_chest_yaw = 0.0_f32;
    let mut max_head_toward_right = 0.0_f32;
    let mut ever_fell = false;
    let mut gaze_set = false;
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();

    for _ in 0..90 {
        app.update();
        let s = sense(&app, e);
        let m = mode(&app, e);
        if matches!(m, PuppetMode::Falling | PuppetMode::Down) {
            ever_fell = true;
        }
        if let Some(neuro) = app.world().get::<PuppetNeuroState>(e)
            && neuro.active_gaze(s.time).is_some()
        {
            gaze_set = true;
        }
        let pelvis_fwd = (s.body(PuppetPart::Pelvis).rot * rig.part(PuppetPart::Pelvis).forward_local)
            .reject_from(s.up)
            .normalize_or(Vec3::NEG_Z);
        let chest_fwd = (s.body(PuppetPart::SpineUpper).rot * rig.part(PuppetPart::SpineUpper).forward_local)
            .reject_from(s.up)
            .normalize_or(Vec3::NEG_Z);
        let yaw = bevy_ext_puppet::rig::signed_angle(pelvis_fwd, chest_fwd, s.up).abs();
        max_chest_yaw = max_chest_yaw.max(yaw);

        let head_fwd = (s.body(PuppetPart::Head).rot * rig.part(PuppetPart::Head).forward_local)
            .reject_from(s.up)
            .normalize_or(Vec3::NEG_Z);
        // Bumper is on +X (right); head forward dot +X rises as puppet glances over.
        max_head_toward_right = max_head_toward_right.max(head_fwd.dot(Vec3::X));
    }

    assert!(!ever_fell, "glancing bump should not knock puppet down");
    assert!(gaze_set, "bump with bumper_pos should set orienting gaze");
    assert!(
        max_chest_yaw > 0.14,
        "expected axial torso shoulder-slip yaw > 0.14 rad, got {max_chest_yaw:.3}"
    );
    assert!(
        max_head_toward_right > 0.12,
        "expected head glance toward +X bumper, got dot={max_head_toward_right:.3}"
    );

    run(&mut app, 2.5);
    assert_eq!(mode(&app, e), PuppetMode::Animated, "bump should recover cleanly to Animated");
}

#[test]
fn gut_hit_folds_torso_forward_while_chest_hit_arches_back() {
    let measure_chest_pitch = |part: PuppetPart| -> (f32, f32) {
        let mut app = app();
        let cfg = PuppetConfig {
            partial_hit_impulse: f32::INFINITY,
            ..default()
        };
        let e = spawn_puppet(&mut app, cfg);
        run(&mut app, 0.3);
        let rig = app.world().get::<PuppetRig>(e).unwrap().clone();
        // Frontal hit pushing backward (+Z) on either abdomen (SpineLower) or chest (SpineUpper).
        command(
            &mut app,
            e,
            PuppetCommand::Hit {
                part,
                impulse: Vec3::new(0.0, 0.0, 14.0),
                point: None,
                kind: HitKind::Blunt,
            },
        );
        // Track peak forward fold (-Z) and peak backward arch (+Z) of SpineUpper relative to Pelvis.
        let mut max_forward_fold = 0.0_f32;
        let mut max_back_arch = 0.0_f32;
        for _ in 0..35 {
            app.update();
            let s = sense(&app, e);
            let pel_up = (s.body(PuppetPart::Pelvis).rot * rig.part(PuppetPart::Pelvis).up_local).normalize();
            let chest_up = (s.body(PuppetPart::SpineUpper).rot * rig.part(PuppetPart::SpineUpper).up_local).normalize();
            // Positive signed angle around +X rotates +Y → +Z (backward arch); negative → -Z (forward fold).
            let pitch = bevy_ext_puppet::rig::signed_angle(pel_up, chest_up, Vec3::X);
            max_forward_fold = max_forward_fold.max(-pitch);
            max_back_arch = max_back_arch.max(pitch);
        }
        (max_forward_fold, max_back_arch)
    };

    let (gut_fold, _gut_arch) = measure_chest_pitch(PuppetPart::SpineLower);
    let (chest_fold, chest_arch) = measure_chest_pitch(PuppetPart::SpineUpper);

    assert!(
        gut_fold > 0.22,
        "frontal gut hit should jackknife chest forward > 0.22 rad, got {gut_fold:.3}"
    );
    assert!(
        chest_arch > 0.14,
        "frontal chest hit should arch upper torso backward > 0.14 rad, got {chest_arch:.3}"
    );
    assert!(
        gut_fold > chest_fold + 0.12,
        "gut hit fold ({gut_fold:.3}) should significantly exceed chest hit forward fold ({chest_fold:.3})"
    );
}

#[test]
fn leg_sweep_hit_buckles_knee_and_destabilizes() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.3);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();

    command(
        &mut app,
        e,
        PuppetCommand::Hit {
            part: PuppetPart::CalfL,
            impulse: Vec3::new(0.0, 0.0, 22.0),
            point: None,
            kind: HitKind::Blunt,
        },
    );

    let mut max_knee_l = 0.0_f32;
    let mut left_animated = false;
    for _ in 0..45 {
        app.update();
        let s = sense(&app, e);
        if mode(&app, e) != PuppetMode::Animated {
            left_animated = true;
        }
        let dt = s.body(PuppetPart::ThighL).rot * rig.part(PuppetPart::ThighL).axis_local;
        let dc = s.body(PuppetPart::CalfL).rot * rig.part(PuppetPart::CalfL).axis_local;
        max_knee_l = max_knee_l.max(dt.angle_between(dc));
    }

    assert!(
        max_knee_l > 0.55,
        "leg sweep should sharply buckle struck knee > 0.55 rad, got {max_knee_l:.3}"
    );
    assert!(left_animated, "heavy calf sweep should destabilize stance out of Animated");
}

#[test]
fn rapid_combo_hits_accumulate_trauma_and_escalate_to_stagger() {
    // Verify a single sub-threshold hit stays in Animated (partial flinch only).
    {
        let mut app = app();
        let e = spawn_puppet(&mut app, PuppetConfig::default());
        run(&mut app, 0.3);
        command(
            &mut app,
            e,
            PuppetCommand::Hit {
                part: PuppetPart::SpineUpper,
                impulse: Vec3::new(0.0, 0.0, 18.0),
                point: None,
                kind: HitKind::Slash,
            },
        );
        run(&mut app, 0.25);
        assert_eq!(mode(&app, e), PuppetMode::Animated, "single 18 N·s hit should remain partial");
    }

    // Now deliver 3 rapid sub-threshold hits: trauma compounds and breaks stance into Active!
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.3);
    let mut peak_trauma = 0.0_f32;
    let mut peak_combo = 0u32;
    let mut escalated = false;

    for hit_idx in 0..4 {
        let sign = if hit_idx % 2 == 0 { 1.0 } else { -1.0 };
        command(
            &mut app,
            e,
            PuppetCommand::Hit {
                part: PuppetPart::SpineUpper,
                impulse: Vec3::new(sign * 4.0, 0.0, 19.0),
                point: None,
                kind: HitKind::Blunt,
            },
        );
        for _ in 0..10 {
            app.update();
            if let Some(neuro) = app.world().get::<PuppetNeuroState>(e) {
                peak_trauma = peak_trauma.max(neuro.trauma);
                peak_combo = peak_combo.max(neuro.hit_combo);
            }
            if mode(&app, e) != PuppetMode::Animated {
                escalated = true;
            }
        }
    }

    assert!(peak_combo >= 3, "expected hit_combo >= 3, got {peak_combo}");
    assert!(peak_trauma > 0.65, "expected compounded trauma > 0.65, got {peak_trauma:.3}");
    assert!(escalated, "rapid sub-threshold combo should escalate to Active stagger");
}

#[test]
fn wall_brace_catches_shove_against_nearby_wall() {
    let mut app = app();
    // Place a tall static wall 0.78 m in front of the puppet (-Z).
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(6.0, 3.0, 0.4),
        Transform::from_xyz(0.0, 1.5, -0.98), // front face at z = -0.78
        Friction::new(0.85),
    ));
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.3);

    // Shove puppet forward into the wall!
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, -78.0) });

    let mut saw_wall_probe = false;
    let mut saw_wall_brace_beh = false;
    let mut saw_wall_braced_event = false;
    let mut min_head_z = f32::INFINITY;
    let mut cursor = app.world().resource::<Messages<PuppetEvent>>().get_cursor();

    for _ in 0..150 {
        app.update();
        let s = sense(&app, e);
        if s.nearby_wall.is_some() {
            saw_wall_probe = true;
        }
        if let Some(beh) = app.world().get::<PuppetBehaviours>(e)
            && beh.has(WallBrace::NAME)
        {
            saw_wall_brace_beh = true;
        }
        min_head_z = min_head_z.min(s.body(PuppetPart::Head).pos.z);
        let msgs = app.world().resource::<Messages<PuppetEvent>>();
        for ev in cursor.read(msgs) {
            if ev.puppet == e && matches!(ev.kind, PuppetEventKind::WallBraced { .. }) {
                saw_wall_braced_event = true;
            }
        }
    }

    assert!(saw_wall_probe, "spatial sense should detect the approaching wall");
    assert!(saw_wall_brace_beh, "WallBrace behaviour should activate automatically near wall");
    assert!(saw_wall_braced_event, "hands should contact wall and emit WallBraced event");
    assert!(
        min_head_z > -0.76,
        "WallBrace should keep skull from smashing through wall face (-0.78), min_head_z={min_head_z:.3}"
    );
}

#[test]
fn high_drop_flails_in_air_then_catches_fall_and_emits_body_impact() {
    let mut app = app();
    let cfg = PuppetConfig {
        getup_delay: -1.0,
        ..default()
    };
    let root = app
        .world_mut()
        .spawn((Puppet, cfg, Transform::from_xyz(0.0, 3.8, 0.0)))
        .id();
    spawn_test_skeleton_world(app.world_mut(), root);
    run(&mut app, 0.08);
    command(&mut app, root, PuppetCommand::Activate);

    let mut max_clearance = 0.0_f32;
    let mut saw_high_fall = false;
    let mut saw_falling_mode = false;
    let mut impact_events = Vec::new();
    let mut cursor = app.world().resource::<Messages<PuppetEvent>>().get_cursor();
    let rig = app.world().get::<PuppetRig>(root).unwrap().clone();

    for _ in 0..240 {
        app.update();
        let s = sense(&app, root);
        max_clearance = max_clearance.max(s.ground_clearance);
        if s.is_high_fall(rig.height) {
            saw_high_fall = true;
        }
        if mode(&app, root) == PuppetMode::Falling {
            saw_falling_mode = true;
        }
        let msgs = app.world().resource::<Messages<PuppetEvent>>();
        for ev in cursor.read(msgs) {
            if ev.puppet == root
                && let PuppetEventKind::BodyImpact { part, impulse, speed, .. } = ev.kind
            {
                impact_events.push((part, impulse, speed));
            }
        }
    }

    assert!(max_clearance > 2.2, "expected high ground clearance > 2.2 m, got {max_clearance:.2}");
    assert!(saw_high_fall, "sense.is_high_fall() should trigger during high drop");
    assert!(saw_falling_mode, "high drop should enter Falling (AirborneFlail -> CatchFall)");
    assert!(
        !impact_events.is_empty(),
        "landing from 3.8 m should emit at least one BodyImpact event"
    );
    let max_speed = impact_events.iter().map(|(_, _, sp)| *sp).fold(0.0_f32, f32::max);
    assert!(max_speed > 2.5, "expected high-speed landing impact > 2.5 m/s, got {max_speed:.2}");
}

#[test]
fn backward_fall_tucks_chin_and_protects_head() {
    let mut app = app();
    let cfg = PuppetConfig {
        getup_delay: -1.0,
        ..default()
    };
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.3);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();

    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, 175.0) });
    command(&mut app, e, PuppetCommand::Fall);

    let mut max_chin_tuck = 0.0_f32;
    let mut first_body_hit = None;
    let mut head_hit_step = None;

    for step in 0..240 {
        app.update();
        let s = sense(&app, e);
        // Forward chin tuck: head up-axis pitches forward relative to chest up-axis
        // around chest's local right axis (negative angle around +X in character frame).
        let chest_rot = s.body(PuppetPart::SpineUpper).rot;
        let chest_up = (chest_rot * rig.part(PuppetPart::SpineUpper).up_local).normalize();
        let head_up = (s.body(PuppetPart::Head).rot * rig.part(PuppetPart::Head).up_local).normalize();
        let chest_right = (chest_rot * rig.part(PuppetPart::SpineUpper).forward_local)
            .cross(chest_up)
            .normalize_or(Vec3::X);
        let chin_tuck = -bevy_ext_puppet::rig::signed_angle(chest_up, head_up, chest_right);
        max_chin_tuck = max_chin_tuck.max(chin_tuck);

        if first_body_hit.is_none()
            && [
                PuppetPart::Pelvis,
                PuppetPart::SpineLower,
                PuppetPart::SpineUpper,
                PuppetPart::HandL,
                PuppetPart::HandR,
            ]
            .iter()
            .any(|p| s.touching(*p))
        {
            first_body_hit = Some(step);
        }
        if head_hit_step.is_none() && s.touching(PuppetPart::Head) {
            head_hit_step = Some(step);
        }
    }

    assert!(
        max_chin_tuck > 0.30,
        "backward fall should tuck chin > 0.30 rad toward chest, got {max_chin_tuck:.3}"
    );
    assert!(first_body_hit.is_some(), "backward fall should make ground contact");
    if let (Some(body_s), Some(head_s)) = (first_body_hit, head_hit_step) {
        assert!(
            body_s <= head_s,
            "pelvis/torso/arms (step {body_s}) should absorb ground contact before head (step {head_s})"
        );
    }
    assert_eq!(mode(&app, e), PuppetMode::Down, "backward fall should settle calmly to Down");
}

#[test]
fn sideways_fall_extends_downside_arm_and_protects_head() {
    let mut app = app();
    let cfg = PuppetConfig {
        getup_delay: -1.0,
        ..default()
    };
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.3);

    // Heavy lateral shove to the right (+X).
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(190.0, 0.0, 0.0) });
    command(&mut app, e, PuppetCommand::Fall);

    let mut right_arm_touch = None;
    let mut head_touch = None;
    let mut max_right_reach = 0.0_f32;

    for step in 0..210 {
        app.update();
        let s = sense(&app, e);
        let reach = s.body(PuppetPart::HandR).pos.x - s.body(PuppetPart::SpineUpper).pos.x;
        max_right_reach = max_right_reach.max(reach);
        if right_arm_touch.is_none()
            && (s.touching(PuppetPart::HandR) || s.touching(PuppetPart::LowerArmR))
        {
            right_arm_touch = Some(step);
        }
        if head_touch.is_none() && s.touching(PuppetPart::Head) {
            head_touch = Some(step);
        }
    }

    assert!(
        max_right_reach > 0.35,
        "downside right arm should reach outward (+X) > 0.35 m, got {max_right_reach:.3}"
    );
    assert!(right_arm_touch.is_some(), "downside arm should catch the sideways fall");
    if let (Some(arm_s), Some(head_s)) = (right_arm_touch, head_touch) {
        assert!(
            arm_s < head_s,
            "downside arm (step {arm_s}) should land before head (step {head_s})"
        );
    }
}

#[test]
fn ramp_push_adapts_step_height_to_slope() {
    let mut app = app();
    // Spawn a gentle 9° ramp rising in front of the puppet (-Z), meeting y = 0 just ahead of the toes.
    let slope_angle = 9.0_f32.to_radians();
    let ramp_len = 6.0_f32;
    let ramp_thick = 0.4_f32;
    let rot = Quat::from_rotation_x(slope_angle);
    // Top surface normal is `rot * Vec3::Y` = `(0, cos a, +sin a)` (sloping UP toward -Z!).
    let n = rot * Vec3::Y;
    let tangent = rot * Vec3::NEG_Z;
    let start_top = Vec3::new(0.0, 0.0, -0.16);
    let center = start_top + tangent * (ramp_len * 0.5) - n * (ramp_thick * 0.5);
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(6.0, ramp_thick, ramp_len),
        Transform::from_translation(center).with_rotation(rot),
        Friction::new(0.95),
    ));

    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.3);
    let rest_foot_y = sense(&app, e).body(PuppetPart::FootL).pos.y;

    // Push forward onto the ascending ramp!
    command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, -58.0) });

    let mut saw_sloped_probe = false;
    let mut saw_sloped_foot = false;
    let mut peak_grounded_foot_y = 0.0_f32;
    for _ in 0..340 {
        app.update();
        let s = sense(&app, e);
        if s.ahead_ground.is_some_and(|g| g.normal.angle_between(Vec3::Y) > 0.08) {
            saw_sloped_probe = true;
        }
        if s.foot_normals.iter().any(|fnm| fnm.angle_between(Vec3::Y) > 0.08) {
            saw_sloped_foot = true;
        }
        if s.feet_down() == 2 {
            let max_foot_y = s
                .body(PuppetPart::FootL)
                .pos
                .y
                .max(s.body(PuppetPart::FootR).pos.y);
            peak_grounded_foot_y = peak_grounded_foot_y.max(max_foot_y);
        }
    }

    assert!(saw_sloped_probe, "spatial sense should probe the ascending ramp ahead");
    assert!(saw_sloped_foot, "planted foot should sense and conform to the ramp surface normal");
    assert!(
        peak_grounded_foot_y > rest_foot_y + 0.015,
        "puppet should step upward onto the sloped ramp (rest={rest_foot_y:.3}, peak={peak_grounded_foot_y:.3})"
    );
    assert_eq!(
        mode(&app, e),
        PuppetMode::Animated,
        "puppet pushed onto a gentle ramp should adapt and recover upright"
    );
}

#[test]
fn physical_getup_rises_from_both_supine_and_prone() {
    for (label, push_z, expect_face_up) in [
        ("supine (backward fall)", 200.0_f32, true),
        ("prone (forward fall)", -200.0_f32, false),
    ] {
        let mut app = app();
        let cfg = PuppetConfig {
            getup_delay: 0.35,
            physical_getup: true,
            ..default()
        };
        let e = spawn_puppet(&mut app, cfg);
        run(&mut app, 0.3);
        command(&mut app, e, PuppetCommand::Push { impulse: Vec3::new(0.0, 0.0, push_z) });
        command(&mut app, e, PuppetCommand::Fall);

        let events = run_collecting_events(&mut app, 9.5);
        let getup_ev = events.iter().find_map(|ev| {
            if ev.puppet == e
                && let PuppetEventKind::GettingUp { face_up } = ev.kind
            {
                Some(face_up)
            } else {
                None
            }
        });
        assert_eq!(
            getup_ev,
            Some(expect_face_up),
            "{label}: expected GettingUp {{ face_up: {expect_face_up} }}, got {getup_ev:?}"
        );
        assert_eq!(
            mode(&app, e),
            PuppetMode::Animated,
            "{label}: physical get-up should recover to Animated"
        );
        let s = sense(&app, e);
        assert!(
            s.trunk_tilt < 0.35 && s.pelvis_height > 0.78,
            "{label}: expected upright after physical get-up (tilt={:.2}, pel={:.2})",
            s.trunk_tilt,
            s.pelvis_height
        );
    }
}

#[test]
fn dying_collapse_buckles_legs_progressively_then_settles() {
    let mut app = app();
    let cfg = PuppetConfig {
        dying_collapse_time: 0.32,
        ..default()
    };
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.3);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();

    command(&mut app, e, PuppetCommand::Kill);
    // Mid-collapse (~0.5 s): knees should be visibly buckling as legs give out first.
    let mut max_early_knee = 0.0_f32;
    for _ in 0..32 {
        app.update();
        let s = sense(&app, e);
        for side in [Side::Left, Side::Right] {
            let dt = s.body(PuppetPart::thigh(side)).rot * rig.part(PuppetPart::thigh(side)).axis_local;
            let dc = s.body(PuppetPart::calf(side)).rot * rig.part(PuppetPart::calf(side)).axis_local;
            max_early_knee = max_early_knee.max(dt.angle_between(dc));
        }
    }
    assert!(
        max_early_knee > 0.40,
        "dying collapse should buckle knees early (> 0.40 rad), got {max_early_knee:.3}"
    );

    // Let body finish collapsing and settling under passive viscoelastic tissue damping.
    run(&mut app, 3.5);
    let s = sense(&app, e);
    assert_eq!(mode(&app, e), PuppetMode::Dead);
    assert!(s.pelvis_height < 0.35, "corpse should lie low, pel={:.2}", s.pelvis_height);
    assert!(
        s.com_vel.length() < 0.20,
        "passive joint damping should settle corpse cleanly, speed={:.2}",
        s.com_vel.length()
    );
}

#[test]
fn reach_for_wound_moves_free_hand_toward_struck_part() {
    let mut app = app();
    let cfg = PuppetConfig {
        partial_hit_impulse: f32::INFINITY,
        reach_for_wound: true,
        ..default()
    };
    let e = spawn_puppet(&mut app, cfg);
    run(&mut app, 0.3);

    let rest_s = sense(&app, e);
    let rest_dist = rest_s
        .body(PuppetPart::HandR)
        .pos
        .distance(rest_s.body(PuppetPart::UpperArmL).pos);

    // Significant slash across left upper arm/shoulder → right hand reaches across to clutch!
    command(
        &mut app,
        e,
        PuppetCommand::Hit {
            part: PuppetPart::UpperArmL,
            impulse: Vec3::new(14.0, 0.0, 6.0),
            point: None,
            kind: HitKind::Slash,
        },
    );

    let mut min_dist = rest_dist;
    let mut wound_recorded = false;
    let mut saw_reach_beh = false;

    for _ in 0..90 {
        app.update();
        let s = sense(&app, e);
        let d = s
            .body(PuppetPart::HandR)
            .pos
            .distance(s.body(PuppetPart::UpperArmL).pos);
        min_dist = min_dist.min(d);
        if let Some(neuro) = app.world().get::<PuppetNeuroState>(e)
            && neuro.wound.is_some_and(|w| w.part == PuppetPart::UpperArmL)
        {
            wound_recorded = true;
        }
        if let Some(beh) = app.world().get::<PuppetBehaviours>(e)
            && beh.has(ReachForWound::NAME)
        {
            saw_reach_beh = true;
        }
    }

    assert!(wound_recorded, "significant wound on UpperArmL should be stored in PuppetNeuroState");
    assert!(saw_reach_beh, "ReachForWound behaviour should activate on significant wound");
    assert!(
        min_dist < rest_dist - 0.14,
        "opposite hand should move substantially closer to wound (rest={rest_dist:.3}, min={min_dist:.3})"
    );
}

#[test]
fn trip_command_triggers_tripped_event_and_stumble() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.3);

    command(
        &mut app,
        e,
        PuppetCommand::Trip {
            side: Side::Right,
            impulse: Vec3::new(0.0, 0.0, -35.0),
        },
    );
    let events = run_collecting_events(&mut app, 0.8);
    let saw_trip = events.iter().any(|ev| {
        ev.puppet == e && matches!(ev.kind, PuppetEventKind::Tripped { side: Side::Right })
    });
    assert!(saw_trip, "Trip command should emit PuppetEventKind::Tripped");
    let saw_active = events.iter().any(|ev| {
        ev.puppet == e
            && matches!(
                ev.kind,
                PuppetEventKind::ModeChanged {
                    to: PuppetMode::Active | PuppetMode::Falling,
                    ..
                }
            )
    });
    assert!(saw_active, "Trip should transition puppet into Active stumble or Falling");
}

#[test]
fn look_at_command_steers_gaze_and_turns_head() {
    let mut app = app();
    let e = spawn_puppet(&mut app, PuppetConfig::default());
    run(&mut app, 0.3);
    let rig = app.world().get::<PuppetRig>(e).unwrap().clone();

    command(&mut app, e, PuppetCommand::Activate);
    command(
        &mut app,
        e,
        PuppetCommand::LookAt {
            point: Vec3::new(2.0, 1.55, -0.6),
            duration: 1.5,
        },
    );
    run(&mut app, 0.4);

    let s = sense(&app, e);
    let neuro = app.world().get::<PuppetNeuroState>(e).unwrap();
    assert!(neuro.active_gaze(s.time).is_some(), "LookAt should set active gaze target");

    let head_fwd = (s.body(PuppetPart::Head).rot * rig.part(PuppetPart::Head).forward_local)
        .reject_from(s.up)
        .normalize_or(Vec3::NEG_Z);
    assert!(
        head_fwd.dot(Vec3::X) > 0.35,
        "head should turn decisively toward +X LookAt target, got dot={:.3}",
        head_fwd.dot(Vec3::X)
    );
}
