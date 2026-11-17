//! The asset reader: multi-root, meshopt-aware, streaming everything else.
//!
//! Sitting at the reader means the rest of the engine is untouched: Bevy's own
//! glTF loader receives a plain GLB and handles KTX2 textures and material
//! extensions as usual.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, ErasedAssetReader, PathStream,
    Reader, VecReader,
};
use bevy::asset::{AssetServer, Handle};
use bevy::log::warn;
use bevy::tasks::ConditionalSendFuture;
// Explicit, not `bevy::prelude::*`: since Bevy 0.20 the prelude carries
// `ContextExt` (`.context()` for `BevyError`), which collides with the
// `anyhow::Context` this crate uses throughout.
use bevy::app::App;
use bevy::asset::AssetApp;

use crate::decode::decode_glb_named;
use crate::pool;
use crate::settings::{GltfDecodeSettings, settings};

/// Wraps another [`AssetReader`], rewriting the glTF files it reads.
pub struct MeshoptAssetReader {
    inner: Box<dyn ErasedAssetReader>,
}

impl MeshoptAssetReader {
    /// Wraps `inner`.
    pub fn new(inner: Box<dyn ErasedAssetReader>) -> Self {
        Self { inner }
    }
}

/// Extensions worth inspecting; anything else is passed through untouched.
fn is_gltf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("glb") || e.eq_ignore_ascii_case("gltf"))
}

impl AssetReader for MeshoptAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let inner = self.inner.read(path).await?;
        // Everything that is not a glTF streams straight through: nothing here
        // inspects a PNG, a KTX2 or an OGG, and buffering it whole bought
        // nothing but peak RSS.
        if !is_gltf(path) {
            return Ok(Box::new(inner) as Box<dyn Reader + 'a>);
        }

        let mut reader = inner;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let name = path.display().to_string();
        match decode_glb_named(&bytes, Some(&name)) {
            Ok(Cow::Owned(decoded)) => Ok(Box::new(VecReader::new(decoded)) as Box<dyn Reader + 'a>),
            Ok(Cow::Borrowed(_)) => Ok(Box::new(VecReader::new(bytes)) as Box<dyn Reader + 'a>),
            Err(error) => {
                // A decode failure must not lose the asset: hand the original
                // bytes over and let the glTF loader report what it makes of
                // them. The warning names what was attempted, because the
                // decoder also widens quantization and lifts KTX2 sources —
                // "meshopt decode failed" sent people looking in the wrong
                // place.
                let extensions = crate::extensions::probe_glb(&bytes);
                warn!(
                    "{name}: rewrite failed ({error:#}); handing the original bytes over \
                     (meshopt {} quantização {} basisu {})",
                    extensions.meshopt, extensions.quantization, extensions.basisu
                );
                Ok(Box::new(VecReader::new(bytes)) as Box<dyn Reader + 'a>)
            }
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        self.inner.read_meta(path).await
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl ConditionalSendFuture<Output = Result<Box<PathStream>, AssetReaderError>> {
        self.inner.read_directory(path)
    }

    fn is_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl ConditionalSendFuture<Output = Result<bool, AssetReaderError>> {
        self.inner.is_directory(path)
    }
}

/// Tries several file readers in order — the first root that has the asset
/// wins.
///
/// This is the native port of the original `sharedAssets` plugin: the world's
/// own tree serves first (per-game overrides), the shared pool fills in the
/// rest, and no example keeps a copy of pool content on disk.
struct MultiRootFileReader {
    roots: Vec<Box<dyn ErasedAssetReader>>,
}

impl MultiRootFileReader {
    fn new(roots: &[PathBuf]) -> Self {
        Self {
            roots: roots
                .iter()
                .map(|root| {
                    Box::new(bevy::asset::io::file::FileAssetReader::new(root))
                        as Box<dyn ErasedAssetReader>
                })
                .collect(),
        }
    }
}

impl AssetReader for MultiRootFileReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        for reader in &self.roots {
            match reader.read(path).await {
                Ok(reader) => return Ok(reader),
                // Only `NotFound` falls through to the next root. A permissions
                // or IO error on the world root used to be swallowed and the
                // search moved on to the shared pool, loading a same-named
                // (possibly stale) file.
                Err(AssetReaderError::NotFound(_)) => {}
                Err(other) => return Err(other),
            }
        }
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let mut missing = AssetReaderError::NotFound(path.to_path_buf());
        for reader in &self.roots {
            match reader.read_meta(path).await {
                Ok(reader) => return Ok(reader),
                Err(AssetReaderError::NotFound(_)) => {}
                Err(other) => missing = other,
            }
        }
        Err(missing)
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl ConditionalSendFuture<Output = Result<Box<PathStream>, AssetReaderError>> {
        async move {
            let mut missing = AssetReaderError::NotFound(path.to_path_buf());
            for reader in &self.roots {
                match reader.read_directory(path).await {
                    Ok(stream) => return Ok(stream),
                    Err(AssetReaderError::NotFound(_)) => {}
                    Err(other) => missing = other,
                }
            }
            Err(missing)
        }
    }

    fn is_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl ConditionalSendFuture<Output = Result<bool, AssetReaderError>> {
        async move {
            let mut seen = false;
            for reader in &self.roots {
                match reader.is_directory(path).await {
                    Ok(true) => return Ok(true),
                    Ok(false) => seen = true,
                    Err(AssetReaderError::NotFound(_)) => {}
                    Err(other) => return Err(other),
                }
            }
            Ok(seen)
        }
    }
}

/// Registers the meshopt-aware multi-root reader over the default file
/// source. `asset_roots` is in precedence order (world root first, shared pool
/// last; the caller builds the list).
///
/// Must run **before** `AssetPlugin` is added, since the plugin snapshots the
/// registered sources when it builds.
pub fn register_asset_source(app: &mut App, asset_roots: Vec<PathBuf>) {
    // One shared list for the reader and for `asset_file_exists`: they used to
    // diverge, because the reader closed over the new roots while the lookup
    // kept the first ones.
    let roots: Arc<Vec<PathBuf>> = pool::register(asset_roots);
    app.register_asset_source(
        AssetSourceId::Default,
        AssetSourceBuilder::new(move || {
            Box::new(MeshoptAssetReader::new(Box::new(MultiRootFileReader::new(
                &roots,
            )))) as Box<dyn ErasedAssetReader>
        }),
    );
}

/// Loads a glTF with the policies from [`settings`].
///
/// The `gltf` crate validates `extensionsRequired` against what *it* knows and
/// rejects the whole file otherwise — and with `KHR_texture_basisu` a texture's
/// source lives inside the extension block, so the core validator also reports
/// `textures[0].source: Missing data`. Both complaints are about the
/// validator's vocabulary, not the data: the reader has already lifted the
/// source and dropped the requirements, so `BEVY_EXT_GLTF_VALIDATE=1` is worth
/// trying; the default keeps it off.
pub fn load_gltf(server: &AssetServer, path: String) -> Handle<bevy::gltf::Gltf> {
    load_gltf_with(server, path, settings())
}

/// [`load_gltf`] with explicit settings.
pub fn load_gltf_with(
    server: &AssetServer,
    path: String,
    settings: GltfDecodeSettings,
) -> Handle<bevy::gltf::Gltf> {
    server
        .load_builder()
        .with_settings(move |loader: &mut bevy::gltf::GltfLoaderSettings| {
            loader.validate = settings.validate;
        })
        .load(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    use bevy::asset::io::file::FileAssetReader;

    fn read(reader: &impl AssetReader, path: &Path) -> Result<Vec<u8>, AssetReaderError> {
        bevy::tasks::block_on(async {
            let mut reader = AssetReader::read(reader, path).await?;
            let mut out = Vec::new();
            reader.read_to_end(&mut out).await?;
            Ok(out)
        })
    }

    /// The reader must actually decode when it is handed a compressed asset —
    /// the decoder passing its own unit tests is not enough if the plumbing
    /// hands it different bytes.
    #[test]
    fn reader_decodes_a_pool_asset() {
        let Some(pool) = test_support::pool_root() else {
            eprintln!("shared-assets pool absent — skipping");
            return;
        };
        let meshes = pool.join("assets/meshes");
        let relative = test_support::first_compressed(&meshes, &pool).expect(
            "the pool ships meshopt-compressed GLBs; finding none means the detector is broken",
        );

        let reader = MeshoptAssetReader::new(Box::new(FileAssetReader::new(
            pool.to_string_lossy().into_owned(),
        )));
        let bytes = read(&reader, &relative).expect("reads");
        assert!(!bytes.is_empty(), "the reader returned bytes");
        assert!(
            !crate::decode::needs_decode(&bytes),
            "{}: the reader returned still-compressed bytes",
            relative.display()
        );
    }

    /// Non-glTF assets must come back byte-identical: the pass-through branch
    /// is the common case for the pool (textures, audio, terrain).
    #[test]
    fn non_gltf_assets_stream_through() {
        let dir = tempfile::tempdir().expect("tmpdir");
        std::fs::write(dir.path().join("map.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        let reader = MeshoptAssetReader::new(Box::new(FileAssetReader::new(
            dir.path().to_string_lossy().into_owned(),
        )));
        let bytes = read(&reader, Path::new("map.png")).expect("reads");
        assert_eq!(bytes, b"\x89PNG\r\n\x1a\n");
    }

    /// Multi-root: the world root serves first (local override), the pool fills
    /// in what is missing, and what neither has is `NotFound`.
    #[test]
    fn multi_root_reader_falls_through_to_the_pool() {
        let world = tempfile::tempdir().expect("tmpdir");
        let pool = tempfile::tempdir().expect("tmpdir");
        std::fs::write(world.path().join("local.txt"), b"local").unwrap();
        std::fs::write(pool.path().join("pooled.txt"), b"pool").unwrap();
        // The same name in both roots: the world one wins.
        std::fs::write(world.path().join("both.txt"), b"world").unwrap();
        std::fs::write(pool.path().join("both.txt"), b"pool").unwrap();

        let reader = MeshoptAssetReader::new(Box::new(MultiRootFileReader::new(&[
            world.path().to_path_buf(),
            pool.path().to_path_buf(),
        ])));
        assert_eq!(read(&reader, Path::new("local.txt")).unwrap(), b"local");
        assert_eq!(read(&reader, Path::new("pooled.txt")).unwrap(), b"pool");
        assert_eq!(read(&reader, Path::new("both.txt")).unwrap(), b"world");
        assert!(matches!(
            read(&reader, Path::new("fantasma.txt")),
            Err(AssetReaderError::NotFound(_))
        ));
    }
}
