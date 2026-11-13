//! Lenient attribute parsing for the editor preview.
//!
//! The game parser (`viber::recipes`) validates and warns; this one only needs
//! to build a preview mesh, so bad/missing values fall back to the same
//! defaults the DSL uses. Attribute names are the contract between both.
//!
//! The accessor returns `Option<String>` because the editor reads `WorldNode`
//! attrs through reflection (each dylib owns its copy of the type); the game
//! parser feeds [`Shape`]/[`MaterialSpec`] directly from its typed attributes.

use super::material::{MaterialSpec, Preset};
use super::shape::Shape;

/// Every tag [`shape_from_attrs`] understands (lowercase).
pub const PRIMITIVE_TAGS: &[&str] = &[
    "box",
    "cuboid",
    "sphere",
    "cylinder",
    "capsule",
    "plane",
    "cone",
    "pyramid",
    "torus",
    "lathe",
    "sweep",
    "extruded",
    "extrudedpolygon",
    "extruded-polygon",
];

/// Whether `tag` is an authored primitive.
pub fn is_primitive_tag(tag: &str) -> bool {
    let tag = tag.to_ascii_lowercase();
    PRIMITIVE_TAGS.contains(&tag.as_str())
}

/// Parses a primitive from its tag and attributes.
///
/// Returns `None` for tags outside [`PRIMITIVE_TAGS`] or for procedural shapes
/// whose geometry attributes are missing (a lathe without `profile`, a sweep
/// without `profile`/`path`, an extruded polygon without `points`).
pub fn shape_from_attrs(tag: &str, attr: impl Fn(&str) -> Option<String>) -> Option<Shape> {
    let lower = tag.to_ascii_lowercase();
    let f = |key: &str, default: f32| {
        attr(key)
            .and_then(|value| value.trim().parse::<f32>().ok())
            .unwrap_or(default)
    };
    let roundness = || f("round", 0.0).clamp(0.0, 1.0);
    Some(match lower.as_str() {
        "box" | "cuboid" => {
            let half_size = attr("half-size")
                .and_then(|value| parse_vec3(&value))
                .unwrap_or([0.5; 3]);
            let round = roundness();
            if round > 0.0 {
                Shape::RoundedCuboid {
                    half_size,
                    roundness: round,
                }
            } else {
                Shape::Cuboid { half_size }
            }
        }
        "sphere" => Shape::Sphere {
            radius: f("radius", 0.5),
        },
        "cylinder" => Shape::Cylinder {
            half_height: f("half-height", 0.5),
            radius: f("radius", 0.5),
        },
        "capsule" => Shape::Capsule {
            radius: f("radius", 0.5),
            half_height: f("half-height", 0.5),
        },
        "plane" => Shape::Plane {
            half_size: attr("half-size")
                .and_then(|value| parse_vec2(&value))
                .unwrap_or([1.0; 2]),
        },
        "cone" => Shape::Cone {
            half_height: f("half-height", 0.5),
            radius: f("radius", 0.5),
            roundness: roundness(),
        },
        "pyramid" => Shape::Pyramid {
            sides: attr("sides")
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(4)
                .max(3),
            radius: f("radius", 0.5),
            half_height: f("half-height", 0.5),
            roundness: roundness(),
        },
        "torus" => Shape::Torus {
            radius: f("radius", 0.5),
            thickness: f("thickness", 0.15),
            start_deg: f("start", 0.0),
            span_deg: f("span", 360.0),
        },
        "lathe" => {
            let profile = attr("profile").and_then(|value| parse_points2(&value))?;
            let segments = attr("segments")
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(24)
                .max(3);
            Shape::Lathe { profile, segments }
        }
        "sweep" => {
            let profile = attr("profile").and_then(|value| parse_points2(&value))?;
            let path = attr("path").and_then(|value| parse_points3(&value))?;
            Shape::Sweep {
                profile,
                path,
                closed: attr("closed").is_some_and(|value| parse_bool(&value)),
                caps: attr("caps").is_none_or(|value| parse_bool(&value)),
            }
        }
        "extruded" | "extrudedpolygon" | "extruded-polygon" => {
            let outer = attr("points")
                .or_else(|| attr("outer"))
                .and_then(|value| parse_points2(&value))?;
            let holes = attr("holes")
                .map(|value| parse_contours(&value))
                .unwrap_or_default();
            Shape::Extruded {
                outer,
                holes,
                depth: f("depth", 0.5),
                roundness: roundness(),
            }
        }
        _ => return None,
    })
}

/// Parses the material fields (`base-color`, `roughness`, `preset`, …).
pub fn material_from_attrs(attr: impl Fn(&str) -> Option<String>) -> MaterialSpec {
    let mut material = MaterialSpec::default();
    if let Some(value) = attr("base-color") {
        material.base_color = parse_color(&value);
    }
    if let Some(value) = attr("metallic") {
        material.metallic = value.trim().parse().ok();
    }
    if let Some(value) = attr("roughness") {
        material.roughness = value.trim().parse().ok();
    }
    if let Some(value) = attr("opacity") {
        material.opacity = value.trim().parse::<f32>().ok().map(|v| v.clamp(0.0, 1.0));
    }
    if let Some(value) = attr("emissive").or_else(|| attr("emissive-color")) {
        material.emissive = parse_color(&value);
    }
    material.texture = attr("texture").or_else(|| attr("texture-url"));
    material.texture_tile = attr("texture-tile-size").and_then(|value| value.trim().parse().ok());
    material.normal_map = attr("normal-map").or_else(|| attr("normal-map-url"));
    material.preset = attr("preset")
        .or_else(|| attr("style"))
        .and_then(|value| Preset::from_name(&value));
    material
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa` or a few common names → sRGB 0..=1.
pub fn parse_color(value: &str) -> Option<[f32; 3]> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .filter_map(|c| c.to_digit(16).map(|d| d as u8))
            .collect();
        match digits.len() {
            3 => Some([
                digits[0] as f32 / 15.0,
                digits[1] as f32 / 15.0,
                digits[2] as f32 / 15.0,
            ]),
            6 | 8 => Some([
                (digits[0] * 16 + digits[1]) as f32 / 255.0,
                (digits[2] * 16 + digits[3]) as f32 / 255.0,
                (digits[4] * 16 + digits[5]) as f32 / 255.0,
            ]),
            _ => None,
        }
    } else {
        Some(match value.to_ascii_lowercase().as_str() {
            "white" => [1.0, 1.0, 1.0],
            "black" => [0.0, 0.0, 0.0],
            "red" => [1.0, 0.0, 0.0],
            "green" => [0.0, 0.5, 0.0],
            "blue" => [0.0, 0.0, 1.0],
            "yellow" => [1.0, 1.0, 0.0],
            "orange" => [1.0, 0.5, 0.0],
            "brown" => [0.35, 0.2, 0.08],
            "gray" | "grey" => [0.5, 0.5, 0.5],
            "silver" => [0.75, 0.75, 0.75],
            "gold" => [1.0, 0.84, 0.0],
            "purple" => [0.5, 0.0, 0.5],
            "cyan" => [0.0, 1.0, 1.0],
            "magenta" => [1.0, 0.0, 1.0],
            _ => return None,
        })
    }
}

fn parse_vec2(value: &str) -> Option<[f32; 2]> {
    let mut parts = value.split_whitespace();
    Some([parts.next()?.parse().ok()?, parts.next()?.parse().ok()?])
}

fn parse_vec3(value: &str) -> Option<[f32; 3]> {
    let mut parts = value.split_whitespace();
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

/// `"x y; …"` or a flat `"x y x y …"` → points.
pub fn parse_points2(value: &str) -> Option<Vec<[f32; 2]>> {
    if value.contains(';') {
        let points: Vec<[f32; 2]> = value
            .split(';')
            .filter_map(|pair| {
                let pair = pair.trim();
                (!pair.is_empty()).then(|| parse_vec2(pair)).flatten()
            })
            .collect();
        return (!points.is_empty()).then_some(points);
    }
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() < 2 || !parts.len().is_multiple_of(2) {
        return None;
    }
    parts
        .chunks(2)
        .map(|pair| Some([pair[0].parse().ok()?, pair[1].parse().ok()?]))
        .collect()
}

/// `"x y z; …"` or a flat `"x y z x y z …"` → path points.
pub fn parse_points3(value: &str) -> Option<Vec<[f32; 3]>> {
    if value.contains(';') {
        let points: Vec<[f32; 3]> = value
            .split(';')
            .filter_map(|point| {
                let point = point.trim();
                (!point.is_empty()).then(|| parse_vec3(point)).flatten()
            })
            .collect();
        return (!points.is_empty()).then_some(points);
    }
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() < 3 || !parts.len().is_multiple_of(3) {
        return None;
    }
    parts
        .chunks(3)
        .map(|triple| {
            Some([
                triple[0].parse().ok()?,
                triple[1].parse().ok()?,
                triple[2].parse().ok()?,
            ])
        })
        .collect()
}

/// `"x y; … | x y; …"` → one contour per hole.
fn parse_contours(value: &str) -> Vec<Vec<[f32; 2]>> {
    value.split('|').filter_map(parse_points2).collect()
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn box_round_upgrades_to_a_rounded_cuboid() {
        let shape = shape_from_attrs("Box", attrs(&[("half-size", "2 1 0.5"), ("round", "0.1")]))
            .expect("box");
        assert!(matches!(shape, Shape::RoundedCuboid { roundness: 0.1, .. }));
    }

    #[test]
    fn procedural_shapes_parse_their_geometry() {
        let lathe = shape_from_attrs(
            "Lathe",
            attrs(&[("profile", "0.5 0 0.4 1 0.3 2"), ("segments", "16")]),
        )
        .expect("lathe");
        assert!(matches!(lathe, Shape::Lathe { segments: 16, .. }));

        let sweep = shape_from_attrs(
            "Sweep",
            attrs(&[
                ("profile", "-1 0 1 0 1 0.2 -1 0.2 -1 0"),
                ("path", "0 0 0 0 0 5 0 1 10"),
            ]),
        )
        .expect("sweep");
        assert!(matches!(sweep, Shape::Sweep { .. }));

        let wall = shape_from_attrs(
            "ExtrudedPolygon",
            attrs(&[
                ("points", "-2 -1 2 -1 2 1 -2 1"),
                ("holes", "-0.5 -0.5 0.5 -0.5 0.5 0.5 -0.5 0.5"),
                ("depth", "0.4"),
            ]),
        )
        .expect("extruded");
        assert!(matches!(wall, Shape::Extruded { depth: 0.4, .. }));
    }

    #[test]
    fn material_parses_colour_and_preset() {
        let material = material_from_attrs(attrs(&[
            ("base-color", "#8a5a30"),
            ("roughness", "0.85"),
            ("preset", "wood"),
        ]));
        assert_eq!(material.base_color, Some([0.5411765, 0.3529412, 0.1882353]));
        assert_eq!(material.roughness, Some(0.85));
        assert_eq!(material.preset, Some(Preset::Wood));
    }

    #[test]
    fn missing_geometry_returns_none() {
        assert!(shape_from_attrs("Lathe", attrs(&[])).is_none());
        assert!(shape_from_attrs("Sweep", attrs(&[("profile", "0 0 1 0")])).is_none());
        assert!(shape_from_attrs("Nope", attrs(&[])).is_none());
    }
}
