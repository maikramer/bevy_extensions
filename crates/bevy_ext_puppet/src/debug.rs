//! Debug drawing: bodies, joints, centre of mass, Center of Pressure (CoP),
//! capture point & flywheel capture region, support polygon, ground/slope
//! normal, wall-brace surface probe, active virtual forces, gaze target, and
//! contact impact spikes. Add [`PuppetDebugPlugin`] (feature `gizmos`) and
//! toggle with [`PuppetDebug`].

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::behaviour::PuppetRequests;
use crate::neuro::PuppetNeuroState;
use crate::part::PuppetPart;
use crate::rig::PuppetRig;
use crate::sense::PuppetSense;
use crate::state::{PuppetMode, PuppetState};

#[derive(Resource, Clone, Copy)]
pub struct PuppetDebug {
    pub enabled: bool,
}

impl Default for PuppetDebug {
    fn default() -> Self {
        Self { enabled: true }
    }
}

pub struct PuppetDebugPlugin;

impl Plugin for PuppetDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PuppetDebug>()
            .add_systems(PostUpdate, draw_puppets.after(TransformSystems::Propagate));
    }
}

fn mode_color(mode: PuppetMode) -> Color {
    match mode {
        PuppetMode::Animated => Color::srgb(0.4, 0.8, 1.0),
        PuppetMode::Active => Color::srgb(0.4, 1.0, 0.4),
        PuppetMode::Falling => Color::srgb(1.0, 0.6, 0.2),
        PuppetMode::Down => Color::srgb(1.0, 0.9, 0.3),
        PuppetMode::GettingUp => Color::srgb(0.8, 0.5, 1.0),
        PuppetMode::Dead => Color::srgb(0.6, 0.6, 0.6),
    }
}

#[allow(clippy::type_complexity)]
fn draw_puppets(
    debug: Res<PuppetDebug>,
    mut gizmos: Gizmos,
    puppets: Query<(
        &PuppetRig,
        &PuppetSense,
        &PuppetState,
        Option<&PuppetRequests>,
        Option<&PuppetNeuroState>,
    )>,
    bodies: Query<(&Position, &Rotation)>,
) {
    if !debug.enabled {
        return;
    }
    for (rig, sense, state, req, neuro) in &puppets {
        let color = mode_color(state.mode);
        for part in PuppetPart::ALL {
            let r = rig.part(part);
            let Ok((pos, rot)) = bodies.get(r.body) else { continue };
            let end = pos.0 + rot.0 * r.end_local;
            let c = if sense.touching(part) { Color::srgb(1.0, 0.2, 0.2) } else { color };
            gizmos.line(pos.0, end, c);
            gizmos.sphere(Isometry3d::from_translation(pos.0), 0.02, c);
            // Contact normal & impact spike when a limb impacts forcefully.
            let imp = sense.contact_impulses[part.index()];
            if sense.touching(part) && imp > 4.0 {
                let pt = sense.contact_points[part.index()];
                let n = sense.contact_normals[part.index()];
                let scale = (imp * 0.012).min(0.65);
                gizmos.arrow(pt, pt + n * scale, Color::srgb(1.0, 0.25, 0.1));
            }
        }
        let ground = sense.ground + 0.01;
        let com_proj = sense.lift(sense.flat(sense.com), ground);
        gizmos.sphere(Isometry3d::from_translation(sense.com), 0.05, Color::WHITE);
        gizmos.line(sense.com, com_proj, Color::WHITE);
        // Ground normal arrow at CoM projection.
        gizmos.arrow(com_proj, com_proj + sense.ground_normal * 0.28, Color::srgb(0.3, 1.0, 0.5));
        // Real Center of Pressure (orange).
        if !sense.support.is_empty() {
            let cop_w = sense.lift(sense.cop, ground + 0.01);
            gizmos.sphere(Isometry3d::from_translation(cop_w), 0.032, Color::srgb(1.0, 0.55, 0.1));
        }

        let cp_color = if sense.capture_margin > 0.0 {
            Color::srgb(1.0, 0.2, 0.2)
        } else {
            Color::srgb(0.2, 1.0, 0.2)
        };
        let cp_world = sense.lift(sense.capture_point, ground);
        gizmos.sphere(Isometry3d::from_translation(cp_world), 0.04, cp_color);
        if sense.capture_region_radius > 0.01 {
            gizmos.circle(
                Isometry3d::new(cp_world, Quat::from_rotation_arc(Vec3::Z, sense.up)),
                sense.capture_region_radius,
                Color::srgba(0.2, 0.9, 1.0, 0.65),
            );
        }
        let am = sense.angular_momentum * 0.02;
        if am.length_squared() > 1e-4 {
            gizmos.arrow(sense.com, sense.com + am, Color::srgb(0.2, 0.9, 1.0));
        }
        let n = sense.support.len();
        for i in 0..n {
            let a = sense.lift(sense.support[i], ground);
            let b = sense.lift(sense.support[(i + 1) % n], ground);
            gizmos.line(a, b, Color::srgb(1.0, 1.0, 0.0));
        }
        // Nearby wall brace probe (magenta disc & normal).
        if let Some(wall) = sense.nearby_wall {
            let q = Quat::from_rotation_arc(Vec3::Z, wall.normal.normalize_or(Vec3::Z));
            gizmos.circle(Isometry3d::new(wall.point, q), 0.14, Color::srgb(1.0, 0.2, 0.9));
            gizmos.arrow(wall.point, wall.point + wall.normal * 0.35, Color::srgb(1.0, 0.2, 0.9));
        }
        // Active VirtualForce vectors (yellow).
        if let Some(req) = req {
            for vf in &req.virtual_forces {
                let arrow = vf.force * 0.0012;
                if arrow.length_squared() > 1e-4 {
                    gizmos.arrow(vf.application, vf.application + arrow, Color::srgb(1.0, 0.95, 0.2));
                }
            }
        }
        // Active orienting gaze ray (soft cyan).
        if let Some(neuro) = neuro
            && let Some(gaze_pt) = neuro.active_gaze(sense.time)
        {
            let head = sense.body(PuppetPart::Head).pos;
            gizmos.line(head, gaze_pt, Color::srgba(0.5, 0.95, 1.0, 0.7));
            gizmos.sphere(Isometry3d::from_translation(gaze_pt), 0.03, Color::srgb(0.5, 0.95, 1.0));
        }
    }
}
