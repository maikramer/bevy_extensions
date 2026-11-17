//! GLB container parsing and encoding — the one implementation.
//!
//! Three places used to parse GLB headers independently (the decoder, the
//! bounds reader and their test helpers), each with its own idea of how much
//! validation was enough. This module owns both directions: [`parse`] splits a
//! file into its JSON and BIN chunks, [`Layout`] says where the payloads land
//! in an encoded container so a caller can write straight into the final
//! buffer, and [`encode`] is the simple owned version.

use anyhow::{Context, Result, bail};

/// GLB container magic (`glTF`).
pub const MAGIC: u32 = 0x4654_6C67;
/// Chunk type tag for the JSON chunk.
pub const CHUNK_JSON: u32 = 0x4E4F_534A;
/// Chunk type tag for the binary chunk.
pub const CHUNK_BIN: u32 = 0x004E_4942;
/// Magic (4) + version (4) + total length (4).
pub const HEADER_LEN: usize = 12;
/// Chunk header: payload length (4) + chunk type (4).
pub const CHUNK_HEADER_LEN: usize = 8;
/// The only container version glTF 2.0 defines.
const VERSION: u32 = 2;

/// The chunks of a GLB, borrowed from the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct Chunks<'a> {
    pub json: &'a [u8],
    pub bin: &'a [u8],
}

/// Rounds up to the 4-byte alignment glTF requires for every chunk and view.
#[inline]
pub fn align4(len: usize) -> usize {
    (len + 3) & !3
}

/// Splits a GLB into its JSON and BIN chunks.
///
/// Rejects a file that is truncated relative to the length its own header
/// declares; a container that declares *fewer* bytes than it has on disk is
/// parsed anyway (exporters write a zero there now and then).
pub fn parse(bytes: &[u8]) -> Result<Chunks<'_>> {
    if bytes.len() < HEADER_LEN {
        bail!("not a GLB: {} bytes", bytes.len());
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into()?);
    if magic != MAGIC {
        bail!("not a GLB: bad magic {magic:#x}");
    }
    let declared = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
    let end = if declared > bytes.len() {
        bail!(
            "GLB header declares {declared} bytes but the file has {}",
            bytes.len()
        );
    } else if declared >= HEADER_LEN {
        declared
    } else {
        bytes.len()
    };
    let bytes = &bytes[..end];

    let mut json: Option<&[u8]> = None;
    let mut bin: &[u8] = &[];
    let mut cursor = HEADER_LEN;
    while cursor + CHUNK_HEADER_LEN <= bytes.len() {
        let len = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into()?) as usize;
        let kind = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into()?);
        let start = cursor + CHUNK_HEADER_LEN;
        let chunk_end = start
            .checked_add(len)
            .filter(|chunk_end| *chunk_end <= bytes.len())
            .context("GLB chunk runs past the end of the file")?;
        match kind {
            CHUNK_JSON => json = Some(&bytes[start..chunk_end]),
            CHUNK_BIN => bin = &bytes[start..chunk_end],
            _ => {} // unknown chunk types are skipped per spec
        }
        cursor = chunk_end;
    }
    Ok(Chunks {
        json: json.context("GLB has no JSON chunk")?,
        bin,
    })
}

/// Where everything lands in an encoded GLB.
///
/// Sizes are the *unpadded* payload lengths; the container pads both chunks to
/// 4 bytes (JSON with spaces, BIN with zeros) and the BIN chunk is omitted
/// entirely when it is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Padded JSON chunk length.
    pub json_len: usize,
    /// Padded BIN chunk payload length (0 when there is no BIN chunk).
    pub bin_len: usize,
    /// Offset of the BIN payload in the container.
    pub bin_start: usize,
    /// Total container length.
    pub total: usize,
}

impl Layout {
    pub fn new(json_unpadded: usize, bin_unpadded: usize) -> Result<Self> {
        let json_len = align4(json_unpadded);
        let bin_len = if bin_unpadded == 0 {
            0
        } else {
            align4(bin_unpadded)
        };
        let bin_start = HEADER_LEN + CHUNK_HEADER_LEN + json_len + CHUNK_HEADER_LEN;
        let total = if bin_len == 0 {
            HEADER_LEN + CHUNK_HEADER_LEN + json_len
        } else {
            bin_start + bin_len
        };
        // The container stores every length as a u32.
        if total > u32::MAX as usize {
            bail!("GLB would be {total} bytes, past the u32 chunk limit");
        }
        Ok(Self {
            json_len,
            bin_len,
            bin_start,
            total,
        })
    }
}

/// Writes the container header and both chunk headers.
///
/// The caller writes the JSON payload at offset 20 and the BIN payload at
/// [`Layout::bin_start`]; padding bytes are expected to stay zero (BIN) or be
/// spaces (JSON), which [`encode`] takes care of.
pub fn write_header(out: &mut [u8], layout: &Layout) -> Result<()> {
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&VERSION.to_le_bytes());
    out[8..12].copy_from_slice(
        &u32::try_from(layout.total)
            .context("GLB total length overflows u32")?
            .to_le_bytes(),
    );
    out[12..16].copy_from_slice(
        &u32::try_from(layout.json_len)
            .context("GLB JSON chunk overflows u32")?
            .to_le_bytes(),
    );
    out[16..20].copy_from_slice(&CHUNK_JSON.to_le_bytes());
    if layout.bin_len > 0 {
        let at = layout.bin_start - CHUNK_HEADER_LEN;
        out[at..at + 4].copy_from_slice(
            &u32::try_from(layout.bin_len)
                .context("GLB BIN chunk overflows u32")?
                .to_le_bytes(),
        );
        out[at + 4..at + 8].copy_from_slice(&CHUNK_BIN.to_le_bytes());
    }
    Ok(())
}

/// Serializes a JSON document plus its binary payload into a GLB.
pub fn encode(doc: &serde_json::Value, bin: &[u8]) -> Result<Vec<u8>> {
    let mut json = serde_json::to_vec(doc)?;
    // Both chunks are padded to 4 bytes: JSON with spaces, BIN with zeros.
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let layout = Layout::new(json.len(), bin.len())?;
    let mut out = vec![0u8; layout.total];
    write_header(&mut out, &layout)?;
    out[20..20 + json.len()].copy_from_slice(&json);
    if layout.bin_len > 0 {
        out[layout.bin_start..layout.bin_start + bin.len()].copy_from_slice(bin);
    }
    Ok(out)
}

/// The JSON chunk's document, parsed.
pub fn parse_json<'a>(json: &[u8]) -> Result<serde_json::Value> {
    serde_json::from_slice(json).context("parsing the GLB JSON chunk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_chunks() {
        let glb = encode(
            &serde_json::json!({ "asset": { "version": "2.0" } }),
            &[1, 2, 3, 4],
        )
        .expect("encodes");
        let chunks = parse(&glb).expect("parses");
        let doc = parse_json(chunks.json).expect("json");
        assert_eq!(doc["asset"]["version"], "2.0");
        assert_eq!(chunks.bin, &[1, 2, 3, 4]);
    }

    #[test]
    fn pads_and_sizes_chunks() {
        // A 1-byte payload and an odd-length JSON both need padding.
        let doc = serde_json::json!({ "a": 1 });
        let glb = encode(&doc, &[7]).expect("writes");
        assert_eq!(glb.len() % 4, 0, "the container is 4-byte aligned");
        let chunks = parse(&glb).expect("parses back");
        assert_eq!(chunks.bin.len() % 4, 0);
        assert_eq!(chunks.bin[0], 7);
        let parsed = parse_json(chunks.json).expect("json survives");
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn rejects_truncated_chunks() {
        let mut glb = encode(&serde_json::json!({ "a": 1 }), &[1, 2, 3, 4]).expect("writes");
        // Claim a chunk far longer than the file.
        glb[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&glb).is_err());
    }

    #[test]
    fn rejects_a_truncated_container() {
        let glb = encode(&serde_json::json!({ "a": 1 }), &[1, 2, 3, 4]).expect("writes");
        assert!(parse(&glb[..glb.len() - 2]).is_err());
    }

    #[test]
    fn empty_bin_chunk_is_omitted() {
        let glb = encode(&serde_json::json!({ "a": 1 }), &[]).expect("writes");
        let layout = Layout::new(serde_json::to_vec(&serde_json::json!({ "a": 1 })).unwrap().len(), 0)
            .expect("layout");
        assert_eq!(glb.len(), layout.total);
        assert_eq!(parse(&glb).expect("parses").bin.len(), 0);
    }

    #[test]
    fn layout_rejects_impossible_sizes() {
        // Past the u32 the container stores every length in.
        assert!(Layout::new(u32::MAX as usize + 32, 16).is_err());
        assert!(Layout::new(16, u32::MAX as usize + 32).is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse(b"").is_err());
        assert!(parse(b"not a glb at all").is_err());
        assert!(parse(&[0u8; 20]).is_err());
    }
}
