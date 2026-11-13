//! Comparação semântica entre o mundo XML e o `.bsn`.
//!
//! Comparar texto de pose não serve: `-180 0 -180` e `180 0 180` são a mesma
//! rotação, e a ida por quaternião (com 6 casas no `.bsn`) muda o último bit.
//! Aqui comparam-se poses, cores e números com tolerância; o resto é exato.

use std::collections::BTreeMap;

use bevy::math::{Quat, Vec3};

use crate::bsn::components::tag_uses_transform;
use crate::bsn::pose::{POSE_KEYS, read_pose};
use crate::xml::XmlNode;
use crate::xml::values::{parse_bool, parse_color, parse_f32, parse_vec3};

const EPS_POS: f32 = 1e-3;
const EPS_ROT: f32 = 1e-4;

/// Compara dois mundos já expandidos. `Err` traz o caminho do primeiro
/// problema.
pub fn worlds_equivalent(
    left_root: &[(String, String)],
    left: &[XmlNode],
    right_root: &[(String, String)],
    right: &[XmlNode],
) -> Result<(), String> {
    if attr_map(left_root) != attr_map(right_root) {
        return Err(format!(
            "root attrs: {:?} != {:?}",
            attr_map(left_root),
            attr_map(right_root)
        ));
    }
    nodes_equivalent(left, right, "world")
}

fn nodes_equivalent(left: &[XmlNode], right: &[XmlNode], path: &str) -> Result<(), String> {
    if left.len() != right.len() {
        return Err(format!(
            "{path}: {} nó(s) != {} nó(s)",
            left.len(),
            right.len()
        ));
    }
    for (index, (left, right)) in left.iter().zip(right).enumerate() {
        let child_path = format!("{path}/{index}:{}", left.tag);
        node_equivalent(left, right, &child_path)?;
    }
    Ok(())
}

fn node_equivalent(left: &XmlNode, right: &XmlNode, path: &str) -> Result<(), String> {
    if !left.tag.eq_ignore_ascii_case(&right.tag) {
        return Err(format!("{path}: tag `{}` != `{}`", left.tag, right.tag));
    }
    if left.text != right.text {
        return Err(format!("{path}: texto {:?} != {:?}", left.text, right.text));
    }
    attrs_equivalent(&left.tag, &left.attrs, &right.attrs, path)?;
    nodes_equivalent(&left.children, &right.children, path)
}

fn attrs_equivalent(
    tag: &str,
    left: &[(String, String)],
    right: &[(String, String)],
    path: &str,
) -> Result<(), String> {
    let tag = tag.to_ascii_lowercase();
    let mut left = attr_map(left);
    let mut right = attr_map(right);

    if tag_uses_transform(&tag) {
        if tag == "directionallight" {
            let left_direction = direction(&left);
            let right_direction = direction(&right);
            if !same_direction(left_direction, right_direction) {
                return Err(format!(
                    "{path}: direction {left_direction:?} != {right_direction:?}"
                ));
            }
            remove(&mut left, "direction");
            remove(&mut right, "direction");
            for key in POSE_KEYS {
                remove(&mut left, key);
                remove(&mut right, key);
            }
        } else {
            let left_pose = read_pose_map(&left, path)?;
            let right_pose = read_pose_map(&right, path)?;
            if !same_pose(left_pose, right_pose) {
                return Err(format!("{path}: pose {:?} != {:?}", left_pose, right_pose));
            }
            for key in POSE_KEYS {
                remove(&mut left, key);
                remove(&mut right, key);
            }
        }
    } else {
        for key in POSE_KEYS {
            remove(&mut left, key);
            remove(&mut right, key);
        }
    }

    match tag.as_str() {
        "pointlight" => compare_light(
            &mut left,
            &mut right,
            path,
            &[
                ("color", LightValue::Color([1.0, 1.0, 1.0])),
                ("intensity", LightValue::Number(1200.0)),
                ("radius", LightValue::Number(0.0)),
                ("shadows", LightValue::Bool(false)),
            ],
        )?,
        "rectlight" => compare_light(
            &mut left,
            &mut right,
            path,
            &[
                ("color", LightValue::Color([1.0, 1.0, 1.0])),
                ("intensity", LightValue::Number(3000.0)),
                ("range", LightValue::Number(12.0)),
                ("width", LightValue::Number(2.0)),
                ("height", LightValue::Number(1.0)),
            ],
        )?,
        "directionallight" => compare_light(
            &mut left,
            &mut right,
            path,
            &[
                ("color", LightValue::Color([1.0, 1.0, 1.0])),
                ("illuminance", LightValue::Number(10_000.0)),
                ("shadows", LightValue::Bool(true)),
            ],
        )?,
        _ => {}
    }

    if let (Some(left_url), Some(right_url)) = (left.get("url"), right.get("url")) {
        if left_url.trim_start_matches('/') != right_url.trim_start_matches('/') {
            return Err(format!("{path}: url {left_url} != {right_url}"));
        }
        remove(&mut left, "url");
        remove(&mut right, "url");
    }

    #[cfg(feature = "terrain")]
    if matches!(
        tag.as_str(),
        "cave" | "chamber" | "shaft" | "arch" | "cliff"
    ) {
        terrain_attrs_equivalent(&tag, &mut left, &mut right, path)?;
    }

    if left != right {
        return Err(format!("{path}: attrs {left:?} != {right:?}"));
    }
    Ok(())
}

/// Attrs de features de terreno: um attr ausente vale o default do spec e os
/// valores comparam-se numericamente — a forma canónica do componente pode
/// mudar `"2.0"` para `"2"` sem mudar o mundo que o jogo carrega.
#[cfg(feature = "terrain")]
pub(crate) fn terrain_attrs_equivalent(
    tag: &str,
    left: &mut BTreeMap<String, String>,
    right: &mut BTreeMap<String, String>,
    path: &str,
) -> Result<(), String> {
    // O DSL do arco aceita `width` e `span` para o mesmo campo do spec.
    if tag == "arch" {
        let normalize = |map: &mut BTreeMap<String, String>| {
            if let Some(width) = map.remove("width") {
                map.entry("span".to_string()).or_insert(width);
            }
        };
        normalize(left);
        normalize(right);
    }
    let mut keys: Vec<String> = left.keys().chain(right.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let resolve = |map: &BTreeMap<String, String>| {
            map.get(&key)
                .cloned()
                .or_else(|| bevy_ext_terrain::feature_attr_default(tag, &key))
        };
        let (Some(left_value), Some(right_value)) = (resolve(left), resolve(right)) else {
            return Err(format!("{path}: `{key}` só existe de um dos lados"));
        };
        if !terrain_values_equal(&left_value, &right_value) {
            return Err(format!("{path}: {key} {left_value:?} != {right_value:?}"));
        }
        remove(left, &key);
        remove(right, &key);
    }
    Ok(())
}

/// Booleanos (`parse_bool`: `1`/`true`/`yes`/`on`…) comparam como bools e
/// listas de números comparam por valor (tolerância de pose); o resto é
/// exacto.
#[cfg(feature = "terrain")]
fn terrain_values_equal(left: &str, right: &str) -> bool {
    let as_bool = |text: &str| match text.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    };
    if let (Some(left), Some(right)) = (as_bool(left), as_bool(right)) {
        return left == right;
    }
    let numbers = |text: &str| -> Option<Vec<f32>> {
        text.split_whitespace()
            .map(|part| part.parse::<f32>().ok())
            .collect()
    };
    match (numbers(left), numbers(right)) {
        (Some(left), Some(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(&right)
                    .all(|(a, b)| (a - b).abs() <= EPS_POS)
        }
        _ => left == right,
    }
}

enum LightValue {
    Color([f32; 3]),
    Number(f32),
    Bool(bool),
}

fn compare_light(
    left: &mut BTreeMap<String, String>,
    right: &mut BTreeMap<String, String>,
    path: &str,
    fields: &[(&str, LightValue)],
) -> Result<(), String> {
    for (key, default) in fields {
        let left_value = left.get(*key);
        let right_value = right.get(*key);
        let equal = match default {
            LightValue::Color(default) => {
                let left = match left_value {
                    Some(value) => {
                        parse_color(value, path).map_err(|err| format!("{path}: {err}"))?
                    }
                    None => *default,
                };
                let right = match right_value {
                    Some(value) => {
                        parse_color(value, path).map_err(|err| format!("{path}: {err}"))?
                    }
                    None => *default,
                };
                left.iter()
                    .zip(right.iter())
                    .all(|(left, right)| (left - right).abs() <= EPS_POS)
            }
            LightValue::Number(default) => {
                let left = match left_value {
                    Some(value) => {
                        parse_f32(value, path).map_err(|err| format!("{path}: {err}"))?
                    }
                    None => *default,
                };
                let right = match right_value {
                    Some(value) => {
                        parse_f32(value, path).map_err(|err| format!("{path}: {err}"))?
                    }
                    None => *default,
                };
                (left - right).abs() <= EPS_POS
            }
            LightValue::Bool(default) => {
                let left = match left_value {
                    Some(value) => {
                        parse_bool(value, path).map_err(|err| format!("{path}: {err}"))?
                    }
                    None => *default,
                };
                let right = match right_value {
                    Some(value) => {
                        parse_bool(value, path).map_err(|err| format!("{path}: {err}"))?
                    }
                    None => *default,
                };
                left == right
            }
        };
        if !equal {
            return Err(format!("{path}: {key} {left_value:?} != {right_value:?}"));
        }
        remove(left, key);
        remove(right, key);
    }
    Ok(())
}

fn read_pose_map(attrs: &BTreeMap<String, String>, path: &str) -> Result<crate::bsn::Pose, String> {
    let attrs: Vec<(String, String)> = attrs
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    read_pose(&attrs).map_err(|err| format!("{path}: {err}"))
}

fn same_pose(left: crate::bsn::Pose, right: crate::bsn::Pose) -> bool {
    (left.translation - right.translation).length() <= EPS_POS
        && (left.scale - right.scale).length() <= EPS_POS
        && same_rotation(left.rotation, right.rotation)
}

fn same_rotation(left: Quat, right: Quat) -> bool {
    // q e -q são a mesma rotação.
    1.0 - left.dot(right).abs() <= EPS_ROT
}

fn direction(attrs: &BTreeMap<String, String>) -> Vec3 {
    let value = attrs
        .get("direction")
        .map(|value| value.as_str())
        .unwrap_or("-1 -1 -1");
    let direction = parse_vec3(value, "direction")
        .map(Vec3::from)
        .unwrap_or(Vec3::new(-1.0, -1.0, -1.0));
    if direction.length_squared() > f32::EPSILON {
        direction.normalize()
    } else {
        Vec3::NEG_Y
    }
}

fn same_direction(left: Vec3, right: Vec3) -> bool {
    left.dot(right) >= 1.0 - EPS_ROT
}

fn attr_map(attrs: &[(String, String)]) -> BTreeMap<String, String> {
    attrs.iter().cloned().collect()
}

fn remove(map: &mut BTreeMap<String, String>, key: &str) {
    map.remove(key);
}
