//! Quantized vertex attributes → the packed `f32` streams `Mesh` demands.
//!
//! Bevy's glTF loader passes POSITION / NORMAL / TANGENT through unconverted
//! and then checks them against the target attribute, which is `Float32x3` /
//! `Float32x4`. A quantized tangent arrives as `Snorm8x4`, gets dropped, and
//! `Mesh::insert_attribute` panics on the missing attribute
//! (`bevy_mesh-0.20.1/src/mesh.rs:471`) — so this conversion is not optional.
//!
//! Split in two so the decoder can lay out the whole binary chunk before it
//! allocates: [`plan`] decides which accessors convert and where their output
//! lands, [`apply`] does the arithmetic. The data is written into a
//! preallocated slice instead of being appended to a growing `Vec` four bytes
//! at a time, which is where most of the old cost lived.

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// glTF accessor component types.
pub const COMP_BYTE: u64 = 5120;
pub const COMP_UNSIGNED_BYTE: u64 = 5121;
pub const COMP_SHORT: u64 = 5122;
pub const COMP_UNSIGNED_SHORT: u64 = 5123;
pub const COMP_FLOAT: u64 = 5126;

/// Vertex semantics Bevy insists on receiving as `f32`.
///
/// Texture coordinates and joint weights have their own conversion paths in
/// Bevy and are left alone.
pub const FLOAT_SEMANTICS: [&str; 3] = ["POSITION", "NORMAL", "TANGENT"];

/// Number of components in a glTF accessor `type`.
fn components_of(kind: &str) -> Option<usize> {
    match kind {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" => Some(4),
        _ => None,
    }
}

/// Bytes per component of a glTF accessor component type.
fn component_size(component_type: u64) -> usize {
    match component_type {
        COMP_BYTE | COMP_UNSIGNED_BYTE => 1,
        COMP_SHORT | COMP_UNSIGNED_SHORT => 2,
        _ => 4,
    }
}

/// One accessor to widen, with both of its ranges resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Job {
    /// Accessor index in the document (for messages).
    pub accessor: usize,
    /// First byte of the accessor's data, absolute in the pre-widen buffer.
    pub read_base: usize,
    /// Distance between consecutive elements.
    pub stride: usize,
    pub count: usize,
    /// Components per element.
    pub kind: usize,
    pub component_type: u64,
    pub normalized: bool,
    /// Where the packed `f32` output starts, absolute in the final buffer.
    pub out_offset: usize,
}

impl Job {
    /// Bytes of packed `f32` this accessor produces.
    pub fn out_len(&self) -> usize {
        self.count * self.kind * 4
    }
}

/// What [`plan`] decided: the conversions, and the binary length they imply.
#[derive(Debug, Default)]
pub struct Plan {
    pub jobs: Vec<Job>,
    /// Length of the whole binary chunk once the outputs are appended.
    pub out_len: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

/// Finds the accessors that need widening, rewrites their JSON, appends one
/// packed buffer view per accessor and returns the ranges to convert.
///
/// `bin_len` is the binary chunk length *before* the new blocks; the outputs
/// start at the next 4-byte boundary after it, in accessor order.
pub fn plan(doc: &mut Value, bin_len: usize) -> Result<Plan> {
    let mut wanted: Vec<usize> = Vec::new();
    for mesh in doc["meshes"].as_array().into_iter().flatten() {
        for primitive in mesh["primitives"].as_array().into_iter().flatten() {
            let Some(attributes) = primitive["attributes"].as_object() else {
                continue;
            };
            for (semantic, index) in attributes {
                if !FLOAT_SEMANTICS.contains(&semantic.as_str()) {
                    continue;
                }
                if let Some(index) = index.as_u64() {
                    wanted.push(index as usize);
                }
            }
        }
    }
    wanted.sort_unstable();
    wanted.dedup();

    let mut plan = Plan {
        jobs: Vec::new(),
        out_len: bin_len,
    };
    for index in wanted {
        let Some(accessor) = doc["accessors"].get(index) else {
            continue;
        };
        let component_type = accessor["componentType"].as_u64().unwrap_or(COMP_FLOAT);
        if component_type == COMP_FLOAT {
            continue; // already float
        }
        let Some(kind) = accessor["type"].as_str().and_then(components_of) else {
            continue;
        };
        let count = accessor["count"].as_u64().unwrap_or(0) as usize;
        let Some(view_index) = accessor["bufferView"].as_u64() else {
            continue;
        };
        let Some(view) = doc["bufferViews"].get(view_index as usize) else {
            continue;
        };
        let normalized = accessor["normalized"].as_bool().unwrap_or(false);
        let size = component_size(component_type);
        let view_offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let accessor_offset = accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        // A view without an explicit stride is tightly packed. `byteStride: 0`
        // is invalid glTF; without the filter a zero stride left `count`
        // unbounded against the buffer.
        let stride = view["byteStride"]
            .as_u64()
            .map(|s| s as usize)
            .filter(|s| *s > 0)
            .unwrap_or(size * kind);
        let read_base = view_offset
            .checked_add(accessor_offset)
            .context("accessor offset overflows the buffer")?;

        let needed = stride
            .checked_mul(count.saturating_sub(1))
            .and_then(|tail| tail.checked_add(read_base))
            .and_then(|tail| tail.checked_add(size * kind))
            .context("accessor range overflows the buffer")?;
        if needed > bin_len {
            bail!("accessor {index}: reads past the buffer ({needed} > {bin_len})");
        }

        let job = Job {
            accessor: index,
            read_base,
            stride,
            count,
            kind,
            component_type,
            normalized,
            out_offset: crate::glb::align4(plan.out_len),
        };
        plan.out_len = job.out_offset + job.out_len();
        plan.jobs.push(job);

        // Point the accessor at a new packed view.
        let new_index = {
            let array = doc["bufferViews"]
                .as_array_mut()
                .context("bufferViews is an array")?;
            array.push(serde_json::json!({
                "buffer": 0,
                "byteOffset": job.out_offset,
                "byteLength": job.out_len(),
            }));
            array.len() - 1
        };
        let accessor = doc["accessors"][index]
            .as_object_mut()
            .context("accessor is an object")?;
        accessor.insert("componentType".into(), serde_json::json!(COMP_FLOAT));
        accessor.insert("bufferView".into(), serde_json::json!(new_index));
        accessor.insert("byteOffset".into(), serde_json::json!(0));
        accessor.remove("normalized");
    }
    Ok(plan)
}

/// Expands every planned accessor.
///
/// `read` is the binary chunk before the new blocks and `write` the region they
/// occupy, starting at `write_base` in the final chunk. The two never overlap:
/// the reads live in the views the decoder just laid out, the writes in the
/// block appended after them.
pub fn apply(jobs: &[Job], read: &[u8], write: &mut [u8], write_base: usize) {
    for job in jobs {
        let out = job.out_offset - write_base;
        debug_assert!(out + job.out_len() <= write.len());
        convert(job, read, &mut write[out..out + job.out_len()]);
    }
}

fn convert(job: &Job, read: &[u8], out: &mut [u8]) {
    let (count, stride, kind) = (job.count, job.stride, job.kind);
    match (job.component_type, kind) {
        (COMP_SHORT, 3) => {
            for element in 0..count {
                let s = job.read_base + element * stride;
                let [a, b, c] = short3(read, s, job.normalized);
                write3(out, element * 12, a, b, c);
            }
        }
        (COMP_UNSIGNED_SHORT, 3) => {
            for element in 0..count {
                let s = job.read_base + element * stride;
                let [a, b, c] = ushort3(read, s, job.normalized);
                write3(out, element * 12, a, b, c);
            }
        }
        (COMP_SHORT, 4) => {
            for element in 0..count {
                let s = job.read_base + element * stride;
                let [a, b, c, d] = short4(read, s, job.normalized);
                write4(out, element * 16, a, b, c, d);
            }
        }
        (COMP_UNSIGNED_SHORT, 4) => {
            for element in 0..count {
                let s = job.read_base + element * stride;
                let [a, b, c, d] = ushort4(read, s, job.normalized);
                write4(out, element * 16, a, b, c, d);
            }
        }
        (COMP_BYTE, 4) => {
            for element in 0..count {
                let s = job.read_base + element * stride;
                let [a, b, c, d] = byte4(read, s, job.normalized);
                write4(out, element * 16, a, b, c, d);
            }
        }
        (COMP_UNSIGNED_BYTE, 4) => {
            for element in 0..count {
                let s = job.read_base + element * stride;
                let [a, b, c, d] = ubyte4(read, s, job.normalized);
                write4(out, element * 16, a, b, c, d);
            }
        }
        _ => {
            let size = component_size(job.component_type);
            for element in 0..count {
                let start = job.read_base + element * stride;
                for component in 0..kind {
                    let at = start + component * size;
                    let value =
                        component_to_f32(&read[at..at + size], job.component_type, job.normalized);
                    let dst = element * kind * 4 + component * 4;
                    out[dst..dst + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
        }
    }
}

/// Reads one integer component and maps it to `f32`.
///
/// A `normalized` accessor stores a fraction of the type's range (glTF spec
/// 3.6.1.2); a plain one stores the value itself, and `KHR_mesh_quantization`
/// leans on the node transform to scale it back. Either way the *meaning* is
/// preserved, so the node's TRS still lands the mesh where it belongs.
#[inline]
fn component_to_f32(bytes: &[u8], component_type: u64, normalized: bool) -> f32 {
    match component_type {
        COMP_BYTE => {
            let v = bytes[0] as i8 as f32;
            if normalized { (v / 127.0).max(-1.0) } else { v }
        }
        COMP_UNSIGNED_BYTE => {
            let v = bytes[0] as f32;
            if normalized { v / 255.0 } else { v }
        }
        COMP_SHORT => {
            let v = i16::from_le_bytes([bytes[0], bytes[1]]) as f32;
            if normalized {
                (v / 32767.0).max(-1.0)
            } else {
                v
            }
        }
        COMP_UNSIGNED_SHORT => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]) as f32;
            if normalized { v / 65535.0 } else { v }
        }
        _ => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    }
}

#[inline]
fn short3(read: &[u8], at: usize, normalized: bool) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (component, slot) in out.iter_mut().enumerate() {
        let at = at + component * 2;
        let v = i16::from_le_bytes([read[at], read[at + 1]]) as f32;
        *slot = if normalized { (v / 32767.0).max(-1.0) } else { v };
    }
    out
}

#[inline]
fn ushort3(read: &[u8], at: usize, normalized: bool) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (component, slot) in out.iter_mut().enumerate() {
        let at = at + component * 2;
        let v = u16::from_le_bytes([read[at], read[at + 1]]) as f32;
        *slot = if normalized { v / 65535.0 } else { v };
    }
    out
}

#[inline]
fn short4(read: &[u8], at: usize, normalized: bool) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (component, slot) in out.iter_mut().enumerate() {
        let at = at + component * 2;
        let v = i16::from_le_bytes([read[at], read[at + 1]]) as f32;
        *slot = if normalized { (v / 32767.0).max(-1.0) } else { v };
    }
    out
}

#[inline]
fn ushort4(read: &[u8], at: usize, normalized: bool) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (component, slot) in out.iter_mut().enumerate() {
        let at = at + component * 2;
        let v = u16::from_le_bytes([read[at], read[at + 1]]) as f32;
        *slot = if normalized { v / 65535.0 } else { v };
    }
    out
}

#[inline]
fn byte4(read: &[u8], at: usize, normalized: bool) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (component, slot) in out.iter_mut().enumerate() {
        let v = read[at + component] as i8 as f32;
        *slot = if normalized { (v / 127.0).max(-1.0) } else { v };
    }
    out
}

#[inline]
fn ubyte4(read: &[u8], at: usize, normalized: bool) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (component, slot) in out.iter_mut().enumerate() {
        let v = read[at + component] as f32;
        *slot = if normalized { v / 255.0 } else { v };
    }
    out
}

#[inline]
fn write3(out: &mut [u8], at: usize, a: f32, b: f32, c: f32) {
    out[at..at + 4].copy_from_slice(&a.to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&b.to_le_bytes());
    out[at + 8..at + 12].copy_from_slice(&c.to_le_bytes());
}

#[inline]
fn write4(out: &mut [u8], at: usize, a: f32, b: f32, c: f32, d: f32) {
    out[at..at + 4].copy_from_slice(&a.to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&b.to_le_bytes());
    out[at + 8..at + 12].copy_from_slice(&c.to_le_bytes());
    out[at + 12..at + 16].copy_from_slice(&d.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_and_sizes() {
        assert_eq!(components_of("VEC3"), Some(3));
        assert_eq!(components_of("MAT4"), None);
        assert_eq!(component_size(COMP_BYTE), 1);
        assert_eq!(component_size(COMP_UNSIGNED_SHORT), 2);
        assert_eq!(component_size(COMP_FLOAT), 4);
    }

    /// The conversion table, pinned. These are the values that make a quantized
    /// mesh land where the node transform says it should.
    #[test]
    fn component_conversion_matches_the_spec() {
        let f = |bytes: &[u8], ty: u64, norm: bool| component_to_f32(bytes, ty, norm);
        assert_eq!(f(&[0x00], COMP_BYTE, false), 0.0);
        assert_eq!(f(&[0xff], COMP_BYTE, false), -1.0);
        assert_eq!(f(&[0x80], COMP_BYTE, true), -1.0, "-128 clamps to -1");
        assert_eq!(f(&[0x7f], COMP_BYTE, true), 1.0);
        assert_eq!(f(&[0xff], COMP_UNSIGNED_BYTE, true), 1.0);
        assert_eq!(
            f(&1000i16.to_le_bytes(), COMP_SHORT, false),
            1000.0
        );
        assert_eq!(f(&i16::MIN.to_le_bytes(), COMP_SHORT, true), -1.0);
        assert_eq!(f(&u16::MAX.to_le_bytes(), COMP_UNSIGNED_SHORT, true), 1.0);
        assert_eq!(f(&1.5f32.to_le_bytes(), COMP_FLOAT, false), 1.5);
    }

    /// A VEC3 accessor widened with an interleaved stride produces the same
    /// bytes as the component-by-component reference path.
    #[test]
    fn apply_matches_the_reference_path() {
        // Two elements of `{ pad: u32, position: i16x3 }` — stride 12, offset 4.
        let mut bin = vec![0u8; 12 * 2];
        for (element, values) in [[-100i16, 0, 100], [32767, -32768, 12]].iter().enumerate() {
            for (component, value) in values.iter().enumerate() {
                let at = element * 12 + 4 + component * 2;
                bin[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
        }
        let mut doc = serde_json::json!({
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": bin.len(), "byteStride": 12 }],
            "accessors": [{ "bufferView": 0, "byteOffset": 4, "componentType": COMP_SHORT, "type": "VEC3", "count": 2, "normalized": true }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
        });
        let plan = plan(&mut doc, bin.len()).expect("plans");
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.out_len, bin.len() + 2 * 12);

        let mut out = vec![0u8; plan.out_len - bin.len()];
        apply(&plan.jobs, &bin, &mut out, bin.len());
        for element in 0..2 {
            for component in 0..3 {
                let at = element * 12 + 4 + component * 2;
                let expected = component_to_f32(&bin[at..at + 2], COMP_SHORT, true);
                let got = f32::from_le_bytes(
                    out[element * 12 + component * 4..][..4].try_into().unwrap(),
                );
                assert_eq!(got, expected, "element {element} component {component}");
            }
        }
        assert_eq!(doc["accessors"][0]["componentType"], COMP_FLOAT);
        assert_eq!(doc["accessors"][0]["bufferView"], 1);
        assert!(doc["accessors"][0].get("normalized").is_none());
    }

    #[test]
    fn plan_skips_float_and_untouched_semantics() {
        let mut doc = serde_json::json!({
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 64 }],
            "accessors": [
                { "bufferView": 0, "componentType": COMP_FLOAT, "type": "VEC3", "count": 2 },
                { "bufferView": 0, "componentType": COMP_SHORT, "type": "VEC2", "count": 2 },
            ],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "TEXCOORD_0": 1 } }] }],
        });
        let plan = plan(&mut doc, 64).expect("plans");
        assert!(plan.is_empty());
        assert_eq!(plan.out_len, 64);
    }

    #[test]
    fn plan_rejects_reads_past_the_buffer() {
        let mut doc = serde_json::json!({
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 8 }],
            "accessors": [{ "bufferView": 0, "componentType": COMP_SHORT, "type": "VEC3", "count": 8 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
        });
        assert!(plan(&mut doc, 8).is_err());
    }

    /// A `byteStride: 0` view is invalid glTF; it must not let `count` run past
    /// the buffer.
    #[test]
    fn zero_stride_is_treated_as_packed() {
        let mut doc = serde_json::json!({
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 12, "byteStride": 0 }],
            "accessors": [{ "bufferView": 0, "componentType": COMP_SHORT, "type": "VEC3", "count": 2 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
        });
        let plan = plan(&mut doc, 12).expect("plans");
        assert_eq!(plan.jobs[0].stride, 6, "packed, not zero");
    }
}
