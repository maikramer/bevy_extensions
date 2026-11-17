//! Test helpers shared by the crate's unit tests.
//!
//! Kept out of the individual `#[cfg(test)]` modules so the pool scan and the
//! fixture fallback exist once.

use std::path::{Path, PathBuf};

/// The real asset pool when `VAEL_ROOT` is set; the fixtures otherwise.
///
/// `VAEL_ROOT` points at the Vael checkout (`VAEL_ROOT=/path/to/Vael`), whose
/// `assets/public` holds the game pool. Without it the two small real GLBs in
/// `tests/fixtures/` cover the same code paths (meshopt + quantization + KTX2),
/// so CI runs the decoder without the game checkout.
pub fn pool_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("VAEL_ROOT").map(PathBuf::from) {
        let candidate = root.join("assets/public");
        if candidate.join("assets/meshes").is_dir() {
            return Some(candidate);
        }
    }
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fixtures.join("assets/meshes").is_dir().then_some(fixtures)
}

/// `<pool>/assets/meshes`.
#[cfg(feature = "legacy")]
pub fn mesh_dir() -> Option<PathBuf> {
    pool_root().map(|root| root.join("assets/meshes"))
}

/// Every `.glb` under `dir`, recursively.
pub fn walk_glb(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_glb(&path, out);
        } else if path.extension().is_some_and(|e| e == "glb") {
            out.push(path);
        }
    }
}

/// First meshopt-compressed GLB under `dir`, as a path relative to `root`.
pub fn first_compressed(dir: &Path, root: &Path) -> Option<PathBuf> {
    let mut paths = Vec::new();
    walk_glb(dir, &mut paths);
    paths.sort();
    paths.into_iter().find_map(|path| {
        let compressed = std::fs::read(&path).is_ok_and(|bytes| crate::decode::needs_decode(&bytes));
        compressed.then(|| path.strip_prefix(root).ok().map(Path::to_path_buf))?
    })
}
