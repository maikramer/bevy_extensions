//! The pre-rewrite decoder, kept as a byte-for-byte oracle.
//!
//! The single-pass rewrite is only trustworthy if it produces *exactly* the
//! bytes the old implementation produced: the loader, the collider baker and
//! the LOD tiers all see that output, and a silent difference would be very
//! hard to attribute later. `tests/parity.rs` (and `examples/decode_bench`
//! with `--features legacy`) compare this module against the shipping decoder
//! on the fixtures and on a sample of the real pool.
//!
//! Nothing in the game links this: the feature is off by default.

#![allow(dead_code)]

use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::decode::{Filter, Mode};

pub const GLB_MAGIC: u32 = 0x4654_6C67;
pub const CHUNK_JSON: u32 = 0x4E4F_534A;
pub const CHUNK_BIN: u32 = 0x004E_4942;
const EXT: &str = "EXT_meshopt_compression";
const QUANTIZATION: &str = "KHR_mesh_quantization";
const BASISU: &str = "KHR_texture_basisu";

pub fn needs_rewrite(bytes: &[u8]) -> bool {
    uses_any(bytes, &[EXT, QUANTIZATION, BASISU])
}

fn uses_any(bytes: &[u8], names: &[&str]) -> bool {
    parse_glb(bytes)
        .ok()
        .and_then(|(json, _)| {
            serde_json::from_slice::<serde_json::Value>(json)
                .ok()
                .map(|doc| {
                    doc.get("extensionsUsed")
                        .and_then(|v| v.as_array())
                        .is_some_and(|used| {
                            used.iter()
                                .filter_map(serde_json::Value::as_str)
                                .any(|e| names.contains(&e))
                        })
                })
        })
        .unwrap_or(false)
}

fn parse_glb(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    if bytes.len() < 12 {
        bail!("not a GLB: {} bytes", bytes.len());
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into()?);
    if magic != GLB_MAGIC {
        bail!("not a GLB: bad magic {magic:#x}");
    }
    let mut json: Option<&[u8]> = None;
    let mut bin: &[u8] = &[];
    let mut cursor = 12;
    while cursor + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into()?) as usize;
        let kind = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into()?);
        let start = cursor + 8;
        let end = start
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .context("GLB chunk runs past the end of the file")?;
        match kind {
            CHUNK_JSON => json = Some(&bytes[start..end]),
            CHUNK_BIN => bin = &bytes[start..end],
            _ => {}
        }
        cursor = end;
    }
    Ok((json.context("GLB has no JSON chunk")?, bin))
}

pub fn decode_stream(
    encoded: &[u8],
    count: usize,
    stride: usize,
    mode: Mode,
    filter: Filter,
) -> Result<Vec<u8>> {
    if count == 0 || stride == 0 {
        return Ok(Vec::new());
    }
    let total = count
        .checked_mul(stride)
        .context("decoded stream size overflows (count × stride)")?;
    let mut out = vec![0u8; total];

    let code = match mode {
        Mode::Attributes => unsafe {
            meshopt::ffi::meshopt_decodeVertexBuffer(
                out.as_mut_ptr().cast(),
                count,
                stride,
                encoded.as_ptr(),
                encoded.len(),
            )
        },
        Mode::Triangles => unsafe {
            meshopt::ffi::meshopt_decodeIndexBuffer(
                out.as_mut_ptr().cast(),
                count,
                stride,
                encoded.as_ptr(),
                encoded.len(),
            )
        },
        Mode::Indices => unsafe {
            meshopt::ffi::meshopt_decodeIndexSequence(
                out.as_mut_ptr().cast(),
                count,
                stride,
                encoded.as_ptr(),
                encoded.len(),
            )
        },
    };
    if code != 0 {
        bail!("meshopt decode failed with code {code} (mode {mode:?}, {count}x{stride})");
    }

    match filter {
        Filter::None => {}
        Filter::Octahedral => unsafe {
            meshopt::ffi::meshopt_decodeFilterOct(out.as_mut_ptr().cast(), count, stride);
        },
        Filter::Quaternion => unsafe {
            meshopt::ffi::meshopt_decodeFilterQuat(out.as_mut_ptr().cast(), count, stride);
        },
        Filter::Exponential => unsafe {
            meshopt::ffi::meshopt_decodeFilterExp(out.as_mut_ptr().cast(), count, stride);
        },
    }
    Ok(out)
}

pub fn decode_glb(bytes: &[u8]) -> Result<Vec<u8>> {
    if !needs_rewrite(bytes) {
        return Ok(bytes.to_vec());
    }
    let (json_chunk, bin) = parse_glb(bytes)?;
    let mut doc: serde_json::Value =
        serde_json::from_slice(json_chunk).context("parsing the GLB JSON chunk")?;

    let buffers = doc
        .get("buffers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for (i, buffer) in buffers.iter().enumerate() {
        if buffer.get("uri").is_some() {
            bail!("buffer {i} is external (`uri`); only self-contained GLBs are decoded");
        }
    }

    let views = doc
        .get("bufferViews")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out_bin: Vec<u8> = Vec::with_capacity(bin.len() * 3);
    let mut relocated: HashMap<usize, (usize, usize)> = HashMap::new();

    for (index, view) in views.iter().enumerate() {
        let Some(ext) = view.get("extensions").and_then(|e| e.get(EXT)) else {
            continue;
        };
        let get_usize = |key: &str| -> Result<usize> {
            ext.get(key)
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as usize)
                .with_context(|| format!("bufferView {index}: `{key}` missing from {EXT}"))
        };
        let byte_offset = ext
            .get("byteOffset")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let byte_length = get_usize("byteLength")?;
        let count = get_usize("count")?;
        let stride = get_usize("byteStride")?;
        let mode = Mode::parse(
            ext.get("mode")
                .and_then(serde_json::Value::as_str)
                .context("EXT_meshopt_compression: `mode` missing")?,
        )?;
        let filter = match ext.get("filter").and_then(serde_json::Value::as_str) {
            Some(name) => Filter::parse(name)?,
            None => Filter::None,
        };

        let end = byte_offset
            .checked_add(byte_length)
            .filter(|end| *end <= bin.len())
            .with_context(|| {
                format!("bufferView {index}: compressed range outside the BIN chunk")
            })?;
        let decoded = decode_stream(&bin[byte_offset..end], count, stride, mode, filter)
            .with_context(|| format!("decoding bufferView {index}"))?;

        while !out_bin.len().is_multiple_of(4) {
            out_bin.push(0);
        }
        relocated.insert(index, (out_bin.len(), decoded.len()));
        out_bin.extend_from_slice(&decoded);
    }

    if relocated.is_empty() {
        let mut doc = doc;
        let mut out_bin = bin.to_vec();
        lift_basisu_sources(&mut doc);
        if dequantize_vertex_attributes(&mut doc, &mut out_bin)? {
            strip_extension(&mut doc, "extensionsUsed", QUANTIZATION);
        }
        strip_extension(&mut doc, "extensionsRequired", QUANTIZATION);
        doc["buffers"] = serde_json::json!([{ "byteLength": out_bin.len() }]);
        widen_uastc_channel_layout(&doc, &mut out_bin);
        return write_glb(&doc, &out_bin);
    }

    let mut plain: HashMap<usize, (usize, usize)> = HashMap::new();
    for (index, view) in views.iter().enumerate() {
        if relocated.contains_key(&index) {
            continue;
        }
        let offset = view
            .get("byteOffset")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let length = view
            .get("byteLength")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= bin.len())
            .with_context(|| format!("bufferView {index}: range outside the BIN chunk"))?;
        while !out_bin.len().is_multiple_of(4) {
            out_bin.push(0);
        }
        plain.insert(index, (out_bin.len(), length));
        out_bin.extend_from_slice(&bin[offset..end]);
    }

    if let Some(array) = doc.get_mut("bufferViews").and_then(|v| v.as_array_mut()) {
        for (index, view) in array.iter_mut().enumerate() {
            let (offset, length) = relocated
                .get(&index)
                .or_else(|| plain.get(&index))
                .copied()
                .context("every buffer view is accounted for")?;
            let object = view
                .as_object_mut()
                .context("bufferView is not a JSON object")?;
            object.insert("buffer".into(), serde_json::json!(0));
            object.insert("byteOffset".into(), serde_json::json!(offset));
            object.insert("byteLength".into(), serde_json::json!(length));
            if let Some(extensions) = object.get_mut("extensions").and_then(|e| e.as_object_mut()) {
                extensions.remove(EXT);
                if extensions.is_empty() {
                    object.remove("extensions");
                }
            }
        }
    }

    doc["buffers"] = serde_json::json!([{ "byteLength": out_bin.len() }]);
    strip_extension(&mut doc, "extensionsUsed", EXT);
    strip_extension(&mut doc, "extensionsRequired", EXT);
    lift_basisu_sources(&mut doc);
    if dequantize_vertex_attributes(&mut doc, &mut out_bin)? {
        strip_extension(&mut doc, "extensionsUsed", QUANTIZATION);
    }
    strip_extension(&mut doc, "extensionsRequired", QUANTIZATION);
    doc["buffers"] = serde_json::json!([{ "byteLength": out_bin.len() }]);
    widen_uastc_channel_layout(&doc, &mut out_bin);

    write_glb(&doc, &out_bin)
}

fn lift_basisu_sources(doc: &mut serde_json::Value) {
    let mut lifted = false;
    if let Some(textures) = doc.get_mut("textures").and_then(|v| v.as_array_mut()) {
        for texture in textures.iter_mut() {
            let Some(object) = texture.as_object_mut() else {
                continue;
            };
            if object.contains_key("source") {
                continue;
            }
            let source = object
                .get("extensions")
                .and_then(|e| e.get(BASISU))
                .and_then(|b| b.get("source"))
                .and_then(serde_json::Value::as_u64);
            let Some(source) = source else { continue };
            object.insert("source".into(), serde_json::json!(source));
            if let Some(extensions) = object.get_mut("extensions").and_then(|e| e.as_object_mut()) {
                extensions.remove(BASISU);
                if extensions.is_empty() {
                    object.remove("extensions");
                }
            }
            lifted = true;
        }
    }
    if lifted {
        strip_extension(doc, "extensionsUsed", BASISU);
        strip_extension(doc, "extensionsRequired", BASISU);
    }
}

const KTX2_IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
const KTX2_DFD_OFFSET_FIELD: usize = 48;
const KTX2_DFD_FIRST_SAMPLE: usize = 28;
const KTX2_DFD_SAMPLE_CHANNEL_TYPE: usize = 3;
const UASTC_CHANNEL_RRR: u8 = 4;
const UASTC_CHANNEL_RGB: u8 = 0;

fn widen_uastc_channel_layout(doc: &serde_json::Value, bin: &mut [u8]) -> usize {
    let Some(views) = doc.get("bufferViews").and_then(|v| v.as_array()) else {
        return 0;
    };
    let Some(images) = doc.get("images").and_then(|v| v.as_array()) else {
        return 0;
    };
    let mut widened = 0;
    for image in images {
        if image.get("uri").is_some() {
            continue;
        }
        let Some(view) = image
            .get("bufferView")
            .and_then(serde_json::Value::as_u64)
            .and_then(|index| views.get(index as usize))
        else {
            continue;
        };
        let start = view
            .get("byteOffset")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let length = view
            .get("byteLength")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let Some(end) = start.checked_add(length).filter(|end| *end <= bin.len()) else {
            continue;
        };
        let data = &mut bin[start..end];
        if data.len() < KTX2_DFD_OFFSET_FIELD + 4 || data[..12] != KTX2_IDENTIFIER {
            continue;
        }
        let dfd = u32::from_le_bytes([
            data[KTX2_DFD_OFFSET_FIELD],
            data[KTX2_DFD_OFFSET_FIELD + 1],
            data[KTX2_DFD_OFFSET_FIELD + 2],
            data[KTX2_DFD_OFFSET_FIELD + 3],
        ]) as usize;
        let channel_type = dfd + KTX2_DFD_FIRST_SAMPLE + KTX2_DFD_SAMPLE_CHANNEL_TYPE;
        if channel_type < data.len() && data[channel_type] == UASTC_CHANNEL_RRR {
            data[channel_type] = UASTC_CHANNEL_RGB;
            widened += 1;
        }
    }
    widened
}

fn strip_extension(doc: &mut serde_json::Value, key: &str, name: &str) {
    let Some(array) = doc.get_mut(key).and_then(|v| v.as_array_mut()) else {
        return;
    };
    array.retain(|e| e.as_str() != Some(name));
    if array.is_empty() {
        doc.as_object_mut().map(|o| o.remove(key));
    }
}

fn write_glb(doc: &serde_json::Value, bin: &[u8]) -> Result<Vec<u8>> {
    let mut json = serde_json::to_vec(doc)?;
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(&bin);
    }
    Ok(out)
}

const COMP_BYTE: u64 = 5120;
const COMP_UNSIGNED_BYTE: u64 = 5121;
const COMP_SHORT: u64 = 5122;
const COMP_UNSIGNED_SHORT: u64 = 5123;
const COMP_FLOAT: u64 = 5126;

const FLOAT_SEMANTICS: [&str; 3] = ["POSITION", "NORMAL", "TANGENT"];

fn components_of(kind: &str) -> Option<usize> {
    match kind {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" => Some(4),
        _ => None,
    }
}

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

fn component_size(component_type: u64) -> usize {
    match component_type {
        COMP_BYTE | COMP_UNSIGNED_BYTE => 1,
        COMP_SHORT | COMP_UNSIGNED_SHORT => 2,
        _ => 4,
    }
}

fn dequantize_vertex_attributes(doc: &mut serde_json::Value, bin: &mut Vec<u8>) -> Result<bool> {
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
    if wanted.is_empty() {
        return Ok(false);
    }

    let views = doc["bufferViews"].as_array().cloned().unwrap_or_default();
    let accessors = doc["accessors"].as_array().cloned().unwrap_or_default();
    let mut converted = false;

    for index in wanted {
        let Some(accessor) = accessors.get(index) else {
            continue;
        };
        let component_type = accessor["componentType"].as_u64().unwrap_or(COMP_FLOAT);
        if component_type == COMP_FLOAT {
            continue;
        }
        let Some(kind) = accessor["type"].as_str().and_then(components_of) else {
            continue;
        };
        let count = accessor["count"].as_u64().unwrap_or(0) as usize;
        let Some(view_index) = accessor["bufferView"].as_u64() else {
            continue;
        };
        let Some(view) = views.get(view_index as usize) else {
            continue;
        };
        let normalized = accessor["normalized"].as_bool().unwrap_or(false);
        let size = component_size(component_type);
        let view_offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let accessor_offset = accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = view["byteStride"]
            .as_u64()
            .map(|s| s as usize)
            .filter(|s| *s > 0)
            .unwrap_or(size * kind);
        let base = view_offset
            .checked_add(accessor_offset)
            .context("accessor offset overflows the buffer")?;

        let needed = stride
            .checked_mul(count.saturating_sub(1))
            .and_then(|tail| tail.checked_add(base))
            .and_then(|tail| tail.checked_add(size * kind))
            .context("accessor range overflows the buffer")?;
        if needed > bin.len() {
            bail!(
                "accessor {index}: reads past the buffer ({needed} > {})",
                bin.len()
            );
        }

        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let out_offset = bin.len();
        bin.reserve(count * kind * 4);
        for element in 0..count {
            let start = base + element * stride;
            for component in 0..kind {
                let at = start + component * size;
                let value = component_to_f32(&bin[at..at + size], component_type, normalized);
                bin.extend_from_slice(&value.to_le_bytes());
            }
        }
        let out_length = count * kind * 4;

        let new_view = serde_json::json!({
            "buffer": 0,
            "byteOffset": out_offset,
            "byteLength": out_length,
        });
        let new_index = {
            let array = doc["bufferViews"]
                .as_array_mut()
                .context("bufferViews is an array")?;
            array.push(new_view);
            array.len() - 1
        };
        let accessor = doc["accessors"][index]
            .as_object_mut()
            .context("accessor is an object")?;
        accessor.insert("componentType".into(), serde_json::json!(COMP_FLOAT));
        accessor.insert("bufferView".into(), serde_json::json!(new_index));
        accessor.insert("byteOffset".into(), serde_json::json!(0));
        accessor.remove("normalized");
        converted = true;
    }
    Ok(converted)
}

#[cfg(test)]
mod parity {
    //! Byte-for-byte parity between the shipping decoder and this oracle.
    //!
    //! Runs with `cargo test -p bevy_ext_gltf --features legacy`. The pool
    //! sample is every fifth GLB of the real pool (or the fixtures when
    //! `VAEL_ROOT` is unset), plus synthetic cases that pin the awkward
    //! layouts: an interleaved quantized attribute, a non-4-aligned output and
    //! a compressed view.

    use super::*;
    use crate::test_support;

    /// A GLB whose single buffer view is meshopt-compressed.
    fn compressed_glb() -> Vec<u8> {
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Vertex {
            position: [f32; 3],
            normal: [f32; 3],
        }
        let vertices: Vec<Vertex> = (0..256)
            .map(|i| Vertex {
                position: [i as f32, (i * 2) as f32, (i * 3) as f32],
                normal: [0.0, 1.0, 0.0],
            })
            .collect();
        let encoded = meshopt::encode_vertex_buffer(&vertices).expect("encodes");
        let doc = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": [EXT],
            "extensionsRequired": [EXT],
            "buffers": [{ "byteLength": encoded.len() }],
            "bufferViews": [{
                "buffer": 0,
                "byteOffset": 0,
                "byteLength": encoded.len(),
                "extensions": {
                    EXT: {
                        "buffer": 0,
                        "byteOffset": 0,
                        "byteLength": encoded.len(),
                        "byteStride": std::mem::size_of::<Vertex>(),
                        "count": vertices.len(),
                        "mode": "ATTRIBUTES",
                    }
                }
            }],
        });
        write_glb(&doc, &encoded).expect("writes")
    }

    /// A quantized, interleaved mesh whose widened output lands unaligned.
    fn quantized_glb() -> Vec<u8> {
        // { class: u8 padding, position: i16x3 } — stride 8, 3 elements.
        let mut bin = Vec::new();
        for element in 0..3i16 {
            bin.extend_from_slice(&[0, 0]);
            bin.extend_from_slice(&(element * 100).to_le_bytes());
            bin.extend_from_slice(&(-element * 100).to_le_bytes());
            bin.extend_from_slice(&(element * 7).to_le_bytes());
        }
        let doc = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": [QUANTIZATION],
            "extensionsRequired": [QUANTIZATION],
            "buffers": [{ "byteLength": bin.len() }],
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": bin.len(), "byteStride": 8 }],
            "accessors": [{
                "bufferView": 0,
                "byteOffset": 2,
                "componentType": COMP_SHORT,
                "type": "VEC3",
                "count": 3,
                "normalized": false,
            }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
        });
        write_glb(&doc, &bin).expect("writes")
    }

    fn assert_parity(name: &str, bytes: &[u8]) {
        if !needs_rewrite(bytes) {
            return;
        }
        let old = decode_glb(bytes).unwrap_or_else(|e| panic!("legacy {name}: {e:#}"));
        let new = crate::decode_glb(bytes).unwrap_or_else(|e| panic!("current {name}: {e:#}"));
        assert_eq!(
            new.as_ref(),
            old.as_slice(),
            "{name}: the rewrite is not byte-identical"
        );
    }

    #[test]
    fn synthetic_layouts_are_identical() {
        assert_parity("meshopt", &compressed_glb());
        assert_parity("quantized", &quantized_glb());
    }

    #[test]
    fn fixtures_are_identical() {
        let Some(mesh_dir) = test_support::mesh_dir() else {
            eprintln!("pool ausente — a saltar");
            return;
        };
        let mut paths = Vec::new();
        test_support::walk_glb(&mesh_dir, &mut paths);
        paths.sort();
        let sample: Vec<_> = paths.iter().step_by(5).collect();
        assert!(!sample.is_empty(), "the sample is not empty");
        for path in sample {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            assert_parity(&path.display().to_string(), &bytes);
        }
    }
}
