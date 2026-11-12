//! `.ahgt` heightgrid reader — the base term of the voxel field.
//!
//! Format (v1): `AHGT` magic, u16 version, u16 width, u16 depth, u32
//! metadata length (bytes 16..20), JSON metadata, then zlib/deflate u16
//! little-endian samples. The metadata carries `worldSize` and `maxHeight`.
//! The mapping matches the Viber terrain grid: the samples span
//! `(-world_size/2, +world_size/2)` on XZ and a raw `u16` maps to
//! `raw / 65535 * max_height` meters.

use std::path::Path;

use anyhow::{Context, Result, bail};
use bevy::math::Vec3;
use bevy_voxel_terrain::HeightField;

const AHGT_MAGIC: u32 = 0x5447_4841; // "AHGT" little-endian
const HEADER_LEN: usize = 20;
const MAX_WORLD_SIZE: f32 = 65_536.0;
const MAX_HEIGHT: f32 = 100_000.0;

/// A decoded `.ahgt` grid that implements [`HeightField`].
#[derive(Debug, Clone)]
pub struct AhgtHeightField {
    width: usize,
    depth: usize,
    world_size: f32,
    max_height: f32,
    heights: Vec<u16>,
}

impl AhgtHeightField {
    /// Loads `path`; the metadata supplies the extents.
    pub fn from_path(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    /// Loads `path` but lets the scene override the extents. This is what the
    /// editor uses when a `VoxelTerrain` component carries its own values.
    pub fn from_path_with(
        path: &Path,
        world_size: Option<f32>,
        max_height: Option<f32>,
    ) -> Result<Self> {
        let mut field = Self::from_path(path)?;
        if let Some(world_size) = world_size {
            field.world_size = validate_world_size(world_size)?;
        }
        if let Some(max_height) = max_height {
            field.max_height = validate_max_height(max_height)?;
        }
        Ok(field)
    }

    /// Decodes an in-memory `.ahgt` blob.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            bail!("AHGT: truncated header");
        }
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != AHGT_MAGIC {
            bail!("AHGT: bad magic 0x{magic:08x}");
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != 1 {
            bail!("AHGT: unsupported version {version}");
        }
        let width = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let depth = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        if width == 0 || depth == 0 {
            bail!("AHGT: empty {width}x{depth} grid");
        }
        let meta_len = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
        let meta_start = HEADER_LEN;
        if meta_start + meta_len > bytes.len() {
            bail!("AHGT: truncated metadata block");
        }
        let meta: serde_json::Value =
            serde_json::from_slice(&bytes[meta_start..meta_start + meta_len])
                .context("AHGT: invalid metadata JSON")?;
        let world_size = meta["worldSize"].as_f64().unwrap_or(0.0) as f32;
        let max_height = meta["maxHeight"].as_f64().unwrap_or(0.0) as f32;
        let world_size = validate_world_size(world_size)?;
        let max_height = validate_max_height(max_height)?;

        let payload = &bytes[meta_start + meta_len..];
        let zlib_wrapped = payload.len() >= 2
            && (payload[0] & 0x0f) == 0x08
            && ((payload[0] as u16) << 8 | payload[1] as u16).is_multiple_of(31);
        let expected = width
            .checked_mul(depth)
            .and_then(|texels| texels.checked_mul(2))
            .ok_or_else(|| anyhow::anyhow!("AHGT: {width}x{depth} grid overflows usize"))?;
        let mut raw = Vec::new();
        raw.try_reserve(expected).map_err(|_| {
            anyhow::anyhow!(
                "AHGT: header requests {expected} bytes of height data ({:.1} GiB) — \
                 refusing the allocation (corrupt header?)",
                expected as f64 / (1024.0 * 1024.0 * 1024.0)
            )
        })?;
        {
            use std::io::Read;
            let mut zlib_failed = false;
            if zlib_wrapped {
                let mut decoder = flate2::read::ZlibDecoder::new(payload).take(expected as u64 + 1);
                if decoder.read_to_end(&mut raw).is_err() {
                    zlib_failed = true;
                    raw.clear();
                }
            }
            if !zlib_wrapped || zlib_failed {
                raw.clear();
                let mut decoder =
                    flate2::read::DeflateDecoder::new(payload).take(expected as u64 + 1);
                decoder
                    .read_to_end(&mut raw)
                    .context("AHGT: deflate decompress failed")?;
            }
        }
        if raw.len() < expected {
            bail!(
                "AHGT: height payload too small ({} bytes for {width}x{depth})",
                raw.len()
            );
        }
        let mut heights = Vec::with_capacity(width * depth);
        for i in 0..width * depth {
            heights.push(u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]));
        }
        Ok(Self {
            width,
            depth,
            world_size,
            max_height,
            heights,
        })
    }

    /// Grid width in samples.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Grid depth in samples.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// World span on XZ (meters).
    pub fn world_size(&self) -> f32 {
        self.world_size
    }

    /// Height of a fully-white sample (meters).
    pub fn max_height(&self) -> f32 {
        self.max_height
    }

    fn step_x(&self) -> f32 {
        self.world_size / (self.width.max(2) - 1) as f32
    }

    fn step_z(&self) -> f32 {
        self.world_size / (self.depth.max(2) - 1) as f32
    }

    fn cell_height(&self, x: usize, z: usize) -> f32 {
        self.heights[z * self.width + x] as f32 / u16::MAX as f32 * self.max_height
    }

    fn bilinear(&self, x: f32, z: f32) -> f32 {
        let half = self.world_size * 0.5;
        let fx = ((x + half) / self.step_x()).clamp(0.0, (self.width - 1) as f32);
        let fz = ((z + half) / self.step_z()).clamp(0.0, (self.depth - 1) as f32);
        let x0 = fx.floor() as usize;
        let z0 = fz.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let z1 = (z0 + 1).min(self.depth - 1);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;
        let h00 = self.cell_height(x0, z0);
        let h10 = self.cell_height(x1, z0);
        let h01 = self.cell_height(x0, z1);
        let h11 = self.cell_height(x1, z1);
        (h00 * (1.0 - tx) + h10 * tx) * (1.0 - tz) + (h01 * (1.0 - tx) + h11 * tx) * tz
    }
}

impl HeightField for AhgtHeightField {
    fn sample(&self, world_x: f32, world_z: f32) -> f32 {
        self.bilinear(world_x, world_z)
    }

    fn sample_normal(&self, world_x: f32, world_z: f32, epsilon: f32) -> Vec3 {
        let e = if epsilon.is_finite() && epsilon > 0.0 {
            epsilon
        } else {
            self.step_x().min(self.step_z()).max(1e-3)
        };
        let hx0 = self.sample(world_x - e, world_z);
        let hx1 = self.sample(world_x + e, world_z);
        let hz0 = self.sample(world_x, world_z - e);
        let hz1 = self.sample(world_x, world_z + e);
        Vec3::new(hx0 - hx1, 2.0 * e, hz0 - hz1).normalize_or_zero()
    }

    fn max_height(&self) -> f32 {
        self.max_height
    }

    fn range_over(&self, min_x: f32, min_z: f32, max_x: f32, max_z: f32) -> Option<(f32, f32)> {
        let half = self.world_size * 0.5;
        let x0 = ((min_x + half) / self.step_x()).floor().max(0.0) as usize;
        let z0 = ((min_z + half) / self.step_z()).floor().max(0.0) as usize;
        let x1 = (((max_x + half) / self.step_x()).ceil().max(0.0) as usize + 1).min(self.width);
        let z1 = (((max_z + half) / self.step_z()).ceil().max(0.0) as usize + 1).min(self.depth);
        if x0 >= x1 || z0 >= z1 {
            return None;
        }
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for z in z0..z1 {
            for x in x0..x1 {
                let h = self.cell_height(x, z);
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        (lo <= hi).then_some((lo, hi))
    }
}

fn validate_world_size(world_size: f32) -> Result<f32> {
    if !world_size.is_finite() || world_size <= 0.0 || world_size > MAX_WORLD_SIZE {
        bail!("AHGT: worldSize {world_size} outside (0, {MAX_WORLD_SIZE}]");
    }
    Ok(world_size)
}

fn validate_max_height(max_height: f32) -> Result<f32> {
    if !max_height.is_finite() || max_height <= 0.0 || max_height > MAX_HEIGHT {
        bail!("AHGT: maxHeight {max_height} outside (0, {MAX_HEIGHT}]");
    }
    Ok(max_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(
        grid: &[u16],
        width: usize,
        depth: usize,
        world_size: f32,
        max_height: f32,
    ) -> Vec<u8> {
        use std::io::Write as _;
        let meta = serde_json::json!({
            "worldSize": world_size,
            "maxHeight": max_height,
        });
        let meta = serde_json::to_vec(&meta).expect("meta");
        let mut raw = Vec::with_capacity(grid.len() * 2);
        for value in grid {
            raw.extend_from_slice(&value.to_le_bytes());
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&raw).expect("deflate");
        let payload = encoder.finish().expect("deflate");

        let mut out = Vec::new();
        out.extend_from_slice(&AHGT_MAGIC.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(depth as u16).to_le_bytes());
        // Bytes 10..16 are reserved in the v1 header.
        out.extend_from_slice(&[0u8; 6]);
        out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        out.extend_from_slice(&meta);
        out.extend_from_slice(&payload);
        out
    }

    #[test]
    fn decode_sample_and_range() {
        let bytes = encode(
            &[0, 32_767, 65_535, 0, 65_535, 0, 32_767, 65_535, 0],
            3,
            3,
            10.0,
            100.0,
        );
        let field = AhgtHeightField::from_bytes(&bytes).expect("decode");
        assert_eq!(field.width(), 3);
        assert_eq!(field.sample(0.0, 0.0), 100.0); // center sample is max
        assert_eq!(field.sample(-5.0, -5.0), 0.0);
        assert_eq!(field.sample(5.0, 5.0), 0.0);
        let (lo, hi) = field.range_over(-5.0, -5.0, 5.0, 5.0).expect("range");
        assert_eq!(lo, 0.0);
        assert_eq!(hi, 100.0);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode(&[0; 4], 2, 2, 10.0, 10.0);
        bytes[0] = b'X';
        assert!(AhgtHeightField::from_bytes(&bytes).is_err());
    }
}
