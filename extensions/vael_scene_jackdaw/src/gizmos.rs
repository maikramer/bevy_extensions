//! Gizmos do viewport: colisores, exclusões de spawn, regiões de spawners,
//! raios de portais e de luzes. Só desenha (não entra no documento).

use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use bevy_ext_scene::ColliderShape;
use jackdaw_extension::prelude::*;
use jackdaw_extension::selection::Selection;
use jackdaw_extension::viewport::MainViewportCamera;

use crate::index::SceneIndex;

/// Colisores a menos disto da câmara (modo "perto").
const NEAR: f32 = 70.0;
/// Teto de colisores desenhados por frame.
const MAX_COLLIDERS: usize = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoMode {
    Off,
    /// Só a seleção.
    #[default]
    Selected,
    /// Seleção + colisores, anéis e regiões perto da câmara.
    Nearby,
}

impl GizmoMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "desligados",
            Self::Selected => "seleção",
            Self::Nearby => "perto da câmara",
        }
    }
}

#[derive(Resource, Default)]
pub struct SceneGizmos {
    pub mode: GizmoMode,
}

#[operator(
    id = "vael.scene.gizmos",
    label = "Vael: gizmos da cena (desligados / seleção / perto)",
    allows_undo = false
)]
pub fn cycle_gizmos(_params: In<OperatorParameters>, mut gizmos: ResMut<SceneGizmos>) -> OperatorResult {
    gizmos.mode = match gizmos.mode {
        GizmoMode::Off => GizmoMode::Selected,
        GizmoMode::Selected => GizmoMode::Nearby,
        GizmoMode::Nearby => GizmoMode::Off,
    };
    OperatorResult::Finished
}

pub fn draw(
    mut gizmos: Gizmos,
    settings: Res<SceneGizmos>,
    index: Res<SceneIndex>,
    selection: Option<Res<Selection>>,
    cameras: Query<&GlobalTransform, With<MainViewportCamera>>,
    globals: Query<&GlobalTransform>,
) {
    if settings.mode == GizmoMode::Off {
        return;
    }
    let camera = cameras.iter().next().map(GlobalTransform::translation);
    let selected: Vec<Entity> = selection
        .map(|selection| selection.entities.clone())
        .unwrap_or_default();

    let mut drawn = 0;
    for (entity, info) in &index.nodes {
        let Ok(global) = globals.get(*entity) else {
            continue;
        };
        let position = global.translation();
        let is_selected = selected.contains(entity);

        // Anéis/regiões: da seleção sempre; dos outros só no modo "perto"
        // (e a menos de 400 m), para não encher a vista de perto.
        let show_area = is_selected
            || (settings.mode == GizmoMode::Nearby
                && camera.is_some_and(|camera| camera.distance(position) < 400.0));
        if show_area && let Some(ring) = &info.ring {
            let center = match ring.at {
                Some(at) => Vec3::new(at.x, position.y, at.y),
                None => position,
            };
            let color = if is_selected { Color::WHITE } else { ring.color };
            ring_column(&mut gizmos, center, ring.radius, color);
        }
        if show_area && let Some((min, max)) = info.region {
            let max = Vec3::new(max.x, max.y.max(min.y + 120.0), max.z);
            let center = (min + max) * 0.5;
            gizmos.cube(
                Transform::from_translation(center).with_scale((max - min).abs()),
                Color::srgb(0.95, 0.65, 0.20),
            );
        }

        let Some(collider) = &info.collider else {
            continue;
        };
        let near = settings.mode == GizmoMode::Nearby
            && camera.is_some_and(|camera| camera.distance(position) < NEAR);
        if !(is_selected || near) || (!is_selected && drawn >= MAX_COLLIDERS) {
            continue;
        }
        drawn += 1;
        let color = match (is_selected, info.destructible.is_some()) {
            (true, _) => Color::WHITE,
            (false, true) => Color::srgb(1.0, 0.62, 0.25),
            (false, false) => Color::srgb(0.35, 0.95, 0.55),
        };
        let (scale, rotation, _) = global.to_scale_rotation_translation();
        match &collider.shape {
            ColliderShape::Cylinder {
                radius,
                half_height,
            }
            | ColliderShape::Capsule {
                radius,
                half_height,
            } => {
                let r = radius * scale.x.max(scale.z);
                let h = half_height * scale.y;
                // Centrado no transform do prop (como o colisor do Viber).
                cylinder(&mut gizmos, position, r, h, color);
            }
            ColliderShape::Sphere { radius } => {
                gizmos.sphere(Isometry3d::from_translation(position), radius * scale.max_element(), color);
            }
            ColliderShape::Box { size, offset_y } => {
                gizmos.cube(
                    Transform::from_translation(position + rotation * Vec3::Y * *offset_y)
                        .with_rotation(rotation)
                        .with_scale(*size * scale),
                    color,
                );
            }
            ColliderShape::Mesh { .. } | ColliderShape::ConvexHull { .. } | ColliderShape::Auto => {
                // Malha/casco: só a marca no pivô (a forma vem do GLB).
                let s = 0.6 * scale.max_element().max(0.5);
                gizmos.line(position - Vec3::X * s, position + Vec3::X * s, color);
                gizmos.line(position - Vec3::Z * s, position + Vec3::Z * s, color);
                gizmos.line(position, position + Vec3::Y * s * 2.0, color);
            }
            ColliderShape::None => {}
        }
    }
}

fn flat(center: Vec3) -> Isometry3d {
    Isometry3d::new(center, Quat::from_rotation_x(FRAC_PI_2))
}

fn cylinder(gizmos: &mut Gizmos, center: Vec3, radius: f32, half_height: f32, color: Color) {
    let bottom = center - Vec3::Y * half_height;
    let top = center + Vec3::Y * half_height;
    gizmos.circle(flat(bottom), radius, color);
    gizmos.circle(flat(top), radius, color);
    for dir in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        gizmos.line(bottom + dir * radius, top + dir * radius, color);
    }
}

/// Anel no chão com uma "parede" baixa, visível sobre o relevo.
fn ring_column(gizmos: &mut Gizmos, center: Vec3, radius: f32, color: Color) {
    gizmos.circle(flat(center), radius, color);
    gizmos.circle(flat(center + Vec3::Y * 6.0), radius, color.with_alpha(0.5));
    for dir in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        gizmos.line(center + dir * radius, center + dir * radius + Vec3::Y * 6.0, color);
    }
}
