//! Contrato IR das receitas do mundo — os tipos que o parser do monólito
//! produz e que os crates extraídos (`bevy_ext_physics`, `bevy_ext_engine`)
//! consomem: a primitiva [`Shape`], a [`TransformSpec`] local e o parser de
//! strings de componente ([`parse_component_string`]).
//!
//! São tipos de dados puros (sem ECS nem reflection): o monólito reexporta-os
//! (`viber::recipes::{Shape, TransformSpec, parse_component_string}`) e o
//! physics usa-os para derivar colisores exatos de primitivas.

/// Local transform of an entity (world transform comes from the hierarchy).
#[derive(Debug, Clone)]
pub struct TransformSpec {
    pub translation: [f32; 3],
    /// Degrees, XYZ order (`euler="x y z"`).
    pub euler_deg: Option<[f32; 3]>,
    /// Raw quaternion `x y z w` — wins over `euler` when both are present.
    pub rotation_quat: Option<[f32; 4]>,
    pub scale: [f32; 3],
}

impl Default for TransformSpec {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            euler_deg: None,
            rotation_quat: None,
            scale: [1.0; 3],
        }
    }
}

/// Mesh primitive shapes, mirroring the `bevy::math::primitives` constructors.
#[derive(Debug, Clone)]
pub enum Shape {
    /// `Cuboid` — half extents per axis.
    Cuboid { half_size: [f32; 3] },
    /// `Sphere` — radius.
    Sphere { radius: f32 },
    /// `Cylinder` — half height + radius.
    Cylinder { half_height: f32, radius: f32 },
    /// `Plane3d` — half size on the XZ plane.
    Plane { half_size: [f32; 2] },
    /// `Capsule` — radius + half height (vertical).
    Capsule { radius: f32, half_height: f32 },
}

/// Parse a component string (`"pos: 0 1 0; preset: fire"`) into
/// key/value pairs; keys are lowercased and trimmed.
pub fn parse_component_string(value: &str) -> Vec<(String, String)> {
    value
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (key, val) = part.split_once(':')?;
            Some((key.trim().to_ascii_lowercase(), val.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_component_string_splits_and_normalizes() {
        let pairs = parse_component_string("Shape: Box; size: 0.8 0.8 0.8;  ; trigger: TRUE");
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0], ("shape".to_string(), "Box".to_string()));
        assert_eq!(pairs[1], ("size".to_string(), "0.8 0.8 0.8".to_string()));
        assert_eq!(pairs[2], ("trigger".to_string(), "TRUE".to_string()));
    }

    #[test]
    fn test_parse_component_string_skips_parts_without_colon() {
        assert!(parse_component_string("none").is_empty());
        assert!(parse_component_string("").is_empty());
        let pairs = parse_component_string("auto; shape: mesh");
        assert_eq!(pairs, vec![("shape".to_string(), "mesh".to_string())]);
    }

    #[test]
    fn test_transform_spec_default_is_identity() {
        let spec = TransformSpec::default();
        assert_eq!(spec.translation, [0.0; 3]);
        assert_eq!(spec.scale, [1.0; 3]);
        assert_eq!(spec.euler_deg, None);
        assert_eq!(spec.rotation_quat, None);
    }
}
