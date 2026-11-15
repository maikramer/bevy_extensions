//! Gizmos do viewport: colisores, exclusões de spawn, regiões de spawners,
//! raios de portais e de luzes. Só desenha (não entra no documento).

use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use bevy_ext_scene::ColliderShape;
use jackdaw_extension::prelude::*;
use jackdaw_extension::selection::Selection;
use jackdaw_extension::viewport::MainViewportCamera;

use crate::cinematics::{CinematicFocus, Cinematics};
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
    cinematics: Option<Res<Cinematics>>,
    focus: Option<Res<CinematicFocus>>,
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

        // Câmara autorada: olho + frente + frustum com o fov.
        if show_area && let Some(camera_spec) = &info.camera {
            let (_, rotation, _) = global.to_scale_rotation_translation();
            let color = if is_selected {
                Color::WHITE
            } else {
                Color::srgb(0.45, 0.75, 1.0)
            };
            camera_marker(
                &mut gizmos,
                position,
                rotation,
                camera_spec.distance.unwrap_or(4.0),
                camera_spec.fov.unwrap_or(60.0),
                color,
            );
        }

        // Sala de interior: o rectângulo e a pose nominal da câmara fixa.
        if show_area && let Some(interior) = &info.interior {
            interior_room(&mut gizmos, interior, is_selected);
        }

        // Região de bioma: o polígono que manda no chão/música.
        if show_area && let Some(biome) = &info.biome {
            biome_polygon(&mut gizmos, biome.polygon.as_deref(), position.y, is_selected);
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

    // Trajecto dos planos: o filme em foco sempre; os outros no modo "perto"
    // (se o primeiro plano estiver a menos de 800 m da câmara).
    let Some(cinematics) = cinematics else {
        return;
    };
    let focused = focus.as_deref().map(|focus| focus.index);
    for (index, row) in cinematics.rows.iter().enumerate() {
        let is_focused = focused.is_some_and(|focused| focused == index);
        let near = row.shots.first().is_some_and(|shot| {
            camera.is_none_or(|camera| camera.distance(shot.from) < 800.0)
        });
        if !(is_focused || (settings.mode == GizmoMode::Nearby && near)) {
            continue;
        }
        let color = if is_focused {
            Color::srgb(1.0, 0.78, 0.30)
        } else {
            Color::srgb(0.55, 0.55, 0.62)
        };
        for shot in &row.shots {
            let shot_color = if selected.contains(&shot.entity) {
                Color::WHITE
            } else {
                color
            };
            gizmos.line(shot.from, shot.to, shot_color);
            gizmos.line(shot.look, shot.look_to, shot_color.with_alpha(0.55));
            cross(&mut gizmos, shot.from, 2.0, shot_color);
            cross(&mut gizmos, shot.look, 1.2, Color::srgb(0.35, 0.85, 0.95));
            gizmos.line(shot.from, shot.look, shot_color.with_alpha(0.25));
        }
    }
}

/// Marca de câmara: corpo, frente e frustum com o fov autorado.
fn camera_marker(
    gizmos: &mut Gizmos,
    eye: Vec3,
    rotation: Quat,
    distance: f32,
    fov: f32,
    color: Color,
) {
    let forward = rotation * Vec3::NEG_Z;
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let body = 0.5;
    for (axis, sign) in [(right, 1.0), (right, -1.0), (up, 1.0), (up, -1.0)] {
        gizmos.line(eye, eye + axis * body * sign, color);
    }
    gizmos.line(eye, eye + forward * body, color);
    let reach = distance.max(2.0) * 0.8;
    let half_h = (fov.clamp(10.0, 170.0) * 0.5).to_radians().tan() * reach;
    let half_w = half_h * 16.0 / 9.0;
    let plane = eye + forward * reach;
    for corner in [
        plane + right * half_w + up * half_h,
        plane - right * half_w + up * half_h,
        plane - right * half_w - up * half_h,
        plane + right * half_w - up * half_h,
    ] {
        gizmos.line(eye, corner, color.with_alpha(0.6));
    }
}

/// Sala de interior: o rectângulo `at`/`size` e a pose nominal da câmara
/// fixa (distância/inclinação/yaw), que o jogo recalcula por sala.
fn interior_room(gizmos: &mut Gizmos, interior: &bevy_ext_scene::InteriorSceneSpec, selected: bool) {
    let color = if selected {
        Color::WHITE
    } else {
        Color::srgb(0.70, 0.70, 0.95)
    };
    let half = interior.size * 0.5;
    let y = 0.5;
    let corners = [
        Vec3::new(interior.at.x - half.x, y, interior.at.y - half.y),
        Vec3::new(interior.at.x + half.x, y, interior.at.y - half.y),
        Vec3::new(interior.at.x + half.x, y, interior.at.y + half.y),
        Vec3::new(interior.at.x - half.x, y, interior.at.y + half.y),
    ];
    for index in 0..4 {
        gizmos.line(corners[index], corners[(index + 1) % 4], color);
    }
    let look = Vec3::new(interior.at.x, y + 1.0, interior.at.y);
    let pitch = interior.camera_pitch.unwrap_or(58.0);
    let yaw = interior.camera_yaw.unwrap_or(0.0);
    let distance = interior.camera_distance.unwrap_or(22.0);
    let eye = look
        + Vec3::new(
            yaw.to_radians().sin() * pitch.to_radians().cos() * distance,
            pitch.to_radians().sin() * distance,
            yaw.to_radians().cos() * pitch.to_radians().cos() * distance,
        );
    gizmos.line(eye, look, color.with_alpha(0.7));
    cross(gizmos, eye, 1.5, color);
}

/// Contorno do polígono de um bioma (`[x,z;x,z;…]`).
fn biome_polygon(gizmos: &mut Gizmos, polygon: Option<&str>, y: f32, selected: bool) {
    let Some(polygon) = polygon else {
        return;
    };
    let points: Vec<Vec2> = polygon
        .trim_matches(|c| c == '[' || c == ']')
        .split(';')
        .filter_map(|pair| {
            let mut parts = pair.split(',').map(|part| part.trim().parse::<f32>());
            let x = parts.next()?.ok()?;
            let z = parts.next()?.ok()?;
            Some(Vec2::new(x, z))
        })
        .collect();
    if points.len() < 2 {
        return;
    }
    let color = if selected {
        Color::WHITE
    } else {
        Color::srgb(0.45, 0.85, 0.45)
    };
    let height = y + 2.0;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        gizmos.line(
            Vec3::new(a.x, height, a.y),
            Vec3::new(b.x, height, b.y),
            color,
        );
    }
}

/// Cruz de 3 eixos (marcador de ponto no mundo).
fn cross(gizmos: &mut Gizmos, at: Vec3, size: f32, color: Color) {
    gizmos.line(at - Vec3::X * size, at + Vec3::X * size, color);
    gizmos.line(at - Vec3::Y * size, at + Vec3::Y * size, color);
    gizmos.line(at - Vec3::Z * size, at + Vec3::Z * size, color);
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
