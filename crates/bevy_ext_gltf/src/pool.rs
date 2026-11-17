//! Which asset files exist under the asset roots.
//!
//! `asset_file_exists` exists because a `load` of an absent path only fails
//! asynchronously with a warning, and the LOD ladder asks about `_lod1`/`_lod2`
//! siblings that usually are not there. It used to be a `stat` per call per
//! root; the non-streaming spawn path calls it once per `<GltfScene>`
//! instance, which is thousands of syscalls for a few hundred distinct URLs.
//!
//! The memo is keyed by the relative path, so a repeated question is a hash
//! lookup. It lives for the process: a regenerated pool needs a restart, which
//! is already true of everything the reader caches.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Cap on the memo, so a caller feeding it dynamic paths cannot grow it forever.
const MEMO_LIMIT: usize = 8192;

static ROOTS: OnceLock<Arc<Vec<PathBuf>>> = OnceLock::new();
static MEMO: Mutex<Option<HashMap<Box<str>, bool>>> = Mutex::new(None);

/// Records the asset roots (first registration wins) and hands back the shared
/// list, so the reader and [`asset_file_exists`] cannot disagree about them.
pub(crate) fn register(roots: Vec<PathBuf>) -> Arc<Vec<PathBuf>> {
    if let Some(existing) = ROOTS.get() {
        crate::stats::warn_once("asset-roots", || {
            "bevy_ext_gltf: as asset roots já foram registadas; a primeira registração manda \
             (o reader e `asset_file_exists` usam a mesma lista)"
                .to_string()
        });
        return existing.clone();
    }
    ROOTS.get_or_init(|| Arc::new(roots)).clone()
}

/// The registered asset roots, in precedence order.
pub fn roots() -> Option<Arc<Vec<PathBuf>>> {
    ROOTS.get().cloned()
}

/// `true` when `path` (relative to the asset roots, with or without a leading
/// `/`) exists in one of them. `false` before the registration.
pub fn asset_file_exists(path: &str) -> bool {
    let Some(roots) = ROOTS.get() else {
        return false;
    };
    let rel = path.trim_start_matches('/');
    let cached = if cache_disabled() {
        None
    } else {
        let memo = MEMO.lock().unwrap_or_else(|poison| poison.into_inner());
        memo.as_ref().and_then(|memo| memo.get(rel).copied())
    };
    if let Some(hit) = cached {
        return hit;
    }
    let exists = roots.iter().any(|root| root.join(rel).is_file());
    if !cache_disabled() {
        let mut memo = MEMO.lock().unwrap_or_else(|poison| poison.into_inner());
        let memo = memo.get_or_insert_with(HashMap::new);
        if memo.len() < MEMO_LIMIT {
            memo.insert(rel.into(), exists);
        }
    }
    exists
}

/// Forgets every memoized answer. Tests and tooling that rewrite the pool
/// between passes need this; the game never does.
pub fn clear_cache() {
    let mut memo = MEMO.lock().unwrap_or_else(|poison| poison.into_inner());
    *memo = None;
}

fn cache_disabled() -> bool {
    std::env::var("BEVY_EXT_GLTF_POOL_CACHE").is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "off"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registration is process-global; tests that need their own roots run
    /// in one test to keep the ordering honest.
    #[test]
    fn existence_is_memoized_per_path() {
        let dir = tempfile::tempdir().expect("tmpdir");
        std::fs::write(dir.path().join("there.glb"), b"x").expect("writes");
        if roots().is_none() {
            register(vec![dir.path().to_path_buf()]);
        }
        let root = roots().expect("roots");
        if root.first().map(|p| p.as_path()) != Some(dir.path()) {
            eprintln!("outra registração já aconteceu — a saltar");
            return;
        }
        clear_cache();
        assert!(asset_file_exists("there.glb"));
        assert!(asset_file_exists("/there.glb"), "leading slash is ignored");
        assert!(!asset_file_exists("missing.glb"));
        // The memo answers "late.glb" *before* it exists, and keeps answering
        // that after it appears: that is the point of the cache.
        assert!(!asset_file_exists("late.glb"));
        std::fs::write(dir.path().join("late.glb"), b"x").expect("writes");
        assert!(!asset_file_exists("late.glb"));
        clear_cache();
        assert!(asset_file_exists("late.glb"), "clear_cache re-reads the disk");
    }
}
