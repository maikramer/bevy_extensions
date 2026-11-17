//! The glTF extensions this crate rewrites, and the cheap probe for them.
//!
//! `needs_rewrite` used to parse the whole JSON chunk of every GLB it was
//! handed — and it was handed every asset, three times per rewritten file.
//! The probe here looks for the extension names as raw substrings first: a
//! file that never mentions them cannot declare them, so the parse only runs
//! when there is something to confirm.

use crate::glb;

/// `EXT_meshopt_compression` — buffer views that need expanding.
pub const MESHOPT: &str = "EXT_meshopt_compression";
/// `KHR_mesh_quantization` — attributes that need widening to `f32`.
pub const QUANTIZATION: &str = "KHR_mesh_quantization";
/// `KHR_texture_basisu` — texture sources that need lifting into the core.
pub const BASISU: &str = "KHR_texture_basisu";
/// `KHR_draco_mesh_compression` — geometry this crate cannot decode.
pub const DRACO: &str = "KHR_draco_mesh_compression";

/// Everything the decoder rewrites.
pub const REWRITTEN: [&str; 3] = [MESHOPT, QUANTIZATION, BASISU];

/// Which of the rewritten extensions a file declares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Probe {
    pub meshopt: bool,
    pub quantization: bool,
    pub basisu: bool,
}

impl Probe {
    /// True when any extension needs work, i.e. when the file is worth parsing.
    pub fn any(&self) -> bool {
        self.meshopt || self.quantization || self.basisu
    }
}

/// Probes a whole GLB. A file that is not a GLB, or whose JSON does not parse,
/// reports nothing to rewrite — the caller then hands the bytes to the loader,
/// which is where the real complaint belongs.
pub fn probe_glb(bytes: &[u8]) -> Probe {
    match glb::parse(bytes) {
        Ok(chunks) => probe_json(chunks.json),
        Err(_) => Probe::default(),
    }
}

/// Fast substring scan before parsing JSON: true when any rewritten extension
/// name appears in `json`.
pub(crate) fn has_rewritten_extension(json: &[u8]) -> bool {
    REWRITTEN.iter().any(|name| contains(json, name.as_bytes()))
}

/// Probes a JSON chunk.
///
/// The substring pass is the point: `extensionsUsed` is a short array near the
/// top, and most of the pool (collision meshes, props that are already plain)
/// never mentions any of these names.
pub fn probe_json(json: &[u8]) -> Probe {
    if !has_rewritten_extension(json) {
        return Probe::default();
    }
    let Ok(doc) = glb::parse_json(json) else {
        return Probe::default();
    };
    uses(&doc)
}

/// Reads `extensionsUsed` from a parsed document.
pub fn uses(doc: &serde_json::Value) -> Probe {
    let used = doc.get("extensionsUsed").and_then(|v| v.as_array());
    let has = |name: &str| {
        used.is_some_and(|used| used.iter().any(|e| e.as_str() == Some(name)))
    };
    Probe {
        meshopt: has(MESHOPT),
        quantization: has(QUANTIZATION),
        basisu: has(BASISU),
    }
}

/// `true` when `haystack` contains `needle`.
///
/// A plain substring search: `slice::starts_with` is a `memcmp`, so the run is
/// a scan for the first byte plus one comparison per candidate. That is far
/// cheaper than a JSON parse and, unlike a parse, it cannot fail.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some(&first) = needle.first() else {
        return true;
    };
    let mut at = 0;
    while at + needle.len() <= haystack.len() {
        let Some(offset) = haystack[at..].iter().position(|b| *b == first) else {
            return false;
        };
        at += offset;
        if haystack[at..].starts_with(needle) {
            return true;
        }
        at += 1;
    }
    false
}

/// Removes `name` from an extension list, dropping the list when it empties.
pub fn strip(doc: &mut serde_json::Value, key: &str, name: &str) {
    let Some(array) = doc.get_mut(key).and_then(|v| v.as_array_mut()) else {
        return;
    };
    array.retain(|e| e.as_str() != Some(name));
    if array.is_empty() {
        doc.as_object_mut().map(|o| o.remove(key));
    }
}

/// Extensions a decoded GLB still *requires* but that nothing here understands.
///
/// Bevy refuses a file whose `extensionsRequired` names something it cannot
/// handle, and the failure it reports is about its own vocabulary rather than
/// the asset. Naming them up front turns "invisible model" into a message.
pub fn unsupported_required(doc: &serde_json::Value) -> Vec<String> {
    let known = [MESHOPT, QUANTIZATION, BASISU];
    doc.get("extensionsRequired")
        .and_then(|v| v.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|e| e.as_str())
                .filter(|name| !known.contains(name))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `true` when a texture/image still carries a Draco mesh block.
pub fn has_draco(doc: &serde_json::Value) -> bool {
    doc.get("meshes")
        .and_then(|v| v.as_array())
        .is_some_and(|meshes| {
            meshes.iter().any(|mesh| {
                mesh.get("primitives")
                    .and_then(|v| v.as_array())
                    .is_some_and(|primitives| {
                        primitives.iter().any(|primitive| {
                            primitive
                                .get("extensions")
                                .and_then(|e| e.get(DRACO))
                                .is_some()
                        })
                    })
            })
        })
}

/// `true` when any accessor declares a sparse substitution.
pub fn has_sparse_accessors(doc: &serde_json::Value) -> bool {
    doc.get("accessors")
        .and_then(|v| v.as_array())
        .is_some_and(|accessors| accessors.iter().any(|a| a.get("sparse").is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glb_with(used: &[&str]) -> Vec<u8> {
        glb::encode(
            &serde_json::json!({ "asset": { "version": "2.0" }, "extensionsUsed": used }),
            &[],
        )
        .expect("encodes")
    }

    #[test]
    fn probe_reads_extensions_used() {
        let probe = probe_glb(&glb_with(&[MESHOPT, BASISU]));
        assert!(probe.meshopt);
        assert!(probe.basisu);
        assert!(!probe.quantization);
        assert!(probe.any());
    }

    #[test]
    fn probe_skips_files_without_the_names() {
        let glb = glb_with(&["KHR_materials_emissive_strength"]);
        assert_eq!(probe_glb(&glb), Probe::default());
    }

    #[test]
    fn probe_ignores_a_name_outside_extensions_used() {
        // The substring is there (a node is called that), the declaration is
        // not: the parse confirms and the probe stays empty.
        let glb = glb::encode(
            &serde_json::json!({
                "asset": { "version": "2.0" },
                "nodes": [{ "name": "EXT_meshopt_compression" }],
            }),
            &[],
        )
        .expect("encodes");
        assert_eq!(probe_glb(&glb), Probe::default());
    }

    #[test]
    fn probe_of_garbage_is_empty() {
        assert_eq!(probe_glb(b"not a glb"), Probe::default());
        assert_eq!(probe_json(b"{ this is not json"), Probe::default());
    }

    #[test]
    fn substring_search_matches_edges() {
        assert!(contains(b"abc", b"abc"));
        assert!(contains(b"xxabcxx", b"abc"));
        assert!(contains(b"xxabc", b"abc"));
        assert!(!contains(b"ab", b"abc"));
        assert!(!contains(b"abcc", b"abd"));
        assert!(contains(b"anything", b""));
    }

    #[test]
    fn strip_drops_the_list_when_it_empties() {
        let mut doc = serde_json::json!({
            "extensionsUsed": [MESHOPT],
            "extensionsRequired": [MESHOPT, BASISU],
        });
        strip(&mut doc, "extensionsUsed", MESHOPT);
        strip(&mut doc, "extensionsRequired", MESHOPT);
        assert!(doc.get("extensionsUsed").is_none());
        assert_eq!(doc["extensionsRequired"][0], BASISU);
    }

    #[test]
    fn unsupported_required_names_the_unknowns() {
        let doc = serde_json::json!({ "extensionsRequired": [MESHOPT, DRACO, "EXT_x"] });
        assert_eq!(unsupported_required(&doc), vec![DRACO.to_string(), "EXT_x".into()]);
    }

    #[test]
    fn detects_draco_and_sparse() {
        let doc = serde_json::json!({
            "meshes": [{ "primitives": [{ "extensions": { DRACO: {} } }] }],
            "accessors": [{ "sparse": {} }],
        });
        assert!(has_draco(&doc));
        assert!(has_sparse_accessors(&doc));
    }
}
