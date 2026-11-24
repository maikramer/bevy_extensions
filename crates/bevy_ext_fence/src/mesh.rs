//! As peças viram malhas: posição, normal, UV e cor por vértice.
//!
//! Uma malha por material ([`Slot`]). Tudo em metros:
//!
//! * o UV segue a peça — V ao longo do eixo, U à volta do perfil — dividido por
//!   `tile_m` da predefinição, com um desvio por peça: duas tábuas com o mesmo
//!   veio denunciam a repetição. O veio da textura corre ao longo de V;
//! * a cor de vértice leva o tom de cada peça e, junto ao chão, o desgaste da
//!   base (terra e líquen) que o [`FenceStyle::weather_tint`] dá;
//! * as tampas são mais escuras: é o topo cortado de uma peça.
//!
//! Tudo em facetas planas (uma normal por face): é o *Volume Talhado* do
//! `docs/ART.md` — chanfros francos, sem arestas derretidas.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::layout::{Beam, FenceLayout, Slot, Stud, Tube};
use crate::specs::FenceStyle;

type Rgba = [f32; 4];

/// Os vértices de uma malha, antes de entrar no `Assets<Mesh>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub colors: Vec<Rgba>,
    pub indices: Vec<u32>,
}

impl SurfaceMesh {
    fn vertex(&mut self, position: Vec3, normal: Vec3, uv: Vec2, color: Rgba) -> u32 {
        self.positions.push(position);
        self.normals.push(normal);
        self.uvs.push(uv);
        self.colors.push(color);
        (self.positions.len() - 1) as u32
    }

    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    /// Triângulos batidos à mão, se houver (testes e diagnóstico).
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// A malha do Bevy (`TriangleList`, índices de 32 bits).
    pub fn to_mesh(&self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        let positions: Vec<[f32; 3]> = self.positions.iter().map(|position| position.to_array()).collect();
        let normals: Vec<[f32; 3]> = self.normals.iter().map(|normal| normal.to_array()).collect();
        let uvs: Vec<[f32; 2]> = self.uvs.iter().map(|uv| uv.to_array()).collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colors.clone());
        mesh.insert_indices(Indices::U32(self.indices.clone()));
        mesh
    }
}

/// O desgaste de base: quanto baixa o tom junto ao chão.
struct Weather {
    height: f32,
    tint: Vec3,
}

impl Weather {
    fn at(&self, above_ground: f32) -> Vec3 {
        let t = (above_ground / self.height).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        self.tint.lerp(Vec3::ONE, t)
    }
}

/// O tom mais escuro de uma peça cortada (o topo de uma tábua).
const END_GRAIN: f32 = 0.82;

/// A secção de uma peça, no plano (largura, espessura), vista do topo.
///
/// Talhada a enxó: um quadrado com os cantos cortados a 45° — oito facetas.
/// Tronco: um polígono de `sides` lados.
pub fn profile(timber: &crate::specs::Timber) -> Vec<Vec2> {
    let (half_w, half_d) = (timber.width.max(1e-3) * 0.5, timber.depth.max(1e-3) * 0.5);
    match timber.shape {
        crate::specs::TimberShape::Round => {
            let sides = timber.sides.clamp(3, 16);
            (0..sides)
                .map(|index| {
                    let angle = std::f32::consts::TAU * index as f32 / sides as f32;
                    Vec2::new(angle.cos() * half_w, angle.sin() * half_d)
                })
                .collect()
        }
        crate::specs::TimberShape::Hewn => {
            let cut = (timber.chamfer.clamp(0.0, 0.45) * half_w.min(half_d)).min(half_w.min(half_d) * 0.49);
            if cut < 1e-4 {
                vec![
                    Vec2::new(-half_w, -half_d),
                    Vec2::new(half_w, -half_d),
                    Vec2::new(half_w, half_d),
                    Vec2::new(-half_w, half_d),
                ]
            } else {
                vec![
                    Vec2::new(-half_w + cut, -half_d),
                    Vec2::new(half_w - cut, -half_d),
                    Vec2::new(half_w, -half_d + cut),
                    Vec2::new(half_w, half_d - cut),
                    Vec2::new(half_w - cut, half_d),
                    Vec2::new(-half_w + cut, half_d),
                    Vec2::new(-half_w, half_d - cut),
                    Vec2::new(-half_w, -half_d + cut),
                ]
            }
        }
    }
}

/// Um vector unitário perpendicular a `axis`.
fn perpendicular(axis: Vec3) -> Vec3 {
    let seed = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    axis.cross(seed).normalize_or_zero()
}

struct Face<'a> {
    positions: [&'a Vec3; 3],
    uvs: [Vec2; 3],
    colors: [Rgba; 3],
}

impl SurfaceMesh {
    /// Um triângulo com a normal virada para `outward` (o lado de fora da peça).
    fn face(&mut self, face: Face, outward: Vec3) {
        let [a, b, c] = face.positions;
        let normal = (*b - *a).cross(*c - *a);
        let length = normal.length();
        if length < 1e-9 || !length.is_finite() {
            return; // face degenerada: a ponta de uma estaca fecha em recta
        }
        let normal = normal / length;
        let (order, normal) = if normal.dot(outward) < 0.0 {
            ([0, 2, 1], -normal)
        } else {
            ([0, 1, 2], normal)
        };
        let indices = order.map(|index| self.vertex(*face.positions[index], normal, face.uvs[index], face.colors[index]));
        self.triangle(indices[0], indices[1], indices[2]);
    }

    fn push_beam(&mut self, beam: &Beam, tile: f32, weather: &Weather) {
        let axis = beam.to - beam.from;
        let length = axis.length();
        if length < 1e-4 {
            return;
        }
        let axis = axis / length;
        let mut across = beam.across - axis * beam.across.dot(axis);
        across = if across.length_squared() < 1e-8 {
            perpendicular(axis)
        } else {
            across.normalize()
        };
        let depth_axis = axis.cross(across);
        let shape = profile(&beam.timber);
        let count = shape.len();
        // Distância à volta do perfil: o U acompanha a superfície, faceta a faceta.
        let mut perimeter = Vec::with_capacity(count + 1);
        perimeter.push(0.0);
        for index in 0..count {
            perimeter.push(perimeter[index] + (shape[(index + 1) % count] - shape[index]).length());
        }
        let ring = |center: Vec3, scale: Vec2| -> Vec<Vec3> {
            shape
                .iter()
                .map(|point| center + across * (point.x * scale.x) + depth_axis * (point.y * scale.y))
                .collect()
        };
        let start = ring(beam.from, Vec2::ONE);
        let end = ring(beam.to, beam.end_scale);
        let (v0, v1) = (beam.uv_offset.y, beam.uv_offset.y + length / tile);
        let color = |point: Vec3| {
            let shade = beam.tint * weather.at(point.y - beam.ground_y);
            [shade.x, shade.y, shade.z, 1.0]
        };

        for index in 0..count {
            let next = (index + 1) % count;
            let (u0, u1) = (perimeter[index] / tile + beam.uv_offset.x, perimeter[index + 1] / tile + beam.uv_offset.x);
            let corners = [start[index], start[next], end[next], end[index]];
            let radial = (corners[0] + corners[1]) * 0.5 - beam.from;
            let outward = radial - axis * radial.dot(axis);
            let shade = [
                color(corners[0]),
                color(corners[1]),
                color(corners[2]),
                color(corners[3]),
            ];
            self.face(
                Face {
                    positions: [&corners[0], &corners[1], &corners[2]],
                    uvs: [Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1)],
                    colors: [shade[0], shade[1], shade[2]],
                },
                outward,
            );
            self.face(
                Face {
                    positions: [&corners[0], &corners[2], &corners[3]],
                    uvs: [Vec2::new(u0, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)],
                    colors: [shade[0], shade[2], shade[3]],
                },
                outward,
            );
        }

        if beam.apex > 0.0 {
            let tip = beam.to + axis * beam.apex;
            let shade = color(tip);
            for index in 0..count {
                let next = (index + 1) % count;
                let radial = (end[index] + end[next]) * 0.5 - beam.to;
                self.face(
                    Face {
                        positions: [&end[index], &end[next], &tip],
                        uvs: [
                            Vec2::new(uv_at(&perimeter, index) / tile + beam.uv_offset.x, v1),
                            Vec2::new(uv_at(&perimeter, index + 1) / tile + beam.uv_offset.x, v1),
                            Vec2::new(uv_at(&perimeter, index + 1) / tile + beam.uv_offset.x, v1),
                        ],
                        colors: [color(end[index]), color(end[next]), shade],
                    },
                    radial - axis * radial.dot(axis),
                );
            }
            return;
        }

        let cut = |point: Vec3| {
            let shade = beam.tint * weather.at(point.y - beam.ground_y) * END_GRAIN;
            [shade.x, shade.y, shade.z, 1.0]
        };
        for (side, center, ring_points, cap_on, normal) in [
            (0, beam.from, &start, beam.caps[0], -axis),
            (1, beam.to, &end, beam.caps[1], axis),
        ] {
            // Um anel fechado em recta (a ponta de uma estaca) não tem tampa.
            let collapsed = side == 1 && beam.end_scale.min_element() < 0.05;
            if !cap_on || collapsed {
                continue;
            }
            let v = if side == 0 { v0 } else { v1 };
            let hub = self.vertex(center, normal, Vec2::new(beam.uv_offset.x, v), cut(center));
            let rim: Vec<u32> = shape
                .iter()
                .zip(ring_points.iter())
                .map(|(point, world)| {
                    self.vertex(*world, normal, Vec2::new(point.x / tile + beam.uv_offset.x, point.y / tile + beam.uv_offset.y), cut(*world))
                })
                .collect();
            for index in 0..count {
                self.triangle(hub, rim[(index + 1) % count], rim[index]);
            }
        }
    }

    fn push_tube(&mut self, tube: &Tube, tile: f32, weather: &Weather) {
        let points = &tube.path;
        let count = points.len();
        if count < 2 || tube.radius.len() != count || tube.sides < 3 {
            return;
        }
        let sides = tube.sides.clamp(3, 16) as usize;
        let mut tangents: Vec<Vec3> = Vec::with_capacity(count);
        for index in 0..count {
            let (previous, next) = (points[index.saturating_sub(1)], points[(index + 1).min(count - 1)]);
            tangents.push((next - previous).normalize_or_zero());
        }
        // Transporte paralelo: a moldura de cada anel roda com a curva, sem torcer.
        let mut side = perpendicular(tangents[0]);
        let mut rings: Vec<Vec<Vec3>> = Vec::with_capacity(count);
        let mut along = vec![0.0];
        for index in 0..count {
            if index > 0 {
                side = Quat::from_rotation_arc(tangents[index - 1], tangents[index]) * side;
                along.push(along[index - 1] + (points[index] - points[index - 1]).length());
                side = (side - tangents[index] * side.dot(tangents[index])).normalize_or_zero();
                if side == Vec3::ZERO {
                    side = perpendicular(tangents[index]);
                }
            }
            let up = tangents[index].cross(side);
            rings.push(
                (0..sides)
                    .map(|corner| {
                        let angle = std::f32::consts::TAU * corner as f32 / sides as f32;
                        points[index] + (side * angle.cos() + up * angle.sin()) * tube.radius[index]
                    })
                    .collect(),
            );
        }
        let mean = tube.radius.iter().sum::<f32>() / count as f32;
        let girth = std::f32::consts::TAU * mean / tile;
        let color = |point: Vec3| {
            let shade = tube.tint * weather.at(point.y - tube.ground_y);
            [shade.x, shade.y, shade.z, 1.0]
        };
        for index in 0..count - 1 {
            let (v0, v1) = (tube.uv_offset.y + along[index] / tile, tube.uv_offset.y + along[index + 1] / tile);
            for corner in 0..sides {
                let next = (corner + 1) % sides;
                let quad = [
                    rings[index][corner],
                    rings[index][next],
                    rings[index + 1][next],
                    rings[index + 1][corner],
                ];
                let (u0, u1) = (
                    girth * corner as f32 / sides as f32 + tube.uv_offset.x,
                    girth * (corner + 1) as f32 / sides as f32 + tube.uv_offset.x,
                );
                let shade = [color(quad[0]), color(quad[1]), color(quad[2]), color(quad[3])];
                let outward = (quad[0] + quad[1]) * 0.5 - points[index];
                self.face(
                    Face {
                        positions: [&quad[0], &quad[1], &quad[2]],
                        uvs: [Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1)],
                        colors: [shade[0], shade[1], shade[2]],
                    },
                    outward,
                );
                self.face(
                    Face {
                        positions: [&quad[0], &quad[2], &quad[3]],
                        uvs: [Vec2::new(u0, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)],
                        colors: [shade[0], shade[2], shade[3]],
                    },
                    outward,
                );
            }
        }
        // As pontas fecham-se (as cordas escondem-nas nos nós).
        for (index, normal) in [(0, -tangents[0]), (count - 1, tangents[count - 1])] {
            let hub = self.vertex(points[index], normal, Vec2::new(tube.uv_offset.x, along[index] / tile), color(points[index]));
            let rim: Vec<u32> = rings[index]
                .iter()
                .map(|point| self.vertex(*point, normal, Vec2::new(tube.uv_offset.x, along[index] / tile), color(*point)))
                .collect();
            for corner in 0..sides {
                self.triangle(hub, rim[(corner + 1) % sides], rim[corner]);
            }
        }
    }

    fn push_stud(&mut self, stud: &Stud) {
        let normal = stud.normal.normalize_or_zero();
        if normal == Vec3::ZERO {
            return;
        }
        let side = perpendicular(normal);
        let up = normal.cross(side);
        let center = stud.at + normal * 0.002;
        let white = [1.0, 1.0, 1.0, 1.0];
        let hub = self.vertex(center + normal * stud.radius * 0.5, normal, Vec2::ZERO, white);
        let rim: Vec<u32> = (0..6)
            .map(|corner| {
                let angle = std::f32::consts::TAU * corner as f32 / 6.0;
                self.vertex(center + (side * angle.cos() + up * angle.sin()) * stud.radius, normal, Vec2::ZERO, white)
            })
            .collect();
        for corner in 0..6 {
            self.triangle(hub, rim[(corner + 1) % 6], rim[corner]);
        }
    }
}

/// As malhas de uma cerca, uma por material, por uma ordem fixa.
pub fn build(layout: &FenceLayout, style: &FenceStyle) -> Vec<(Slot, SurfaceMesh)> {
    let tile = style.tile_m.max(0.05);
    let weather = Weather {
        height: style.weather_height.max(0.01),
        tint: Vec3::new(
            style.weather_tint.to_linear().red,
            style.weather_tint.to_linear().green,
            style.weather_tint.to_linear().blue,
        ),
    };
    let mut meshes: HashMap<Slot, SurfaceMesh> = HashMap::new();
    for beam in &layout.beams {
        meshes.entry(beam.slot).or_default().push_beam(beam, tile, &weather);
    }
    for tube in &layout.tubes {
        meshes.entry(tube.slot).or_default().push_tube(tube, tile, &weather);
    }
    for stud in &layout.studs {
        meshes.entry(Slot::Iron).or_default().push_stud(stud);
    }
    [Slot::Primary, Slot::Secondary, Slot::Iron]
        .into_iter()
        .filter_map(|slot| {
            let mesh = meshes.remove(&slot)?;
            (!mesh.is_empty()).then_some((slot, mesh))
        })
        .collect()
}

fn uv_at(perimeter: &[f32], index: usize) -> f32 {
    perimeter[index.min(perimeter.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;
    use crate::layout::{FenceFrame, Role, Rng, layout};
    use crate::specs::{FenceKind, FenceStyle, TimberShape, gate, pasture_rail, presets, resolve, rope};

    fn flat(_: f32, _: f32) -> Option<f32> {
        Some(0.0)
    }

    fn fence_of(style: &FenceStyle, points: &str) -> FenceLayout {
        let spec = bevy_ext_scene::FenceSpec {
            points: points.into(),
            ..Default::default()
        };
        layout(&spec, &resolve(&spec, style), &FenceFrame::default(), &flat, 3).expect("layout")
    }

    fn meshes_of(style: &FenceStyle, points: &str) -> Vec<(Slot, SurfaceMesh)> {
        let fence = fence_of(style, points);
        build(&fence, &resolve(&bevy_ext_scene::FenceSpec::default(), style))
    }

    fn check(mesh: &SurfaceMesh) {
        let count = mesh.positions.len();
        assert_eq!(mesh.normals.len(), count);
        assert_eq!(mesh.uvs.len(), count);
        assert_eq!(mesh.colors.len(), count);
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(mesh.positions.iter().all(|value| value.is_finite()));
        assert!(mesh.normals.iter().all(|value| value.is_finite()));
        assert!(mesh.uvs.iter().all(|value| value.is_finite()));
        for index in &mesh.indices {
            assert!((*index as usize) < count, "índice fora da malha");
        }
        for normal in &mesh.normals {
            assert!((normal.length() - 1.0).abs() < 1e-3, "{normal}");
        }
        for color in &mesh.colors {
            assert_eq!(color[3], 1.0);
            assert!(color[0] >= 0.0 && color[0] <= 1.2, "{color:?}");
        }
        for [a, b, c] in mesh.indices.as_chunks::<3>().0 {
            let (a, b, c) = (mesh.positions[*a as usize], mesh.positions[*b as usize], mesh.positions[*c as usize]);
            assert!((b - a).cross(c - a).length() > 1e-9, "triângulo degenerado");
        }
    }

    #[test]
    fn a_secao_talhada_tem_oito_facetas() {
        let hewn = profile(&crate::specs::Timber::hewn(0.1, 0.1, 0.2));
        assert_eq!(hewn.len(), 8);
        // O chanfro é 20% de meio lado: 1 cm de corte em cada canto.
        assert!((hewn[2].x - 0.05).abs() < 1e-6 && (hewn[2].y + 0.04).abs() < 1e-6);
        let square = profile(&crate::specs::Timber::hewn(0.1, 0.07, 0.0));
        assert_eq!(square.len(), 4);
        assert!((square[1].x - 0.05).abs() < 1e-6 && (square[1].y + 0.035).abs() < 1e-6);
        let round = profile(&crate::specs::Timber::round(0.14, 8));
        assert_eq!(round.len(), 8);
        assert!(round.iter().all(|point| (point.length() - 0.07).abs() < 1e-6));
        // Menos de três lados vira três; mais de dezasseis fica em dezasseis.
        assert_eq!(profile(&crate::specs::Timber::round(0.1, 0)).len(), 3);
        assert_eq!(profile(&crate::specs::Timber::round(0.1, 40)).len(), 16);
    }

    #[test]
    fn todas_as_predefinicoes_dao_malhas_validas() {
        for style in presets() {
            let spec = bevy_ext_scene::FenceSpec {
                points: "0 0 2.4 0 2.4 2.4".into(),
                ..Default::default()
            };
            let fence = layout(&spec, &resolve(&spec, &style), &FenceFrame::default(), &flat, 7)
                .unwrap_or_else(|error| panic!("{}: {error}", style.key));
            let meshes = build(&fence, &style);
            assert!(!meshes.is_empty(), "{}", style.key);
            let faces: usize = meshes.iter().map(|(_, mesh)| mesh.indices.len() / 3).sum();
            assert!(faces >= 150, "{}: {faces} triângulos", style.key);
            assert!(faces < 60_000, "{}: {faces} triângulos", style.key);
            for (_, mesh) in &meshes {
                check(mesh);
            }
        }
    }

    #[test]
    fn uma_malha_por_material() {
        let slots = |meshes: &[(Slot, SurfaceMesh)]| meshes.iter().map(|(slot, _)| *slot).collect::<Vec<_>>();
        assert_eq!(slots(&meshes_of(&rope(), "0 0 2 0")), vec![Slot::Primary, Slot::Secondary]);
        assert_eq!(slots(&meshes_of(&gate(), "0 0 2.6 0")), vec![Slot::Primary, Slot::Iron]);
        // Sem ferragens nem segundo material, há uma malha só.
        assert_eq!(meshes_of(&pasture_rail(), "0 0 3 0").len(), 1);
    }

    #[test]
    fn o_veio_corre_ao_longo_da_peca_em_metros() {
        let style = pasture_rail();
        let tile = style.tile_m;
        let weather = Weather {
            height: 0.28,
            tint: Vec3::splat(0.6),
        };
        // Uma travessa de 3 m, sem inclinação: a face de cima tem a normal +Y.
        let timber = crate::specs::Timber::hewn(0.10, 0.07, 0.14);
        let beam = Beam {
            from: Vec3::new(0.0, 0.5, 0.0),
            to: Vec3::new(3.0, 0.5, 0.0),
            across: Vec3::Y,
            timber,
            end_scale: Vec2::ONE,
            apex: 0.0,
            caps: [true; 2],
            role: Role::Rail,
            slot: Slot::Primary,
            uv_offset: Vec2::new(0.25, 0.5),
            tint: Vec3::ONE,
            ground_y: -10.0,
        };
        let mut mesh = SurfaceMesh::default();
        mesh.push_beam(&beam, tile, &weather);
        let top: Vec<(f32, f32, f32)> = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .zip(&mesh.uvs)
            .filter(|((_, normal), _)| normal.y > 0.999)
            .map(|((position, _), uv)| (position.x, uv.y, uv.x))
            .collect();
        assert_eq!(top.len(), 6, "a face de cima são dois triângulos, cada um com os seus vértices");
        let range = |values: Vec<f32>| {
            let (low, high) = (values.iter().copied().fold(f32::MAX, f32::min), values.iter().copied().fold(f32::MIN, f32::max));
            high - low
        };
        // V segue o comprimento (3 m ÷ tile), a partir do desvio da peça.
        assert!((range(top.iter().map(|s| s.1).collect()) - 3.0 / tile).abs() < 1e-4);
        assert!((top.iter().map(|s| s.1).fold(f32::MAX, f32::min) - 0.5).abs() < 1e-4);
        // U é a largura da face de cima (o lado menos os dois chanfros), à volta do perfil.
        let cut = timber.chamfer * timber.width.min(timber.depth) * 0.5;
        let face_width = timber.depth - 2.0 * cut;
        assert!((range(top.iter().map(|s| s.2).collect()) - face_width / tile).abs() < 1e-4);
        // e V cresce com o x.
        for (x0, v0, _) in &top {
            for (x1, v1, _) in &top {
                assert!((x1 - x0) * (v1 - v0) >= -1e-6, "o veio acompanha o comprimento");
            }
        }
        // Duas peças iguais com desvios diferentes não mostram o mesmo veio.
        let mut other = SurfaceMesh::default();
        let shifted = Beam {
            uv_offset: Vec2::new(0.6, 0.1),
            ..beam.clone()
        };
        other.push_beam(&shifted, tile, &weather);
        assert_ne!(mesh.uvs, other.uvs);
    }

    #[test]
    fn a_base_escurece_junto_ao_chao() {
        let style = pasture_rail();
        let fence = fence_of(&style, "0 0 3 0");
        let (_, mesh) = &build(&fence, &style)[0];
        let tint = style.weather_tint.to_linear();
        // O pé enterrado dos postes leva o desgaste por inteiro.
        let feet: Vec<&Rgba> = mesh
            .colors
            .iter()
            .zip(&mesh.positions)
            .filter(|(_, position)| (position.y + style.bury).abs() < 1e-4)
            .map(|(color, _)| color)
            .collect();
        assert!(feet.len() >= 16, "{}", feet.len());
        let foot = feet.iter().map(|color| color[0]).fold(f32::MAX, f32::min);
        assert!(foot < tint.red * 1.1, "pé {foot} contra a tinta {}", tint.red);
        // Longe do chão o tom é o da peça, sem desgaste.
        let heads: Vec<f32> = mesh
            .colors
            .iter()
            .zip(&mesh.positions)
            .filter(|(_, position)| position.y > style.height * 0.75)
            .map(|(color, _)| color[0])
            .collect();
        assert!(!heads.is_empty());
        assert!(heads.iter().all(|value| *value > tint.red * 2.0), "{heads:?}");
    }

    #[test]
    fn as_tampas_sao_o_topo_cortado() {
        let weather = Weather {
            height: 0.28,
            tint: Vec3::splat(0.6),
        };
        let beam = Beam {
            from: Vec3::ZERO,
            to: Vec3::Y,
            across: Vec3::X,
            timber: crate::specs::Timber::hewn(0.08, 0.02, 0.1),
            end_scale: Vec2::ONE,
            apex: 0.0,
            caps: [true; 2],
            role: Role::Board,
            slot: Slot::Primary,
            uv_offset: Vec2::ZERO,
            tint: Vec3::ONE,
            ground_y: -10.0,
        };
        let mut mesh = SurfaceMesh::default();
        mesh.push_beam(&beam, 1.2, &weather);
        check(&mesh);
        // A tampa é a face com a normal exactamente para cima (os chanfros têm 0,92).
        let cap = mesh.colors.iter().zip(&mesh.normals).find(|(_, normal)| normal.y > 0.999).expect("tampa de cima").0;
        assert!((cap[0] - END_GRAIN).abs() < 1e-4, "{cap:?}");
    }

    #[test]
    fn uma_ponta_de_estaca_fecha_sem_faces_degeneradas() {
        let mut beam = Beam {
            from: Vec3::ZERO,
            to: Vec3::Y,
            across: Vec3::X,
            timber: crate::specs::Timber::hewn(0.08, 0.02, 0.1),
            end_scale: Vec2::new(0.0, 0.9),
            apex: 0.0,
            caps: [false, false],
            role: Role::Board,
            slot: Slot::Primary,
            uv_offset: Vec2::ZERO,
            tint: Vec3::ONE,
            ground_y: 0.0,
        };
        let weather = Weather {
            height: 0.28,
            tint: Vec3::splat(0.6),
        };
        let mut mesh = SurfaceMesh::default();
        mesh.push_beam(&beam, 1.2, &weather);
        check(&mesh);
        assert!(mesh.positions.iter().any(|point| (point.y - 1.0).abs() < 1e-5));
        // Com tampa e pirâmide, o topo ganha facetas a mais.
        beam.end_scale = Vec2::splat(0.7);
        beam.apex = 0.05;
        beam.caps = [true, false];
        let mut capped = SurfaceMesh::default();
        capped.push_beam(&beam, 1.2, &weather);
        check(&capped);
        assert!(capped.indices.len() > mesh.indices.len());
        assert!(capped.positions.iter().any(|point| point.y > 1.04));
        // A malha do Bevy tem os mesmos atributos.
        let bevy_mesh = capped.to_mesh();
        assert_eq!(bevy_mesh.indices().unwrap().len(), capped.indices.len());
        assert!(matches!(bevy_mesh.attribute(Mesh::ATTRIBUTE_COLOR), Some(VertexAttributeValues::Float32x4(_))));
    }

    #[test]
    fn a_curva_da_moldura_nao_parte_o_tubo() {
        let fence = fence_of(&rope(), "0 0 2 0");
        let tube = &fence.tubes[0];
        let mut mesh = SurfaceMesh::default();
        mesh.push_tube(
            tube,
            1.2,
            &Weather {
                height: 0.28,
                tint: Vec3::splat(0.6),
            },
        );
        check(&mesh);
        assert_eq!(mesh.indices.len() / 3, (tube.path.len() - 1) * tube.sides as usize * 2 + tube.sides as usize * 2);
    }

    #[test]
    fn o_rng_e_deterministico_e_uniforme() {
        assert_eq!(Rng::new(9).next_u64(), Rng::new(9).next_u64());
        let mut rng = Rng::new(1);
        let mean = (0..10_000).map(|_| rng.unit()).sum::<f32>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
        let mut rng = Rng::new(2);
        assert!((0..1000).all(|_| {
            let value = rng.sym(3.0);
            (-3.0..=3.0).contains(&value)
        }));
    }

    #[test]
    fn um_tronco_sem_chanfro_e_um_poligono() {
        let timber = crate::specs::Timber {
            shape: TimberShape::Round,
            sides: 0,
            ..crate::specs::Timber::round(0.1, 0)
        };
        assert_eq!(profile(&timber).len(), 3, "menos de três lados vira três");
        assert_eq!(FenceKind::from_key("rail"), Some(FenceKind::Rail));
    }
}
