//! Decode policies, read once from the environment.
//!
//! Three of the four knobs exist because the crate carries workarounds for
//! Bevy behaviour that may change: the `UASTC_RRR` re-label (`bevy_image`'s
//! BC4 slice sizing) and the quantized-attribute widening (`Mesh` refusing
//! `Snorm8x4` for `ATTRIBUTE_TANGENT`). Keeping them as *policy* means the
//! next person can A/B them instead of rediscovering why they exist.
//!
//! ```text
//! BEVY_EXT_GLTF_VALIDATE=1        # run the gltf validator (off by default)
//! BEVY_EXT_GLTF_UASTC_RELABEL=0   # keep single-channel UASTC as RRR
//! BEVY_EXT_GLTF_DEQUANTIZE=0      # A/B only: meshes will panic
//! BEVY_EXT_GLTF_MIPS=inline|background|off
//! ```

use std::sync::OnceLock;

/// Where the CPU mip chains are built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mipmaps {
    /// On the main thread, in the `Update` that sees the asset.
    Inline,
    /// Off the main thread for large images; small ones stay inline.
    Background,
    /// Never (the images keep their single level).
    Off,
}

/// How the reader treats the extensions it knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GltfDecodeSettings {
    /// Run the `gltf` crate's validator on load.
    ///
    /// Off by default: the validator rejects files that *require* extensions
    /// it does not know, and Bevy reports that as its own vocabulary problem
    /// rather than the asset's. The reader normalizes those away first, so
    /// this can usually be turned on — which is exactly why it is a knob.
    pub validate: bool,
    /// Re-label single-channel UASTC textures as `RGB` (the BC4 workaround).
    pub uastc_relabel: bool,
    /// Widen quantized POSITION/NORMAL/TANGENT to `f32`. A/B only — with this
    /// off, `Mesh::insert_attribute` panics on the missing attribute.
    pub dequantize: bool,
    /// Mip chain policy for plain RGBA images.
    pub mipmaps: Mipmaps,
}

impl Default for GltfDecodeSettings {
    fn default() -> Self {
        Self {
            validate: false,
            uastc_relabel: true,
            dequantize: true,
            mipmaps: Mipmaps::Background,
        }
    }
}

/// The settings for this process (`BEVY_EXT_GLTF_*`).
pub fn settings() -> GltfDecodeSettings {
    static SETTINGS: OnceLock<GltfDecodeSettings> = OnceLock::new();
    *SETTINGS.get_or_init(GltfDecodeSettings::from_env)
}

impl GltfDecodeSettings {
    fn from_env() -> Self {
        let default = Self::default();
        Self {
            validate: flag("BEVY_EXT_GLTF_VALIDATE", default.validate),
            uastc_relabel: flag("BEVY_EXT_GLTF_UASTC_RELABEL", default.uastc_relabel),
            dequantize: flag("BEVY_EXT_GLTF_DEQUANTIZE", default.dequantize),
            mipmaps: match std::env::var("BEVY_EXT_GLTF_MIPS").ok().as_deref() {
                Some("inline") => Mipmaps::Inline,
                Some("off") => Mipmaps::Off,
                Some("background") => Mipmaps::Background,
                _ => default.mipmaps,
            },
        }
    }
}

/// `default` unless the variable is set; `0`/`false`/`off`/empty turn it off.
fn flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => !matches!(value.as_str(), "" | "0" | "false" | "off"),
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_the_workarounds_on() {
        let settings = GltfDecodeSettings::default();
        assert!(!settings.validate, "the validator stays a knob, not a gate");
        assert!(settings.uastc_relabel);
        assert!(settings.dequantize);
        assert_eq!(settings.mipmaps, Mipmaps::Background);
    }
}
