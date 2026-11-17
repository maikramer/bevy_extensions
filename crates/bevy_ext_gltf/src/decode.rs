//! The rewrite: expand meshopt views, widen quantized attributes, lift KTX2
//! texture sources — laying the whole binary chunk out before writing a byte.
//!
//! # Why one pass matters
//!
//! Every size this module needs is known from the JSON alone (`count × stride`
//! for a compressed view, `count × components × 4` for a widened accessor,
//! `byteLength` for a plain one), so the final offsets can be assigned *before*
//! any data moves. That lets the decoder allocate the output GLB once and
//! decode straight into it, instead of growing an intermediate buffer, cloning
//! it, and copying it again on the way out.
//!
//! # What is a `Cow` for
//!
//! Most of the pool is plain GLB. Returning `Cow::Borrowed` there means the
//! reader hands its own buffer to the loader instead of duplicating every file
//! it does not need to touch.

use std::borrow::Cow;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::settings::settings;
use crate::{dequant, extensions, glb, stats};

/// KTX2 file identifier (spec §3.1).
const KTX2_IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
/// Byte offset of `dfdByteOffset` in a KTX2 header: 12 (identifier) + 36 (nine
/// `u32` header fields) (spec §3.1).
const KTX2_DFD_OFFSET_FIELD: usize = 48;
/// Byte offset of `supercompressionScheme` in a KTX2 header (the ninth field).
const KTX2_SUPERCOMPRESSION_FIELD: usize = 44;
/// `BasisLZ`, the scheme an ETC1S stream ships with.
const KTX2_SUPERCOMPRESSION_BASISLZ: u32 = 1;
/// First byte of the first sample descriptor in a KTX2 data-format descriptor:
/// 4 (`dfdTotalSize`) + 4 (`vendorIdAndVersion`) + 4 (`descriptorBlockSize`) +
/// 4 (color model/primaries/transfer/flags) + 4 (`texelBlockDimension`) + 8
/// (`bytesPlane`) (spec §3.10).
const KTX2_DFD_FIRST_SAMPLE: usize = 28;
/// `channelType` of the first sample within its descriptor: `bitOffset` (2) +
/// `bitLength` (1) (spec §3.10).
const KTX2_DFD_SAMPLE_CHANNEL_TYPE: usize = 3;
/// `colorModel` byte of the descriptor block (spec §3.10).
const KTX2_DFD_COLOR_MODEL: usize = 8;
/// `KHR_DF_MODEL_ETC1S` — the model Bevy cannot transcode.
const KTX2_MODEL_ETC1S: u8 = 163;
/// `KHR_DF_CHANNEL_UASTC_RRR` — the single-channel UASTC layout.
const UASTC_CHANNEL_RRR: u8 = 4;
/// `KHR_DF_CHANNEL_UASTC_RGB` — the three-channel UASTC layout.
const UASTC_CHANNEL_RGB: u8 = 0;

/// What a compressed buffer view expands to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Interleaved vertex attributes.
    Attributes,
    /// Triangle index buffer.
    Triangles,
    /// Index sequence (points/lines, or non-triangle topologies).
    Indices,
}

impl Mode {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "ATTRIBUTES" => Ok(Mode::Attributes),
            "TRIANGLES" => Ok(Mode::Triangles),
            "INDICES" => Ok(Mode::Indices),
            other => bail!("unknown EXT_meshopt_compression mode `{other}`"),
        }
    }
}

/// Post-decode transform to undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    None,
    Octahedral,
    Quaternion,
    Exponential,
}

impl Filter {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "NONE" => Ok(Filter::None),
            "OCTAHEDRAL" => Ok(Filter::Octahedral),
            "QUATERNION" => Ok(Filter::Quaternion),
            "EXPONENTIAL" => Ok(Filter::Exponential),
            other => bail!("unknown EXT_meshopt_compression filter `{other}`"),
        }
    }
}

/// `true` when `bytes` is a GLB that needs rewriting.
pub fn needs_rewrite(bytes: &[u8]) -> bool {
    extensions::probe_glb(bytes).any()
}

/// `true` when `bytes` carries meshopt-compressed buffer views.
pub fn needs_decode(bytes: &[u8]) -> bool {
    extensions::probe_glb(bytes).meshopt
}

/// Decodes every compressed buffer view in a GLB, returning a plain GLB.
///
/// Returns the input borrowed when nothing needs rewriting, so this is safe to
/// call on any asset without paying a copy.
pub fn decode_glb(bytes: &[u8]) -> Result<Cow<'_, [u8]>> {
    decode_glb_named(bytes, None)
}

/// [`decode_glb`] with a name for the diagnostics.
pub fn decode_glb_named<'a>(bytes: &'a [u8], name: Option<&str>) -> Result<Cow<'a, [u8]>> {
    let started = Instant::now();
    let Ok(chunks) = glb::parse(bytes) else {
        stats::record_file(
            bytes.len(),
            bytes.len(),
            false,
            started.elapsed().as_nanos() as u64,
        );
        return Ok(Cow::Borrowed(bytes));
    };
    if !extensions::has_rewritten_extension(chunks.json) {
        stats::record_file(
            bytes.len(),
            bytes.len(),
            false,
            started.elapsed().as_nanos() as u64,
        );
        return Ok(Cow::Borrowed(bytes));
    }
    let Ok(doc) = glb::parse_json(chunks.json) else {
        stats::record_file(
            bytes.len(),
            bytes.len(),
            false,
            started.elapsed().as_nanos() as u64,
        );
        return Ok(Cow::Borrowed(bytes));
    };
    if !extensions::uses(&doc).any() {
        stats::record_file(
            bytes.len(),
            bytes.len(),
            false,
            started.elapsed().as_nanos() as u64,
        );
        return Ok(Cow::Borrowed(bytes));
    }
    let out = rewrite_parsed(doc, chunks.bin, name)?;
    stats::record_file(bytes.len(), out.len(), true, started.elapsed().as_nanos() as u64);
    Ok(Cow::Owned(out))
}

/// One compressed buffer view, resolved against the source BIN chunk.
#[derive(Debug, Clone, Copy)]
struct Compressed {
    /// Range of the compressed bytes in the source BIN.
    offset: usize,
    len: usize,
    count: usize,
    stride: usize,
    mode: Mode,
    filter: Filter,
}

impl Compressed {
    fn parse(index: usize, ext: &Value, bin_len: usize) -> Result<Self> {
        let get_usize = |key: &str| -> Result<usize> {
            ext.get(key)
                .and_then(Value::as_u64)
                .map(|v| v as usize)
                .with_context(|| format!("bufferView {index}: `{key}` missing from {}", extensions::MESHOPT))
        };
        let offset = ext.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let len = get_usize("byteLength")?;
        let count = get_usize("count")?;
        let stride = get_usize("byteStride")?;
        // Malformed claims (count/stride come from the JSON) must not overflow
        // the output size: in release the wrap used to hand the C decoder a
        // buffer smaller than it wrote.
        count
            .checked_mul(stride)
            .context("decoded stream size overflows (count × stride)")?;
        let mode = Mode::parse(
            ext.get("mode")
                .and_then(Value::as_str)
                .context("EXT_meshopt_compression: `mode` missing")?,
        )?;
        let filter = match ext.get("filter").and_then(Value::as_str) {
            Some(name) => Filter::parse(name)?,
            None => Filter::None,
        };
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= bin_len)
            .with_context(|| {
                format!("bufferView {index}: compressed range outside the BIN chunk")
            })?;
        Ok(Self {
            offset,
            len: end - offset,
            count,
            stride,
            mode,
            filter,
        })
    }

    /// Bytes this view expands to (an empty stream decodes to nothing).
    fn decoded_len(&self) -> usize {
        if self.count == 0 || self.stride == 0 {
            0
        } else {
            self.count * self.stride
        }
    }
}

/// Expands one compressed stream into raw bytes.
///
/// `count` elements of `stride` bytes each; `encoded` is the compressed slice
/// taken from the buffer the extension names.
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
    decode_into(&mut out, encoded, count, stride, mode, filter)?;
    Ok(out)
}

/// [`decode_stream`] into a caller-provided buffer of exactly `count × stride`
/// bytes — the decoder's output lands in the final GLB without an intermediate.
pub fn decode_stream_into(
    out: &mut [u8],
    encoded: &[u8],
    count: usize,
    stride: usize,
    mode: Mode,
    filter: Filter,
) -> Result<()> {
    if count == 0 || stride == 0 {
        return Ok(());
    }
    let total = count
        .checked_mul(stride)
        .context("decoded stream size overflows (count × stride)")?;
    if out.len() != total {
        bail!(
            "decode target is {} bytes, expected {total} ({count}×{stride})",
            out.len()
        );
    }
    decode_into(out, encoded, count, stride, mode, filter)
}

fn decode_into(
    out: &mut [u8],
    encoded: &[u8],
    count: usize,
    stride: usize,
    mode: Mode,
    filter: Filter,
) -> Result<()> {
    // The safe wrappers in `meshopt` are generic over a compile-time sized
    // element type; the extension's stride is only known at runtime, so the
    // C entry points are called directly.
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

    // Filters are applied in place over the decoded bytes.
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
    Ok(())
}

/// The whole rewrite: plan the layout, rewrite the JSON, write the container.
fn rewrite_parsed(mut doc: Value, bin: &[u8], name: Option<&str>) -> Result<Vec<u8>> {
    if !doc.is_object() {
        bail!("the glTF JSON chunk is not an object");
    }
    diagnose(&doc, bin, name);

    // Buffers other than the embedded BIN chunk are not supported: the
    // pipeline emits self-contained GLBs, and a URI buffer would need the
    // asset reader to resolve a sibling file.
    if let Some(buffers) = doc.get("buffers").and_then(Value::as_array) {
        for (i, buffer) in buffers.iter().enumerate() {
            if buffer.get("uri").is_some() {
                bail!("buffer {i} is external (`uri`); only self-contained GLBs are decoded");
            }
        }
    }

    // ---- plan: where every view lands in the new BIN chunk ----
    let view_count = doc
        .get("bufferViews")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut targets: Vec<Option<(usize, usize)>> = vec![None; view_count];
    let mut compressed: Vec<(usize, Compressed)> = Vec::new();
    let mut copies: Vec<(usize, usize, usize)> = Vec::new();
    let mut at = 0usize;
    for (index, view) in doc["bufferViews"].as_array().into_iter().flatten().enumerate() {
        let Some(ext) = view
            .get("extensions")
            .and_then(|e| e.get(extensions::MESHOPT))
        else {
            continue;
        };
        let spec = Compressed::parse(index, ext, bin.len())?;
        at = glb::align4(at);
        targets[index] = Some((at, spec.decoded_len()));
        at += spec.decoded_len();
        compressed.push((index, spec));
    }
    let pre_dequant = if compressed.is_empty() {
        // Nothing to expand: the binary chunk survives as-is and only the JSON
        // changes (a KTX2 source lift, a quantization flag).
        bin.len()
    } else {
        for (index, view) in doc["bufferViews"].as_array().into_iter().flatten().enumerate() {
            if targets[index].is_some() {
                continue;
            }
            let offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let end = offset
                .checked_add(view.get("byteLength").and_then(Value::as_u64).unwrap_or(0) as usize)
                .filter(|end| *end <= bin.len())
                .with_context(|| format!("bufferView {index}: range outside the BIN chunk"))?;
            let length = end - offset;
            at = glb::align4(at);
            targets[index] = Some((at, length));
            copies.push((at, offset, length));
            at += length;
        }
        at
    };

    // ---- rewrite the JSON: every view points into the single new buffer ----
    if !compressed.is_empty() {
        if let Some(array) = doc.get_mut("bufferViews").and_then(Value::as_array_mut) {
            for (index, view) in array.iter_mut().enumerate() {
                let (offset, length) = targets[index]
                    .with_context(|| format!("bufferView {index} was not planned"))?;
                let object = view
                    .as_object_mut()
                    .context("bufferView is not a JSON object")?;
                object.insert("buffer".into(), json!(0));
                object.insert("byteOffset".into(), json!(offset));
                object.insert("byteLength".into(), json!(length));
                if let Some(ext) = object.get_mut("extensions").and_then(|e| e.as_object_mut()) {
                    ext.remove(extensions::MESHOPT);
                    if ext.is_empty() {
                        object.remove("extensions");
                    }
                }
            }
        }
        doc["buffers"] = json!([{ "byteLength": pre_dequant }]);
        extensions::strip(&mut doc, "extensionsUsed", extensions::MESHOPT);
        extensions::strip(&mut doc, "extensionsRequired", extensions::MESHOPT);
    }

    lift_basisu_sources(&mut doc);
    // After the buffer views are rewritten: the widener reads decoded data and
    // appends its packed output to the same binary chunk.
    let settings = settings();
    let plan = if settings.dequantize {
        let dequant_started = Instant::now();
        let plan = dequant::plan(&mut doc, pre_dequant)?;
        stats::record_dequant(dequant_started.elapsed().as_nanos() as u64);
        plan
    } else {
        dequant::Plan {
            jobs: Vec::new(),
            out_len: pre_dequant,
        }
    };
    if !plan.is_empty() {
        extensions::strip(&mut doc, "extensionsUsed", extensions::QUANTIZATION);
    }
    // `KHR_mesh_quantization` only tells a reader to expect normalized integer
    // attributes, which core glTF accessors already describe — the flag gates
    // readers rather than changing the data. Bevy refuses any file that
    // *requires* an extension it does not name, so the requirement is dropped
    // while `extensionsUsed` keeps the honest record of how the file was built.
    extensions::strip(&mut doc, "extensionsRequired", extensions::QUANTIZATION);
    doc["buffers"] = json!([{ "byteLength": plan.out_len }]);

    // ---- write the container ----
    let mut json_bytes = serde_json::to_vec(&doc)?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let layout = glb::Layout::new(json_bytes.len(), plan.out_len)?;
    let mut out = vec![0u8; layout.total];
    glb::write_header(&mut out, &layout)?;
    out[20..20 + json_bytes.len()].copy_from_slice(&json_bytes);
    let bin_start = layout.bin_start;
    if layout.bin_len > 0 {
        if compressed.is_empty() {
            out[bin_start..bin_start + bin.len()].copy_from_slice(bin);
        } else {
            for (index, spec) in &compressed {
                let (offset, len) = targets[*index].expect("compressed views are planned");
                decode_stream_into(
                    &mut out[bin_start + offset..bin_start + offset + len],
                    &bin[spec.offset..spec.offset + spec.len],
                    spec.count,
                    spec.stride,
                    spec.mode,
                    spec.filter,
                )
                .with_context(|| format!("decoding bufferView {index}"))?;
            }
            for (dest, source, length) in &copies {
                out[bin_start + dest..bin_start + dest + length]
                    .copy_from_slice(&bin[*source..*source + *length]);
            }
        }
    }

    // Widened attributes read the views just decoded and write their own block
    // after them; the two ranges never overlap, so one chunk serves both.
    if !plan.is_empty() {
        let dequant_start = plan.jobs[0].out_offset;
        let started = Instant::now();
        let (head, tail) = out.split_at_mut(bin_start + dequant_start);
        dequant::apply(&plan.jobs, &head[bin_start..], tail, dequant_start);
        stats::record_dequant(started.elapsed().as_nanos() as u64);
    }

    // Last, on the FINAL buffer: the layout lives inside the KTX2 payload, so
    // it has to be patched where the image bytes actually ended up.
    let widened = if settings.uastc_relabel {
        widen_uastc_channel_layout(&doc, &mut out[bin_start..bin_start + layout.bin_len])
    } else {
        0
    };
    stats::record_uastc_relabels(widened);
    Ok(out)
}

/// Warnings that turn a silent failure into an actionable one.
fn diagnose(doc: &Value, bin: &[u8], name: Option<&str>) {
    let required = extensions::unsupported_required(doc);
    if !required.is_empty() {
        stats::record_unsupported();
        for extension in required {
            stats::warn_once(&format!("required:{extension}"), || {
                format!(
                    "{}: exige `{extension}`, que o descodificador não conhece — o loader vai recusar o ficheiro",
                    name.unwrap_or("<glb>")
                )
            });
        }
    }
    if extensions::has_draco(doc) {
        stats::record_unsupported();
        stats::warn_once("draco", || {
            format!(
                "{}: geometria comprimida com Draco; nem este crate nem o Bevy a descodificam — reexporta sem Draco",
                name.unwrap_or("<glb>")
            )
        });
    }
    if extensions::has_sparse_accessors(doc) {
        stats::warn_once("sparse", || {
            format!(
                "{}: accessors esparsos; o rewrite não os toca e o loader pode recusar",
                name.unwrap_or("<glb>")
            )
        });
    }
    if let Some(image) = first_etc1s_image(doc, bin) {
        stats::record_unsupported();
        stats::warn_once("etc1s", || {
            format!(
                "{image}: textura KTX2 ETC1S/BasisLZ; o Bevy só transcodifica UASTC — gera com `text3d finish`"
            )
        });
    }
}

/// The first embedded KTX2 image that ships an ETC1S/BasisLZ stream.
fn first_etc1s_image(doc: &Value, bin: &[u8]) -> Option<String> {
    let views = doc.get("bufferViews").and_then(Value::as_array)?;
    for (index, image) in doc.get("images").and_then(Value::as_array)?.iter().enumerate() {
        if image.get("uri").is_some() {
            continue;
        }
        let Some(view) = image
            .get("bufferView")
            .and_then(Value::as_u64)
            .and_then(|index| views.get(index as usize))
        else {
            continue;
        };
        let start = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let length = view.get("byteLength").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(end) = start.checked_add(length).filter(|end| *end <= bin.len()) else {
            continue;
        };
        let data = &bin[start..end];
        if data.len() < KTX2_DFD_OFFSET_FIELD + 4 || data[..12] != KTX2_IDENTIFIER {
            continue;
        }
        let scheme = u32::from_le_bytes([
            data[KTX2_SUPERCOMPRESSION_FIELD],
            data[KTX2_SUPERCOMPRESSION_FIELD + 1],
            data[KTX2_SUPERCOMPRESSION_FIELD + 2],
            data[KTX2_SUPERCOMPRESSION_FIELD + 3],
        ]);
        let dfd = u32::from_le_bytes([
            data[KTX2_DFD_OFFSET_FIELD],
            data[KTX2_DFD_OFFSET_FIELD + 1],
            data[KTX2_DFD_OFFSET_FIELD + 2],
            data[KTX2_DFD_OFFSET_FIELD + 3],
        ]) as usize;
        let model = dfd + KTX2_DFD_COLOR_MODEL;
        if scheme == KTX2_SUPERCOMPRESSION_BASISLZ
            || (model < data.len() && data[model] == KTX2_MODEL_ETC1S)
        {
            return Some(format!("image {index}"));
        }
    }
    None
}

/// Moves `KHR_texture_basisu`'s image index into the texture's core `source`.
///
/// A KTX2 texture puts its image index inside the extension block and leaves
/// the core `source` unset. `gltf::Texture::source()` unwraps that field, so
/// the loader *panics* on such a file rather than reporting anything useful.
///
/// The image itself needs no conversion — it is an ordinary glTF image whose
/// `mimeType` is `image/ktx2`, which Bevy decodes with the `basis-universal`
/// and `ktx2` features. Only the index is in the wrong place, so it is lifted
/// and the requirement dropped.
fn lift_basisu_sources(doc: &mut Value) {
    let mut lifted = false;
    if let Some(textures) = doc.get_mut("textures").and_then(Value::as_array_mut) {
        for texture in textures.iter_mut() {
            let Some(object) = texture.as_object_mut() else {
                continue;
            };
            if object.contains_key("source") {
                continue;
            }
            let source = object
                .get("extensions")
                .and_then(|e| e.get(extensions::BASISU))
                .and_then(|b| b.get("source"))
                .and_then(Value::as_u64);
            let Some(source) = source else { continue };
            object.insert("source".into(), json!(source));
            if let Some(ext) = object.get_mut("extensions").and_then(|e| e.as_object_mut()) {
                ext.remove(extensions::BASISU);
                if ext.is_empty() {
                    object.remove("extensions");
                }
            }
            lifted = true;
        }
    }
    if lifted {
        extensions::strip(doc, "extensionsUsed", extensions::BASISU);
        extensions::strip(doc, "extensionsRequired", extensions::BASISU);
    }
}

/// Rewrites single-channel UASTC textures to the three-channel layout.
///
/// Workaround for a Bevy bug that makes an entire GLB unloadable. The pool's
/// KTX2 textures carry a *channel layout* in their data-format descriptor, and
/// `bevy_image::ktx2::get_transcoded_formats` maps `UASTC_RRR` to **BC4**. It
/// then sizes the slice handed to the UASTC transcoder with the *target*
/// format's block size —
///
/// ```text
/// let block_bytes = texture_format_info.block_copy_size(None).unwrap();
/// let level_bytes = (num_blocks_x * num_blocks_y * block_bytes) as usize;
/// transcoder.transcode_slice(&level_data[offset..(offset + level_bytes)], …)
/// ```
///
/// — but a UASTC block is **always 16 bytes**, while a BC4 block is 8. Half the
/// level is passed to a transcoder that needs all of it, and the load dies with
/// `Failed to transcode mip level 0 from UASTC to BC4: TranscodeFailed`
/// (BC5/BC7 paths are 16 bytes and unaffected, which is why only `RRR` — an
/// ambient-occlusion or roughness map exported as one channel — breaks).
/// Still present in Bevy 0.20.1 (`bevy_image-0.20.1/src/ktx2.rs:181-193`).
///
/// Declaring the texture `RGB` sends it down the BC7 path, where the block
/// sizes agree. Nothing is lost: UASTC codes four channels in every block
/// whatever the descriptor says, and the material samples the red channel, so
/// the widen is a re-labelling, not a conversion. The cost is BC7's 16 bytes
/// per block instead of BC4's 8 — twice the VRAM for those maps, and the
/// difference between "an invisible cactus" and "a cactus".
///
/// [`stats::DecodeStats::uastc_relabels`] counts them, so the cost is visible.
///
/// Returns how many textures were rewritten.
fn widen_uastc_channel_layout(doc: &Value, bin: &mut [u8]) -> usize {
    let Some(views) = doc.get("bufferViews").and_then(Value::as_array) else {
        return 0;
    };
    let Some(images) = doc.get("images").and_then(Value::as_array) else {
        return 0;
    };
    let mut widened = 0;
    for image in images {
        // An image with a `uri` lives in a sibling file we do not control.
        if image.get("uri").is_some() {
            continue;
        }
        let Some(view) = image
            .get("bufferView")
            .and_then(Value::as_u64)
            .and_then(|index| views.get(index as usize))
        else {
            continue;
        };
        let start = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let length = view.get("byteLength").and_then(Value::as_u64).unwrap_or(0) as usize;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_and_filter_parsing() {
        assert_eq!(Mode::parse("ATTRIBUTES").unwrap(), Mode::Attributes);
        assert_eq!(Mode::parse("TRIANGLES").unwrap(), Mode::Triangles);
        assert_eq!(Mode::parse("INDICES").unwrap(), Mode::Indices);
        assert!(Mode::parse("SOMETHING").is_err());

        assert_eq!(Filter::parse("NONE").unwrap(), Filter::None);
        assert_eq!(Filter::parse("OCTAHEDRAL").unwrap(), Filter::Octahedral);
        assert_eq!(Filter::parse("QUATERNION").unwrap(), Filter::Quaternion);
        assert_eq!(Filter::parse("EXPONENTIAL").unwrap(), Filter::Exponential);
        assert!(Filter::parse("BILINEAR").is_err());
    }

    #[test]
    fn decode_stream_is_empty_for_empty_input() {
        let out = decode_stream(&[], 0, 12, Mode::Attributes, Filter::None).unwrap();
        assert!(out.is_empty());
        let out = decode_stream(&[1, 2, 3], 4, 0, Mode::Attributes, Filter::None).unwrap();
        assert!(out.is_empty(), "a zero stride decodes to nothing");
    }

    /// Round-trips real vertex data through the encoder the pipeline uses.
    #[test]
    fn decode_stream_round_trips_encoded_vertices() {
        #[repr(C)]
        #[derive(Clone, Copy, Default, PartialEq, Debug)]
        struct Vertex {
            position: [f32; 3],
            uv: [f32; 2],
        }
        let vertices: Vec<Vertex> = (0..64)
            .map(|i| Vertex {
                position: [i as f32, (i * 2) as f32, (i * 3) as f32],
                uv: [i as f32 * 0.5, i as f32 * 0.25],
            })
            .collect();
        let encoded = meshopt::encode_vertex_buffer(&vertices).expect("encodes");

        let stride = std::mem::size_of::<Vertex>();
        let decoded = decode_stream(
            &encoded,
            vertices.len(),
            stride,
            Mode::Attributes,
            Filter::None,
        )
        .expect("decodes");
        assert_eq!(decoded.len(), vertices.len() * stride);

        let raw = unsafe {
            std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), vertices.len() * stride)
        };
        assert_eq!(decoded, raw, "decoded bytes match the original vertices");

        // The same stream into a preallocated buffer.
        let mut into = vec![0u8; vertices.len() * stride];
        decode_stream_into(&mut into, &encoded, vertices.len(), stride, Mode::Attributes, Filter::None)
            .expect("decodes into");
        assert_eq!(into, raw);
    }

    /// Garbage in must be an error, not a panic or silent corruption.
    #[test]
    fn decode_stream_reports_bad_input() {
        assert!(decode_stream(&[0xff; 8], 32, 12, Mode::Attributes, Filter::None).is_err());
        let mut out = vec![0u8; 24];
        assert!(decode_stream_into(&mut out, &[0xff; 8], 2, 12, Mode::Attributes, Filter::None).is_err());
    }

    /// A target of the wrong size is a programming error, not a silent overrun.
    #[test]
    fn decode_stream_into_checks_the_target() {
        assert!(decode_stream_into(&mut [0u8; 8], &[1, 2], 2, 12, Mode::Attributes, Filter::None).is_err());
    }

    #[test]
    fn decode_glb_passes_through_uncompressed_files() {
        let doc = json!({ "asset": { "version": "2.0" } });
        let glb = glb::encode(&doc, &[1, 2, 3, 4]).expect("writes");
        assert!(!needs_decode(&glb));
        let out = decode_glb(&glb).expect("passes through");
        assert!(matches!(out, Cow::Borrowed(_)), "no copy for a plain GLB");
        assert_eq!(out.as_ref(), glb);
    }

    /// The rewrite has to survive a file that only needs the KTX2 lift: the
    /// binary chunk is reused and only the JSON moves.
    #[test]
    fn decode_glb_keeps_the_bin_chunk_for_a_json_only_rewrite() {
        let image = vec![0xABu8; 64];
        let doc = json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["KHR_texture_basisu"],
            "extensionsRequired": ["KHR_texture_basisu"],
            "images": [{ "mimeType": "image/ktx2", "bufferView": 0 }],
            "textures": [{ "extensions": { "KHR_texture_basisu": { "source": 0 } } }],
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": image.len() }],
            "buffers": [{ "byteLength": image.len() }],
        });
        let glb = glb::encode(&doc, &image).expect("writes");
        let out = decode_glb(&glb).expect("rewrites");
        let chunks = glb::parse(&out).expect("parses");
        assert_eq!(chunks.bin, &image[..], "the payload is untouched");
        let doc = glb::parse_json(chunks.json).expect("json");
        assert_eq!(doc["textures"][0]["source"], 0);
        assert!(doc["textures"][0].get("extensions").is_none());
        assert!(doc.get("extensionsUsed").is_none());
        assert!(doc.get("extensionsRequired").is_none());
    }

    /// Builds the smallest KTX2 stream that carries a data-format descriptor:
    /// identifier + the nine header fields + the descriptor offset, then a DFD
    /// whose first sample has `channel_type` = `channel`.
    fn ktx2_with_channel(channel: u8) -> Vec<u8> {
        /// Where the fixture puts its DFD: 12 + 36 header + 32 index.
        const DFD_OFFSET: u32 = 80;
        let mut bytes = KTX2_IDENTIFIER.to_vec();
        bytes.extend_from_slice(&[0_u8; 36]);
        // dfdByteOffset, then dfdByteLength / kv pair / supercompression.
        bytes.extend_from_slice(&DFD_OFFSET.to_le_bytes());
        bytes.extend_from_slice(&[0_u8; 4]);
        bytes.extend_from_slice(&[0_u8; 24]);
        assert_eq!(bytes.len(), DFD_OFFSET as usize);
        let mut dfd = vec![0_u8; KTX2_DFD_FIRST_SAMPLE + KTX2_DFD_SAMPLE_CHANNEL_TYPE + 1];
        dfd[KTX2_DFD_FIRST_SAMPLE + KTX2_DFD_SAMPLE_CHANNEL_TYPE] = channel;
        bytes.extend_from_slice(&dfd);
        bytes
    }

    /// A GLB whose single image is `image`, embedded in the BIN chunk.
    fn glb_with_image(image: Vec<u8>) -> (Value, Vec<u8>) {
        let doc = json!({
            "asset": { "version": "2.0" },
            "images": [{ "mimeType": "image/ktx2", "bufferView": 0 }],
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": image.len() }],
        });
        (doc, image)
    }

    /// The bug this fix exists for: Bevy sizes the UASTC slice with the BC4
    /// block size (8) instead of UASTC's own 16, so an `RRR` texture kills the
    /// whole GLB. Re-labelling it `RGB` sends it down the BC7/16-byte path.
    #[test]
    fn widen_rewrites_rrr() {
        let (doc, mut bin) = glb_with_image(ktx2_with_channel(UASTC_CHANNEL_RRR));
        let at = KTX2_DFD_OFFSET_FIELD;
        let dfd = u32::from_le_bytes(bin[at..at + 4].try_into().unwrap()) as usize;
        let byte = dfd + KTX2_DFD_FIRST_SAMPLE + KTX2_DFD_SAMPLE_CHANNEL_TYPE;
        assert_eq!(bin[byte], UASTC_CHANNEL_RRR, "fixture starts as RRR");

        assert_eq!(widen_uastc_channel_layout(&doc, &mut bin), 1);
        assert_eq!(bin[byte], UASTC_CHANNEL_RGB);
    }

    /// The layouts Bevy already handles must survive untouched — a widen is a
    /// workaround, not a blanket re-encode.
    #[test]
    fn widen_leaves_other_layouts_alone() {
        for channel in [UASTC_CHANNEL_RGB, 3, 5, 6] {
            let (doc, mut bin) = glb_with_image(ktx2_with_channel(channel));
            assert_eq!(widen_uastc_channel_layout(&doc, &mut bin), 0, "{channel}");
            let at = KTX2_DFD_OFFSET_FIELD;
            let dfd = u32::from_le_bytes(bin[at..at + 4].try_into().unwrap()) as usize;
            let byte = dfd + KTX2_DFD_FIRST_SAMPLE + KTX2_DFD_SAMPLE_CHANNEL_TYPE;
            assert_eq!(bin[byte], channel);
        }
    }

    #[test]
    fn widen_ignores_foreign_and_external_images() {
        // Not a KTX2 stream: the descriptor offset would be read from garbage.
        let (doc, mut bin) = glb_with_image(vec![0_u8; 64]);
        assert_eq!(widen_uastc_channel_layout(&doc, &mut bin), 0);
        assert!(bin.iter().all(|b| *b == 0), "nothing rewritten");

        // An external image is a sibling file the reader does not own.
        let mut doc = json!({
            "images": [{ "mimeType": "image/ktx2", "uri": "a.ktx2" }],
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 64 }],
        });
        let mut bin = ktx2_with_channel(UASTC_CHANNEL_RRR);
        assert_eq!(widen_uastc_channel_layout(&doc, &mut bin), 0);
        // A `bufferView` past the end of the BIN chunk must not panic.
        doc["images"] = json!([{ "mimeType": "image/ktx2", "bufferView": 3 }]);
        assert_eq!(widen_uastc_channel_layout(&doc, &mut bin), 0);
    }

    #[test]
    fn external_buffers_are_rejected() {
        let doc = json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["EXT_meshopt_compression"],
            "buffers": [{ "uri": "a.bin", "byteLength": 4 }],
        });
        let glb = glb::encode(&doc, &[]).expect("writes");
        assert!(decode_glb(&glb).is_err());
    }

    /// ETC1S/BasisLZ is the one KTX2 flavour Bevy cannot transcode, and it
    /// fails as an invisible texture rather than an error. Both signals the
    /// spec offers have to be recognized.
    #[test]
    fn etc1s_images_are_reported() {
        let (doc, bin) = glb_with_image(ktx2_with_channel(UASTC_CHANNEL_RGB));
        assert!(first_etc1s_image(&doc, &bin).is_none(), "UASTC is fine");

        let mut baselz = bin.clone();
        baselz[KTX2_SUPERCOMPRESSION_FIELD..KTX2_SUPERCOMPRESSION_FIELD + 4]
            .copy_from_slice(&KTX2_SUPERCOMPRESSION_BASISLZ.to_le_bytes());
        assert!(first_etc1s_image(&doc, &baselz).is_some(), "supercompression");

        let mut etc1s = bin;
        let dfd = u32::from_le_bytes(etc1s[KTX2_DFD_OFFSET_FIELD..][..4].try_into().unwrap()) as usize;
        etc1s[dfd + KTX2_DFD_COLOR_MODEL] = KTX2_MODEL_ETC1S;
        assert!(first_etc1s_image(&doc, &etc1s).is_some(), "descriptor model");
    }
}
