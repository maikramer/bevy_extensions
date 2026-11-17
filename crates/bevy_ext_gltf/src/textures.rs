//! Mipmaps + samplers for loaded world textures — THE single writer.
//!
//! Bevy's image loader does NOT generate mipmaps for plain PNG/JPG files
//! (only KTX2/DDS ship them) and the default sampler ships with
//! `anisotropy_clamp: 1`. World-space tiled ground textures (the simple-rpg
//! terrain tiles `vale_grass.png` every 5 m) then alias into crawling pixel
//! speckle — worst when the camera moves. This module generates a
//! box-filter mip chain CPU-side for uncompressed 4-byte-per-pixel images on
//! load, and raises the sampler anisotropy to 8.
//!
//! **Sampler ownership lives HERE and only here.** Two systems used to write
//! the same image's sampler: the mip patch below (ClampToEdge + aniso 8) and
//! `terrain::drop_failed_terrain_textures` (REPEAT). They run unordered in
//! `Update`, the render world extracts whatever state wins the race, and one
//! run in two shipped a ClampToEdge ground texture — with world/meter UVs
//! that stretches the border texel into long streaks ("estrada e relva
//! quebradas"). The fix is one registry ([`WorldTiledTextures`], filled by
//! the terrain bootstrap at `server.load` time, i.e. strictly before the
//! `Added` event can fire) and one writer: [`patch_image`] decides the
//! final sampler in a single pass. Do NOT mutate `image.sampler` elsewhere.

use std::collections::{HashMap, HashSet};

use bevy::asset::{AssetEvent, AssetId, Assets};
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};

use crate::settings::{Mipmaps, settings};

/// The deepest chain the CPU builder emits (1×1 stops earlier).
const MAX_MIP_LEVELS: usize = 12;

/// Asset ids whose textures use world-space tiled UVs (terrain chunks, road
/// ribbons, junction discs, ground decals — meter-unit UVs that need REPEAT).
///
/// The terrain bootstrap registers each id in the same statement that calls
/// `AssetServer::load`, so the registration is visible to
/// [`generate_mipmaps_on_load`] before the image's `Added` event is ever
/// read: the first (and only) patch already carries the tiled sampler.
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct WorldTiledTextures(HashSet<AssetId<Image>>);

impl WorldTiledTextures {
    /// Marks a texture id as world-tiled. Idempotent.
    pub fn register(&mut self, id: AssetId<Image>) {
        self.0.insert(id);
    }

    /// True when `id` was registered as world-tiled.
    pub fn contains(&self, id: AssetId<Image>) -> bool {
        self.0.contains(&id)
    }
}

/// Plugin: registry resource, the load-patch system and the mip finisher.
pub struct TexturesPlugin;

impl Plugin for TexturesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldTiledTextures>()
            .init_resource::<PendingMips>()
            .add_systems(bevy::app::Update, (generate_mipmaps_on_load, finish_mipmaps));
    }
}

/// Mip chains that are being built off the main thread.
#[derive(Resource, Default)]
pub struct PendingMips(HashMap<AssetId<Image>, PendingMip>);

struct PendingMip {
    task: Task<Option<(Vec<u8>, u32)>>,
    world_tiled: bool,
    width: u32,
    height: u32,
}

/// Images at least this large get their chain built on the compute pool: a
/// 2048² box filter is 4 M pixels of work, and during loading every
/// millisecond of the main thread delays the veil.
const BACKGROUND_MIP_PIXELS: u32 = 1024 * 1024;

/// Loads a world-tiled texture and registers its id in the same statement —
/// registration has to be visible before the image's `Added` event is ever
/// read, or the first patch would settle a ClampToEdge sampler and the
/// world-space UVs would smear into streaks. Every world-space-UV load site
/// (terrain chunks, road ribbons, junction discs, ground decals, tiled
/// primitives) goes through this.
pub fn load_tiled_image(
    server: &AssetServer,
    tiled: &mut WorldTiledTextures,
    url: &str,
) -> Handle<Image> {
    // strip leading '/' — bevy treats root-absolute asset paths as unapproved
    let handle: Handle<Image> = server.load(url.trim_start_matches('/').to_string());
    tiled.register(handle.id());
    handle
}

/// Patch every freshly-loaded image: settle its sampler and give it a mip
/// chain.
///
/// The sampler is written synchronously — that is the policy this module owns,
/// see the module docs. The chain is built here for small images and handed to
/// the compute pool for large ones (`BEVY_EXT_GLTF_MIPS=inline` forces the old
/// behaviour, `off` skips it).
pub fn generate_mipmaps_on_load(
    mut events: MessageReader<AssetEvent<Image>>,
    tiled: Res<WorldTiledTextures>,
    mut images: ResMut<Assets<Image>>,
    mut pending: ResMut<PendingMips>,
) {
    let policy = settings().mipmaps;
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::LoadedWithDependencies { id }) = event else {
            continue;
        };
        let Some(image) = images.get_mut(*id) else {
            continue;
        };
        let world_tiled = tiled.contains(*id);
        let image = image.into_inner();
        apply_sampler(image, world_tiled);
        if policy == Mipmaps::Off || !wants_mips(image) {
            continue;
        }
        let (width, height) = (image.width(), image.height());
        let defer = policy == Mipmaps::Background
            && width * height >= BACKGROUND_MIP_PIXELS
            && !pending.0.contains_key(id);
        if defer {
            let pool = AsyncComputeTaskPool::try_get();
            if let (Some(pool), Some(data)) = (pool, image.data.clone()) {
                // The task owns a copy: the image must stay usable while the
                // chain is built, and a hot reload can replace it under us.
                let task = pool.spawn(async move { build_mip_chain(&data, width, height) });
                pending.0.insert(
                    *id,
                    PendingMip {
                        task,
                        world_tiled,
                        width,
                        height,
                    },
                );
                continue;
            }
        }
        apply_mips_inline(image, world_tiled);
    }
}

/// Applies the chains that finished since the last frame.
pub fn finish_mipmaps(mut images: ResMut<Assets<Image>>, mut pending: ResMut<PendingMips>) {
    if pending.0.is_empty() {
        return;
    }
    let mut done = Vec::new();
    for (id, entry) in pending.0.iter_mut() {
        if let Some(result) = block_on(poll_once(&mut entry.task)) {
            done.push((*id, result, entry.world_tiled, entry.width, entry.height));
        }
    }
    for (id, result, world_tiled, width, height) in done {
        pending.0.remove(&id);
        let Some((chain, levels)) = result else {
            continue;
        };
        let Some(image) = images.get_mut(id) else {
            continue;
        };
        // A hot reload may have swapped the asset while the task ran: the
        // chain belongs to the old one.
        if image.texture_descriptor.mip_level_count > 1
            || image.width() != width
            || image.height() != height
        {
            continue;
        }
        apply_mip_chain(image.into_inner(), chain, levels, world_tiled);
    }
}

/// Sampler for textures with world-space (meter) UVs (`world/tile`, road
/// `arc/scale`): sem REPEAT os UVs > 1.0 clampeiam na borda e a textura
/// estica nos riscos do texel da borda. Linear + anisotropy 8 — wgpu exige
/// filtros lineares com aniso > 1.
pub fn world_tiled_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    })
}

/// True when the format is plain uncompressed RGBA/BGRA (1×1 blocks, 4
/// bytes) — the only layouts the CPU box filter here understands.
fn is_plain_rgba(format: bevy::render::render_resource::TextureFormat) -> bool {
    use bevy::render::render_resource::TextureFormat;
    matches!(
        format,
        TextureFormat::Rgba8UnormSrgb
            | TextureFormat::Rgba8Unorm
            | TextureFormat::Bgra8UnormSrgb
            | TextureFormat::Bgra8Unorm
    )
}

/// Patch a freshly-loaded image.
///
/// * `world_tiled` — the image was registered in [`WorldTiledTextures`]: its
///   sampler becomes [`world_tiled_sampler`] unconditionally (even if the
///   file ships its own mips).
/// * Otherwise: append a box-filter mip chain when the file has a single
///   level and set the linear + aniso 8 sampler. Images that already carry
///   mips (KTX2/DDS) and exotic formats pass through untouched.
pub fn patch_image(image: &mut Image, world_tiled: bool) {
    apply_sampler(image, world_tiled);
    apply_mips_inline(image, world_tiled);
}

/// The sampler half of [`patch_image`], applied the moment the asset arrives.
///
/// Kept separate so a deferred mip chain does not delay the one write
/// [`WorldTiledTextures`] exists to serialize.
pub fn apply_sampler(image: &mut Image, world_tiled: bool) {
    if world_tiled {
        image.sampler = world_tiled_sampler();
    }
}

/// The mip half of [`patch_image`], on the calling thread.
pub fn apply_mips_inline(image: &mut Image, world_tiled: bool) {
    if !wants_mips(image) {
        return;
    }
    let (width, height) = (image.width(), image.height());
    let built = match image.data.as_deref() {
        Some(data) => build_mip_chain(data, width, height),
        None => None,
    };
    if let Some((chain, levels)) = built {
        apply_mip_chain(image, chain, levels, world_tiled);
    }
}

/// True when this image can take a CPU mip chain.
fn wants_mips(image: &Image) -> bool {
    if image.texture_descriptor.mip_level_count > 1 {
        return false;
    }
    // Render targets (o viewport do editor externo, câmaras render-to-texture)
    // têm de ficar com 1 nível: o wgpu recusa uma view com vários mips como
    // color attachment ("texture view is not renderable").
    if image
        .texture_descriptor
        .usage
        .contains(bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT)
    {
        return false;
    }
    if !is_plain_rgba(image.texture_descriptor.format) {
        return false;
    }
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || (width <= 1 && height <= 1) {
        return false;
    }
    let Some(data) = image.data.as_ref() else {
        return false;
    };
    data.len() >= (width as usize) * (height as usize) * 4
}

/// Puts a finished chain on the image.
pub fn apply_mip_chain(image: &mut Image, chain: Vec<u8>, levels: u32, world_tiled: bool) {
    image.data = Some(chain);
    image.texture_descriptor.mip_level_count = levels;
    // Filtragem anisotrópica nas texturas de chão: em ângulo rasant o mip
    // único de maior compressão é que era amostrado, e é isso que cintila.
    // (Tiled já recebe aniso 8 via `world_tiled_sampler`.)
    if !world_tiled {
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            anisotropy_clamp: 8,
            ..ImageSamplerDescriptor::linear()
        });
    }
}

/// Box-filter mip chain over a plain 4-byte-per-pixel image.
///
/// The levels are concatenated into one buffer (level 0 first) and built in
/// place: level N is written into the tail of the same allocation level N-1
/// lives in, so the chain costs one allocation and one copy of the source.
///
/// Averaging the four bytes of a pixel as one `u32` is safe because each
/// channel sum (≤ 4 × 255 = 1020) stays inside the 16-bit slot the mask leaves
/// for it; the `>> 2` then matches the per-channel floor exactly.
pub fn build_mip_chain(data: &[u8], width: u32, height: u32) -> Option<(Vec<u8>, u32)> {
    let mip0 = (width as usize) * (height as usize) * 4;
    if width <= 1 && height <= 1 || data.len() < mip0 {
        return None;
    }

    let mut level_dims = [(0u32, 0u32); MAX_MIP_LEVELS];
    let mut level_offsets = [0usize; MAX_MIP_LEVELS];
    level_dims[0] = (width, height);
    level_offsets[0] = 0;
    let mut level_count = 1usize;
    let mut total = mip0;
    while level_count < MAX_MIP_LEVELS {
        let (lw, lh) = level_dims[level_count - 1];
        if lw <= 1 && lh <= 1 {
            break;
        }
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        level_dims[level_count] = (nw, nh);
        level_offsets[level_count] = total;
        total += (nw * nh * 4) as usize;
        level_count += 1;
    }
    if level_count <= 1 {
        return None;
    }

    let mut chain = Vec::with_capacity(total);
    chain.extend_from_slice(&data[..mip0]);
    chain.resize(total, 0);

    for lvl in 1..level_count {
        let (lw, lh) = level_dims[lvl - 1];
        let (nw, nh) = level_dims[lvl];
        let prev_start = level_offsets[lvl - 1];
        let next_start = level_offsets[lvl];
        let next_len = (nw * nh * 4) as usize;
        let (head, tail) = chain.split_at_mut(next_start);
        let prev = &head[prev_start..];
        let next = &mut tail[..next_len];
        downsample(prev, lw, lh, next, nw, nh);
    }
    Some((chain, level_count as u32))
}

/// One 2×2 box-filter step, four channels at a time.
fn downsample(prev: &[u8], lw: u32, lh: u32, next: &mut [u8], nw: u32, nh: u32) {
    #[inline]
    fn pixel(row: &[u8], at: usize) -> u32 {
        u32::from_ne_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]])
    }
    let lw_u = lw as usize;
    for y in 0..nh as usize {
        let sy = (y * 2).min(lh as usize - 1);
        let sy1 = (y * 2 + 1).min(lh as usize - 1);
        let row0 = &prev[sy * lw_u * 4..(sy + 1) * lw_u * 4];
        let row1 = &prev[sy1 * lw_u * 4..(sy1 + 1) * lw_u * 4];
        let out = &mut next[y * nw as usize * 4..(y + 1) * nw as usize * 4];
        for x in 0..nw as usize {
            let x0 = (x * 2).min(lw_u - 1) * 4;
            let x1 = (x * 2 + 1).min(lw_u - 1) * 4;
            let (a, b) = (pixel(row0, x0), pixel(row0, x1));
            let (c, d) = (pixel(row1, x0), pixel(row1, x1));
            // R/B in the low 16 bits slots, G/A in the high ones.
            let rb = ((a & 0x00FF_00FF)
                + (b & 0x00FF_00FF)
                + (c & 0x00FF_00FF)
                + (d & 0x00FF_00FF))
                >> 2;
            let ga = (((a >> 8) & 0x00FF_00FF)
                + ((b >> 8) & 0x00FF_00FF)
                + ((c >> 8) & 0x00FF_00FF)
                + ((d >> 8) & 0x00FF_00FF))
                >> 2;
            let value = (rb & 0x00FF_00FF) | ((ga & 0x00FF_00FF) << 8);
            out[x * 4..x * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    };

    fn rgba_image(width: u32, height: u32, fill: [u8; 4]) -> Image {
        let len = (width * height * 4) as usize;
        Image {
            data: Some(vec_of(fill, len)),
            data_order: Default::default(),
            texture_descriptor: TextureDescriptor {
                label: None,
                size: Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8UnormSrgb,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
            sampler: ImageSampler::Default,
            texture_view_descriptor: None,
            asset_usage: RenderAssetUsages::default(),
            copy_on_resize: false,
        }
    }

    fn vec_of(fill: [u8; 4], len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        for i in 0..len {
            v.push(fill[i % 4]);
        }
        v
    }

    /// Sampler de uma imagem patched: address mode e anisotropia efectivos.
    fn sampler_of(image: &Image) -> (ImageAddressMode, u16) {
        match &image.sampler {
            ImageSampler::Descriptor(d) => (d.address_mode_u, d.anisotropy_clamp),
            ImageSampler::Default => panic!("sampler should be an explicit descriptor"),
        }
    }

    /// Sólido: a cadeia de mips preserva a cor e produz os níveis esperados.
    #[test]
    fn test_mip_chain_on_solid_color() {
        let mut image = rgba_image(8, 4, [255, 0, 0, 255]);
        patch_image(&mut image, false);
        // 8x4 → 4x2 → 2x1 → 1x1
        assert_eq!(image.texture_descriptor.mip_level_count, 4);
        let data = image.data.as_ref().expect("data present");
        assert_eq!(data.len(), (32 + 8 + 2 + 1) * 4);
        assert!(data.chunks(4).all(|px| px == [255, 0, 0, 255]));
    }

    /// Já tem mips (KTX2/DDS): passa intacto.
    #[test]
    fn test_patch_skips_render_targets() {
        let mut image = Image::new_target_texture(
            64,
            64,
            bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb,
            None,
        );
        patch_image(&mut image, false);
        assert_eq!(image.texture_descriptor.mip_level_count, 1);
    }

    #[test]
    fn test_patch_skips_existing_mips() {
        let mut image = rgba_image(4, 4, [1, 2, 3, 4]);
        image.texture_descriptor.mip_level_count = 3;
        let before = image.data.clone();
        patch_image(&mut image, false);
        assert_eq!(image.data, before);
        assert_eq!(image.texture_descriptor.mip_level_count, 3);
    }

    /// Formatos comprimidos/não-RGBA passam intactos.
    #[test]
    fn test_patch_skips_unsupported_format() {
        use bevy::render::render_resource::TextureFormat;
        let mut image = rgba_image(4, 4, [0; 4]);
        image.texture_descriptor.format = TextureFormat::R16Uint;
        let before = image.data.clone();
        patch_image(&mut image, false);
        assert_eq!(image.data, before);
        assert_eq!(image.texture_descriptor.mip_level_count, 1);
        assert!(is_plain_rgba(TextureFormat::Rgba8UnormSrgb));
        assert!(!is_plain_rgba(TextureFormat::R16Uint));
    }

    /// 1×1: sem cadeia a acrescentar, permanece com um nível.
    #[test]
    fn test_patch_1x1_is_noop() {
        let mut image = rgba_image(1, 1, [9, 9, 9, 255]);
        patch_image(&mut image, false);
        assert_eq!(image.texture_descriptor.mip_level_count, 1);
    }

    /// World-tiled: REPEAT em u/v + aniso 8 + cadeia de mips, tudo no
    /// primeiro (e único) patch — é isto que remove a corrida de samplers.
    #[test]
    fn test_patch_world_tiled_gets_repeat_sampler_and_mips() {
        let mut image = rgba_image(8, 4, [255, 0, 0, 255]);
        patch_image(&mut image, true);
        assert_eq!(image.texture_descriptor.mip_level_count, 4);
        assert_eq!(sampler_of(&image), (ImageAddressMode::Repeat, 8));
        // v acompanha u — sem isto um repeat só num eixo estica a outra dim.
        if let ImageSampler::Descriptor(d) = &image.sampler {
            assert_eq!(d.address_mode_v, ImageAddressMode::Repeat);
        }
    }

    /// World-tiled com mips próprios (hipotético KTX2 de chão): o sampler
    /// tiled aplica-se NA MESMA, e os dados ficam intactos.
    #[test]
    fn test_patch_world_tiled_resamples_even_with_mips() {
        let mut image = rgba_image(4, 4, [1, 2, 3, 4]);
        image.texture_descriptor.mip_level_count = 3;
        let before = image.data.clone();
        patch_image(&mut image, true);
        assert_eq!(image.data, before);
        assert_eq!(image.texture_descriptor.mip_level_count, 3);
        assert_eq!(sampler_of(&image), (ImageAddressMode::Repeat, 8));
    }

    /// Não-tiled mantém ClampToEdge (default de `linear()`): GLBs e afins
    /// não devem ganhar REPEAT só porque o chão precisa.
    #[test]
    fn test_patch_plain_keeps_clamp_address_mode() {
        let mut image = rgba_image(8, 4, [255, 0, 0, 255]);
        patch_image(&mut image, false);
        assert_eq!(sampler_of(&image), (ImageAddressMode::ClampToEdge, 8));
    }

    /// The per-channel implementation this filter replaced, kept verbatim as
    /// the reference: the `u32` trick must be a speed-up, not a re-encode.
    fn reference_chain(data: &[u8], width: u32, height: u32) -> Vec<u8> {
        let mut levels: Vec<Vec<u8>> = vec![data.to_vec()];
        let mut lw = width;
        let mut lh = height;
        while (lw > 1 || lh > 1) && levels.len() < 12 {
            let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
            let prev = levels.last().expect("levels is non-empty");
            let mut next = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let sx = (x * 2).min(lw - 1);
                        let sy = (y * 2).min(lh - 1);
                        let sx1 = (sx + 1).min(lw - 1);
                        let sy1 = (sy + 1).min(lh - 1);
                        let px = |xx: u32, yy: u32| ((yy * lw + xx) * 4 + c) as usize;
                        next[((y * nw + x) * 4 + c) as usize] = ((prev[px(sx, sy)] as u32
                            + prev[px(sx1, sy)] as u32
                            + prev[px(sx, sy1)] as u32
                            + prev[px(sx1, sy1)] as u32)
                            / 4)
                            as u8;
                    }
                }
            }
            levels.push(next);
            lw = nw;
            lh = nh;
        }
        levels.concat()
    }

    /// Odd dimensions (37×23) exercise the edge clamping that the averaged
    /// `u32` path has to reproduce exactly.
    #[test]
    fn build_mip_chain_matches_the_reference_filter() {
        let (width, height) = (37u32, 23u32);
        let data: Vec<u8> = (0..(width * height * 4) as usize)
            .map(|i| ((i * 37 + i / 7) % 256) as u8)
            .collect();
        let (chain, levels) = build_mip_chain(&data, width, height).expect("chain");
        assert_eq!(levels, 6, "37×23 → 18×11 → 9×5 → 4×2 → 2×1 → 1×1");
        assert_eq!(chain, reference_chain(&data, width, height));
    }

    #[test]
    fn build_mip_chain_stops_at_one_pixel_and_rejects_short_data() {
        let data = vec![7u8; 64 * 64 * 4];
        let (_, levels) = build_mip_chain(&data, 64, 64).expect("chain");
        assert_eq!(levels, 7, "64 → 1 is seven levels");
        let (chain, _) = build_mip_chain(&data, 64, 64).expect("chain");
        assert_eq!(chain.len(), 16384 + 4096 + 1024 + 256 + 64 + 16 + 4);
        // A 1×1 image has no chain, and truncated data never reads past itself.
        assert!(build_mip_chain(&[0; 4], 1, 1).is_none());
        assert!(build_mip_chain(&[0; 16], 8, 8).is_none());
    }
}
