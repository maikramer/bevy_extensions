//! GPU dispersive iWave + river-advected interactive water surface solver.
//!
//! Maintains a floating 128 m x 128 m rolling compute grid (`512 x 512`, 0.25 m
//! texels) centered around the active player/camera with integer 1.0 m window
//! snapping and 4.0 m hysteresis so walking shifts history by exact texels with
//! zero numerical smear. Outputs `(height_m, dh_dx, dh_dz, foam)` into an
//! `Rgba16Float` GPU texture (`wave_tex`) sampled directly by `water.wesl`, while
//! lazily rasterizing `(depth_m, flow_x, flow_z, plunge_boil)` into a `128 x 128`
//! `domain_tex` from the host's [`crate::source::WaterSource`].

use std::sync::OnceLock;

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferSize, BufferUsages,
    CachedComputePipelineId, ComputePassDescriptor, ComputePipelineDescriptor, Extent3d,
    FilterMode, MipmapFilterMode, PipelineCache, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, StorageTextureAccess, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages, TextureViewDimension, TextureViewId,
};
use bevy::render::renderer::{
    RenderContext, RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue,
};
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

use std::marker::PhantomData;

use crate::body::{CONTOUR_PEAK, WaterBody, WaterDomainSample, WaterKind};
use crate::environment::WaterEnvironment;
use crate::obstacles::{self, WaterObstacle};
use crate::source::{WaterFocus, WaterObstacles, WaterSource};

pub const WATER_WAVES_WGSL: &str = include_str!("water_waves.wgsl");

/// Simulation fine grid resolution (`512 x 512` at 0.25 m = 128 m x 128 m).
pub const WAVE_GRID_SIZE: u32 = 512;
pub const WAVE_TEXEL_M: f32 = 0.25;
pub const WAVE_WORLD_SIZE: f32 = (WAVE_GRID_SIZE as f32) * WAVE_TEXEL_M;

/// Hydrodynamic domain grid resolution (`256 x 256` at 0.5 m = 128 m x 128 m).
pub const DOMAIN_GRID_SIZE: u32 = 256;
pub const DOMAIN_SNAP_M: f32 = 1.0;
pub const DOMAIN_TEXEL_M: f32 = WAVE_WORLD_SIZE / DOMAIN_GRID_SIZE as f32;
const WINDOW_HYSTERESIS_M: f32 = 4.0;
const FINE_PER_DOMAIN: i32 = (DOMAIN_SNAP_M / WAVE_TEXEL_M) as i32; // 4

pub const MAX_IMPULSES: usize = 64;
const KERNEL_RADIUS: usize = 5;
const KERNEL_QUAD_SIDE: usize = KERNEL_RADIUS + 1; // 6 -> 36 symmetric weights -> 9 vec4s
const KERNEL_VEC4S: usize = 9;
const IMPULSE_VEC4S: usize = 2 * MAX_IMPULSES; // 128 vec4s
const TOTAL_UNIFORM_VEC4S: usize = 4 + KERNEL_VEC4S + IMPULSE_VEC4S; // 141 vec4s = 2256 bytes

static WAVE_COMPUTE_SHADER: OnceLock<Handle<Shader>> = OnceLock::new();

/// Velocity and body footprint published for interactive surface wakes/craters.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct WaterKinematics {
    /// World-space linear velocity `(vx, vy, vz)` in m/s.
    pub velocity: Vec3,
    /// Horizontal interaction radius of the body in meters.
    pub body_radius: f32,
    /// Local Y offset from the entity origin to the surface that touches the
    /// water (feet for characters, keel for boats). The splash/wake contact
    /// measures submersion against `origin + offset`, so a float whose origin
    /// sits at the waterline (0) still reports the hull depth below it.
    pub contact_y_offset: f32,
}

impl WaterKinematics {
    /// Kinematics for a body whose contact point is at `contact_y_offset`
    /// (0 = the entity origin itself, like a character's feet).
    pub fn new(velocity: Vec3, body_radius: f32) -> Self {
        Self {
            velocity,
            body_radius,
            contact_y_offset: 0.0,
        }
    }
}

/// Interactive impulse pushed during gameplay for the GPU water wave solver.
#[derive(Debug, Clone, Copy)]
pub struct WaveImpulseRequest {
    pub pos: Vec2,
    pub vel: Vec2,
    pub radius: f32,
    pub strength: f32,
    pub foam: f32,
    pub is_wake: bool,
}

/// Main-world interactive water wave simulation state and textures.
#[derive(Resource)]
pub struct WaterWaveWorld {
    pub wave_tex: Handle<Image>,
    pub domain_tex: Handle<Image>,
    pub obstacle_tex: Handle<Image>,
    /// Current bottom-left world XZ corner in integer meters (`1.0 m` aligned).
    pub origin_cell_1m: IVec2,
    /// Fine texel shift accumulated this frame (`+/- 4 * delta_1m`).
    pub pending_shift_fine: IVec2,
    /// Impulses queued during this frame before extraction.
    pub impulses: Vec<WaveImpulseRequest>,
    /// True whenever any lake/river overlaps the active 128 m x 128 m domain.
    pub active_water: bool,
    /// Cached waterline obstacles overlapping the rolling wave window.
    pub active_obstacles: Vec<WaterObstacle>,
    domain_populated: bool,
    last_domain_origin: IVec2,
    last_terrain_sig: u64,
    last_obstacle_sig: u64,
    /// Geração dos obstáculos publicada pelo host (`WaterObstacles`).
    last_obstacle_gen: u64,
}

impl WaterWaveWorld {
    /// Returns world-space `[origin_x, origin_z, world_size, texel_m]` for shaders.
    pub fn window_vec4(&self) -> Vec4 {
        Vec4::new(
            self.origin_cell_1m.x as f32 * DOMAIN_SNAP_M,
            self.origin_cell_1m.y as f32 * DOMAIN_SNAP_M,
            WAVE_WORLD_SIZE,
            WAVE_TEXEL_M,
        )
    }

    /// Evaluates water current at `pos_xz` after obstacle deflection & wake shelter.
    pub fn effective_flow_at(&self, pos_xz: Vec2, base_flow: Vec2) -> Vec2 {
        obstacles::deflect_flow_by_obstacles(
            pos_xz,
            base_flow,
            &self.active_obstacles,
        )
    }

    /// Queues a splashdown/entry crater or continuous Kelvin wake into this frame.
    pub fn push_impulse(&mut self, req: WaveImpulseRequest) {
        if self.impulses.len() < MAX_IMPULSES {
            self.impulses.push(req);
        }
    }

    /// Updates rolling window origin with hysteresis, returning true if shifted.
    pub fn update_window_focus(&mut self, focus_xz: Vec2) -> bool {
        let half = WAVE_WORLD_SIZE * 0.5;
        if !self.domain_populated {
            let target = IVec2::new(
                (focus_xz.x - half).round() as i32,
                (focus_xz.y - half).round() as i32,
            );
            self.origin_cell_1m = target;
            self.pending_shift_fine = IVec2::splat(WAVE_GRID_SIZE as i32 * 2);
            return true;
        }
        let center_x = self.origin_cell_1m.x as f32 * DOMAIN_SNAP_M + half;
        let center_z = self.origin_cell_1m.y as f32 * DOMAIN_SNAP_M + half;
        if (focus_xz.x - center_x).abs() <= WINDOW_HYSTERESIS_M
            && (focus_xz.y - center_z).abs() <= WINDOW_HYSTERESIS_M
        {
            return false;
        }
        let next_origin = IVec2::new(
            (focus_xz.x - half).round() as i32,
            (focus_xz.y - half).round() as i32,
        );
        let delta = next_origin - self.origin_cell_1m;
        if delta == IVec2::ZERO {
            return false;
        }
        self.origin_cell_1m = next_origin;
        self.pending_shift_fine += delta * FINE_PER_DOMAIN;
        true
    }
}

/// Zero-initialized `Rgba16Float` image with clamped bilinear filtering.
pub fn make_wave_rgba16f_image(width: u32, height: u32, storage: bool) -> Image {
    let bytes = vec![0u8; (width as usize) * (height as usize) * 8];
    let mut img = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    let mut sampler_desc = ImageSamplerDescriptor::linear();
    sampler_desc.set_address_mode(ImageAddressMode::ClampToEdge);
    img.sampler = ImageSampler::Descriptor(sampler_desc);
    let mut usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST;
    if storage {
        usage |= TextureUsages::STORAGE_BINDING;
    }
    img.texture_descriptor.usage = usage;
    img
}

/// Evaluates Bessel J0(x) to machine precision via 48-point periodic quadrature.
fn bessel_j0(x: f64) -> f64 {
    const N: usize = 48;
    let step = std::f64::consts::PI / (N as f64);
    let mut sum = 0.0_f64;
    for k in 0..N {
        let theta = (k as f64 + 0.5) * step;
        sum += (x * theta.sin()).cos();
    }
    sum / (N as f64)
}

/// Computes the 11x11 symmetric dispersive Hankel convolution quadrant `G(|dx|, |dz|)`
/// packed into 9 `vec4<f32>` entries (`idx = az * 6 + ax`) alongside calibrated restoring `g_eff`.
///
/// Satisfies exact DC zero-sum `Σ G = 0` and non-negative Brillouin spectrum `Ĝ(k) >= 0`.
pub fn compute_iwave_kernel() -> ([[f32; 4]; KERNEL_VEC4S], f32) {
    // Sigma = 0.62 matches 0.25 m texels cleanly within radius P=5 while preserving
    // clean capillary-gravity dispersion across lambda in [0.8 m, 16 m].
    const SIGMA: f64 = 0.62;
    const Q_STEPS: usize = 128;
    const Q_MAX: f64 = 5.0;
    let dq = Q_MAX / (Q_STEPS as f64);

    let mut raw = [[0.0_f64; KERNEL_QUAD_SIDE]; KERNEL_QUAD_SIDE];
    for az in 0..KERNEL_QUAD_SIDE {
        for ax in 0..KERNEL_QUAD_SIDE {
            let r = ((ax * ax + az * az) as f64).sqrt();
            let mut acc = 0.0_f64;
            for m in 0..Q_STEPS {
                let q = (m as f64 + 0.5) * dq;
                let weight = q * q * (-SIGMA * q * q).exp();
                acc += weight * bessel_j0(q * r) * dq;
            }
            raw[az][ax] = acc;
        }
    }

    let g0 = raw[0][0].max(1e-12);
    for row in &mut raw {
        for v in row {
            *v /= g0;
        }
    }

    // Balance outer negative annulus weights so the truncated [-5..5]^2 stencil
    // sums strictly to zero (preserving exact flat-surface equilibrium) while G(0,0) = 1.0.
    let mut pos_sum = 0.0_f64;
    let mut neg_sum = 0.0_f64;
    for dz in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
        for dx in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
            let v = raw[dz.unsigned_abs()][dx.unsigned_abs()];
            if v >= 0.0 {
                pos_sum += v;
            } else {
                neg_sum -= v;
            }
        }
    }
    if neg_sum > 1e-12 {
        let scale = pos_sum / neg_sum;
        for row in &mut raw {
            for v in row {
                if *v < 0.0 {
                    *v *= scale;
                }
            }
        }
    }

    // Exact residual removal via uniform adjustment on r=0 center if floating rounding remains.
    let mut total = 0.0_f64;
    for dz in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
        for dx in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
            total += raw[dz.unsigned_abs()][dx.unsigned_abs()];
        }
    }
    raw[0][0] -= total;

    // Pack into 9 vec4<f32>.
    let mut packed = [[0.0_f32; 4]; KERNEL_VEC4S];
    for az in 0..KERNEL_QUAD_SIDE {
        for ax in 0..KERNEL_QUAD_SIDE {
            let idx = az * KERNEL_QUAD_SIDE + ax;
            packed[idx >> 2][idx & 3] = raw[az][ax] as f32;
        }
    }

    // Calibrate g_eff so physical wavelength ~2 m travels smoothly around ~1.8 m/s
    // without violating the explicit Verlet stability margin.
    let g_eff = 28.0_f32;
    (packed, g_eff)
}

#[inline]
pub fn bodies_signature(bodies: &[WaterBody]) -> u64 {
    let mut h = (bodies.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for b in bodies {
        h ^= (b.stations.len() as u64).wrapping_shl(16)
            ^ (b.water_y.to_bits() as u64)
            ^ ((b.at.x.to_bits() as u64).rotate_left(7));
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    }
    h
}

/// Lazily rasterizes `(depth_m, flow_x, flow_z, plunge_boil)` into `domain_tex`
/// whenever the active 128 m window shifts or terrain water changes.
#[inline]
fn write_rgba16f_texel(bytes: &mut [u8], texel_idx: usize, vals: [f32; 4]) {
    let base = texel_idx * 8;
    for (c, &v) in vals.iter().enumerate() {
        let bits = half::f16::from_f32(v).to_bits().to_le_bytes();
        bytes[base + c * 2] = bits[0];
        bytes[base + c * 2 + 1] = bits[1];
    }
}

pub fn rasterize_domain_into_bytes<S: WaterSource>(
    source: &S,
    origin_cell_1m: IVec2,
    out_bytes: &mut [u8],
) -> bool {
    rasterize_domain_with_obstacles(source, origin_cell_1m, &[], out_bytes, None)
}

/// Rasterizes hydrodynamic depth/deflected-flow into `out_domain_bytes` and,
/// when `out_obstacle_bytes` is provided, writes `(halo, obs_nx, obs_nz, radius)`
/// at `DOMAIN_GRID_SIZE x DOMAIN_GRID_SIZE` (`0.5 m/texel`).
pub fn rasterize_domain_with_obstacles<S: WaterSource>(
    source: &S,
    origin_cell_1m: IVec2,
    obstacles: &[obstacles::WaterObstacle],
    out_domain_bytes: &mut [u8],
    mut out_obstacle_bytes: Option<&mut [u8]>,
) -> bool {
    let size = DOMAIN_GRID_SIZE as usize;
    let ox = origin_cell_1m.x as f32 * DOMAIN_SNAP_M;
    let oz = origin_cell_1m.y as f32 * DOMAIN_SNAP_M;
    let wx_max = ox + WAVE_WORLD_SIZE;
    let wz_max = oz + WAVE_WORLD_SIZE;

    // Filter water bodies overlapping the active 128 m x 128 m window.
    let mut active_indices: Vec<usize> = Vec::new();
    for (idx, body) in source.bodies().iter().enumerate() {
        let reach = match body.kind {
            WaterKind::Lake => body.radius * CONTOUR_PEAK * body.mirror_reach.max(1.0) + 1.5,
            WaterKind::River => {
                if body.hull > 0.0 {
                    body.hull + body.carve_radius + 2.0
                } else {
                    1.0e5
                }
            }
        };
        if body.at.x + reach >= ox
            && body.at.x - reach <= wx_max
            && body.at.y + reach >= oz
            && body.at.y - reach <= wz_max
        {
            active_indices.push(idx);
        }
    }

    if active_indices.is_empty() {
        out_domain_bytes.fill(0);
        if let Some(obs_bytes) = out_obstacle_bytes.as_deref_mut() {
            obs_bytes.fill(0);
        }
        return false;
    }

    let mut any_wet = false;
    for iz in 0..size {
        let wz = oz + (iz as f32 + 0.5) * DOMAIN_TEXEL_M;
        for ix in 0..size {
            let wx = ox + (ix as f32 + 0.5) * DOMAIN_TEXEL_M;
            let p = Vec2::new(wx, wz);
            let bed_y = source.bed_y(wx, wz);
            let mut best: Option<WaterDomainSample> = None;
            for &bi in &active_indices {
                if let Some(sample) = source.bodies()[bi].sample_domain(p, bed_y, 1.5) {
                    let take = match best {
                        None => true,
                        Some(prev) => sample.depth > prev.depth,
                    };
                    if take {
                        best = Some(sample);
                    }
                }
            }
            let tidx = iz * size + ix;
            let base = tidx * 8;
            if let Some(s) = best {
                any_wet = true;
                let flow = obstacles::deflect_flow_by_obstacles(p, s.flow, obstacles);
                write_rgba16f_texel(out_domain_bytes, tidx, [s.depth, flow.x, flow.y, s.plunge]);
            } else {
                out_domain_bytes[base..base + 8].fill(0);
            }

            if let Some(obs_bytes) = out_obstacle_bytes.as_deref_mut() {
                if let Some(obs) = obstacles::nearest_obstacle_at(p, obstacles) {
                    let halo = if obs.signed_dist <= 0.0 {
                        1.0 + (-obs.signed_dist / obs.radius.max(0.5)).min(1.0)
                    } else {
                        ((obstacles::OBSTACLE_HALO_M - obs.signed_dist)
                            / obstacles::OBSTACLE_HALO_M)
                            .clamp(0.0, 1.0)
                    };
                    write_rgba16f_texel(
                        obs_bytes,
                        tidx,
                        [halo, obs.normal.x, obs.normal.y, obs.radius],
                    );
                } else {
                    obs_bytes[base..base + 8].fill(0);
                }
            }
        }
    }
    any_wet
}

fn begin_wave_frame(mut waves: ResMut<WaterWaveWorld>) {
    waves.impulses.clear();
    waves.pending_shift_fine = IVec2::ZERO;
}

/// Foco da janela rolante: o `WaterFocus` do host (posição do herói), ou a
/// câmara que desenha para a janela.
fn window_focus(
    focus: Option<&WaterFocus>,
    cam_q: &Query<
        (
            &Transform,
            Option<&Camera>,
            Option<&bevy::camera::RenderTarget>,
        ),
        With<Camera3d>,
    >,
) -> Option<Vec2> {
    if let Some(p) = focus.and_then(|f| f.0) {
        return Some(p);
    }
    cam_q
        .iter()
        .find(|(_, cam, target)| match (cam, target) {
            (Some(cam), Some(bevy::camera::RenderTarget::Window(_))) => cam.is_active,
            (Some(_), None) => true,
            _ => false,
        })
        .map(|(t, _, _)| Vec2::new(t.translation.x, t.translation.z))
}

/// Re-rasteriza `domain_tex`/`obstacle_tex` quando a janela anda, os corpos de
/// água mudam (`S::signature`) ou o host publica obstáculos novos
/// (`WaterObstacles::generation`). A coleta de obstáculos (Rapier/avian/voxel)
/// é do HOST: aqui só entram os [`WaterObstacle`]s já cortados na lâmina.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_wave_window_and_domain<S: WaterSource>(
    mut waves: ResMut<WaterWaveWorld>,
    focus: Option<Res<WaterFocus>>,
    cam_q: Query<
        (
            &Transform,
            Option<&Camera>,
            Option<&bevy::camera::RenderTarget>,
        ),
        With<Camera3d>,
    >,
    source: Option<Res<S>>,
    obstacles_res: Option<Res<WaterObstacles>>,
    mut images: ResMut<Assets<Image>>,
) {
    if let Some(pos) = window_focus(focus.as_deref(), &cam_q) {
        waves.update_window_focus(pos);
    }

    let Some(source) = source else {
        waves.active_water = false;
        return;
    };
    let obs_gen = obstacles_res.as_ref().map_or(0, |o| o.generation);
    let sig = source.signature();
    let base_dirty = !waves.domain_populated
        || waves.origin_cell_1m != waves.last_domain_origin
        || sig != waves.last_terrain_sig
        || obs_gen != waves.last_obstacle_gen;
    if !base_dirty {
        return;
    }

    let origin = waves.origin_cell_1m;
    let obstacles = obstacles_res
        .as_ref()
        .map(|o| o.obstacles.clone())
        .unwrap_or_default();
    let obs_sig = obstacles::obstacles_signature(&obstacles);
    let need_raster = !waves.domain_populated
        || origin != waves.last_domain_origin
        || sig != waves.last_terrain_sig
        || obs_sig != waves.last_obstacle_sig;
    if !need_raster {
        return;
    }

    let need_len = (DOMAIN_GRID_SIZE as usize) * (DOMAIN_GRID_SIZE as usize) * 8;
    let mut obs_bytes = vec![0u8; need_len];
    let domain_handle = waves.domain_tex.clone();
    if let Some(mut img) = images.get_mut(&domain_handle) {
        let data = img.data.get_or_insert_with(|| vec![0u8; need_len]);
        if data.len() != need_len {
            data.resize(need_len, 0);
        }
        waves.active_water = rasterize_domain_with_obstacles(
            &*source,
            origin,
            &obstacles,
            data,
            Some(&mut obs_bytes),
        );
    }
    let obstacle_handle = waves.obstacle_tex.clone();
    if let Some(mut img) = images.get_mut(&obstacle_handle) {
        let data = img.data.get_or_insert_with(|| vec![0u8; need_len]);
        if data.len() != need_len {
            data.resize(need_len, 0);
        }
        data.copy_from_slice(&obs_bytes);
    }

    waves.active_obstacles = obstacles;
    waves.domain_populated = true;
    waves.last_domain_origin = origin;
    waves.last_terrain_sig = sig;
    waves.last_obstacle_sig = obs_sig;
    waves.last_obstacle_gen = obs_gen;
}

// ── Render World Compute Pipeline ───────────────────────────────────────────

#[derive(Resource, Default)]
struct ExtractedWaveFrame {
    wave_tex: Handle<Image>,
    domain_tex: Handle<Image>,
    obstacle_tex: Handle<Image>,
    window: Vec4,
    shift_fine: IVec2,
    dt: f32,
    time: f32,
    rain: f32,
    wind: Vec2,
    active: bool,
    impulses: Vec<WaveImpulseRequest>,
}

fn extract_wave_frame(
    mut out: ResMut<ExtractedWaveFrame>,
    waves: Extract<Option<Res<WaterWaveWorld>>>,
    time: Extract<Option<Res<Time>>>,
    env: Extract<Option<Res<WaterEnvironment>>>,
) {
    let Some(waves) = waves.as_ref() else {
        out.active = false;
        return;
    };
    out.wave_tex = waves.wave_tex.clone();
    out.domain_tex = waves.domain_tex.clone();
    out.obstacle_tex = waves.obstacle_tex.clone();
    out.window = waves.window_vec4();
    out.shift_fine = waves.pending_shift_fine;
    out.active = waves.active_water;
    out.impulses.clear();
    out.impulses.extend_from_slice(&waves.impulses);
    if let Some(t) = time.as_ref() {
        out.dt = t.delta_secs();
        out.time = t.elapsed_secs();
    } else {
        out.dt = 1.0 / 60.0;
    }
    if let Some(env) = env.as_ref() {
        out.rain = env.rain.clamp(0.0, 1.0);
        out.wind = Vec2::from(env.wind);
    } else {
        out.rain = 0.0;
        out.wind = Vec2::ZERO;
    }
}

#[derive(Resource)]
struct WaterWavePipeline {
    layout: BindGroupLayoutDescriptor,
    step_pipeline: CachedComputePipelineId,
    export_pipeline: CachedComputePipelineId,
    uniform_buf: Buffer,
    state_a: Buffer,
    state_b: Buffer,
    domain_sampler: Sampler,
    bind_a_to_b: Option<BindGroup>,
    bind_b_to_a: Option<BindGroup>,
    cached_views: Option<(TextureViewId, TextureViewId, TextureViewId)>,
    parity: usize,
    kernel_packed: [[f32; 4]; KERNEL_VEC4S],
    g_eff: f32,
    uniform_scratch: Vec<u8>,
}

fn init_wave_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
) {
    let Some(shader) = WAVE_COMPUTE_SHADER.get().cloned() else {
        return;
    };
    let entries = vec![
        BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: BufferSize::new((TOTAL_UNIFORM_VEC4S * 16) as u64),
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 1,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 2,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 3,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 4,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 5,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::StorageTexture {
                access: StorageTextureAccess::WriteOnly,
                format: TextureFormat::Rgba16Float,
                view_dimension: TextureViewDimension::D2,
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 6,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
    ];
    let layout = BindGroupLayoutDescriptor::new("water_waves_layout", &entries);

    let step_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("water_waves_step".into()),
        layout: vec![layout.clone()],
        shader: shader.clone(),
        entry_point: Some("step_sim".into()),
        zero_initialize_workgroup_memory: true,
        ..Default::default()
    });
    let export_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("water_waves_export".into()),
        layout: vec![layout.clone()],
        shader,
        entry_point: Some("gradient_export".into()),
        zero_initialize_workgroup_memory: true,
        ..Default::default()
    });

    let uniform_bytes = (TOTAL_UNIFORM_VEC4S * 16) as u64;
    let state_bytes = ((WAVE_GRID_SIZE * WAVE_GRID_SIZE) as u64) * 16;
    let uniform_buf = render_device.create_buffer(&BufferDescriptor {
        label: Some("water_waves_uniforms"),
        size: uniform_bytes,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let state_a = render_device.create_buffer(&BufferDescriptor {
        label: Some("water_waves_state_a"),
        size: state_bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let state_b = render_device.create_buffer(&BufferDescriptor {
        label: Some("water_waves_state_b"),
        size: state_bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let domain_sampler = render_device.create_sampler(&SamplerDescriptor {
        label: Some("water_domain_sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..Default::default()
    });

    let (kernel_packed, g_eff) = compute_iwave_kernel();
    commands.insert_resource(WaterWavePipeline {
        layout,
        step_pipeline,
        export_pipeline,
        uniform_buf,
        state_a,
        state_b,
        domain_sampler,
        bind_a_to_b: None,
        bind_b_to_a: None,
        cached_views: None,
        parity: 0,
        kernel_packed,
        g_eff,
        uniform_scratch: vec![0u8; uniform_bytes as usize],
    });
}

fn pack_sim_uniforms(
    frame: &ExtractedWaveFrame,
    kernel_packed: &[[f32; 4]; KERNEL_VEC4S],
    g_eff: f32,
    out: &mut [u8],
) {
    let mut floats = [0.0_f32; TOTAL_UNIFORM_VEC4S * 4];
    // 0: origin_size
    floats[0] = frame.window.x;
    floats[1] = frame.window.y;
    floats[2] = frame.window.z;
    floats[3] = frame.window.w;
    // 1: shift_dt_time
    floats[4] = frame.shift_fine.x as f32;
    floats[5] = frame.shift_fine.y as f32;
    floats[6] = frame.dt.clamp(0.001, 0.025);
    floats[7] = frame.time;
    // 2: env_count
    let count = frame.impulses.len().min(MAX_IMPULSES);
    floats[8] = frame.rain;
    floats[9] = frame.wind.x;
    floats[10] = frame.wind.y;
    floats[11] = count as f32;
    // 3: physics
    floats[12] = g_eff;
    // 4..13: kernel (9 vec4s)
    for (k, v) in kernel_packed.iter().enumerate() {
        let base = 16 + k * 4;
        floats[base..base + 4].copy_from_slice(v);
    }
    // 13..141: impulses (64 * 2 vec4s)
    for (i, imp) in frame.impulses.iter().take(MAX_IMPULSES).enumerate() {
        let base = (4 + KERNEL_VEC4S) * 4 + i * 8;
        floats[base] = imp.pos.x;
        floats[base + 1] = imp.pos.y;
        floats[base + 2] = imp.vel.x;
        floats[base + 3] = imp.vel.y;
        floats[base + 4] = imp.radius;
        floats[base + 5] = imp.strength;
        floats[base + 6] = imp.foam;
        floats[base + 7] = if imp.is_wake { 1.0 } else { 0.0 };
    }
    for (i, &f) in floats.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&f.to_le_bytes());
    }
}

fn prepare_wave_bind_groups(
    gpu: Option<ResMut<WaterWavePipeline>>,
    frame: Res<ExtractedWaveFrame>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let Some(gpu) = gpu else {
        return;
    };
    if !frame.active {
        return;
    }
    let Some(wave_gpu) = gpu_images.get(&frame.wave_tex) else {
        return;
    };
    let Some(domain_gpu) = gpu_images.get(&frame.domain_tex) else {
        return;
    };
    let Some(obstacle_gpu) = gpu_images.get(&frame.obstacle_tex) else {
        return;
    };

    let p = gpu.into_inner();
    let views = (
        wave_gpu.texture_view.id(),
        domain_gpu.texture_view.id(),
        obstacle_gpu.texture_view.id(),
    );
    if p.cached_views != Some(views) {
        let bgl = pipeline_cache.get_bind_group_layout(&p.layout);
        let make_bg = |in_buf: &Buffer, out_buf: &Buffer, label: &'static str| {
            render_device.create_bind_group(
                label,
                &bgl,
                &BindGroupEntries::sequential((
                    p.uniform_buf.as_entire_binding(),
                    in_buf.as_entire_binding(),
                    out_buf.as_entire_binding(),
                    &domain_gpu.texture_view,
                    &p.domain_sampler,
                    BindingResource::TextureView(&wave_gpu.texture_view),
                    &obstacle_gpu.texture_view,
                )),
            )
        };
        let bg_a = make_bg(&p.state_a, &p.state_b, "water_waves_a_to_b");
        let bg_b = make_bg(&p.state_b, &p.state_a, "water_waves_b_to_a");
        p.bind_a_to_b = Some(bg_a);
        p.bind_b_to_a = Some(bg_b);
        p.cached_views = Some(views);
    }

    pack_sim_uniforms(&frame, &p.kernel_packed, p.g_eff, &mut p.uniform_scratch);
    queue.write_buffer(&p.uniform_buf, 0, &p.uniform_scratch);
}

fn dispatch_water_waves(
    gpu: Option<ResMut<WaterWavePipeline>>,
    frame: Res<ExtractedWaveFrame>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let Some(gpu) = gpu else {
        return;
    };
    let p = gpu.into_inner();
    if !frame.active {
        return;
    }
    let Some(step_pipe) = pipeline_cache.get_compute_pipeline(p.step_pipeline) else {
        return;
    };
    let Some(export_pipe) = pipeline_cache.get_compute_pipeline(p.export_pipeline) else {
        return;
    };
    let bg = if p.parity == 0 {
        p.bind_a_to_b.as_ref()
    } else {
        p.bind_b_to_a.as_ref()
    };
    let Some(bg) = bg else {
        return;
    };

    let groups = WAVE_GRID_SIZE / 8;
    {
        let mut pass = ctx.command_encoder().begin_compute_pass(&ComputePassDescriptor {
            label: Some("water_waves_step"),
            timestamp_writes: None,
        });
        pass.set_pipeline(step_pipe);
        pass.set_bind_group(0, bg, &[]);
        pass.dispatch_workgroups(groups, groups, 1);
    }
    {
        let mut pass = ctx.command_encoder().begin_compute_pass(&ComputePassDescriptor {
            label: Some("water_waves_export"),
            timestamp_writes: None,
        });
        pass.set_pipeline(export_pipe);
        pass.set_bind_group(0, bg, &[]);
        pass.dispatch_workgroups(groups, groups, 1);
    }

    p.parity ^= 1;
}

/// Registers the GPU dispersive iWave + river advection solver and world
/// textures. `S` is the host's [`WaterSource`] resource (terrain runtime in
/// the Vael engine, a plain body list in other games).
pub struct WaterWavePlugin<S: WaterSource>(pub PhantomData<S>);

impl<S: WaterSource> Default for WaterWavePlugin<S> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<S: WaterSource> Plugin for WaterWavePlugin<S> {
    fn build(&self, app: &mut App) {
        let wave_tex;
        let domain_tex;
        let obstacle_tex;
        if let Some(mut images) = app.world_mut().get_resource_mut::<Assets<Image>>() {
            wave_tex = images.add(make_wave_rgba16f_image(WAVE_GRID_SIZE, WAVE_GRID_SIZE, true));
            domain_tex =
                images.add(make_wave_rgba16f_image(DOMAIN_GRID_SIZE, DOMAIN_GRID_SIZE, false));
            obstacle_tex =
                images.add(make_wave_rgba16f_image(DOMAIN_GRID_SIZE, DOMAIN_GRID_SIZE, false));
        } else {
            wave_tex = Handle::default();
            domain_tex = Handle::default();
            obstacle_tex = Handle::default();
        }
        app.insert_resource(WaterWaveWorld {
            wave_tex,
            domain_tex,
            obstacle_tex,
            origin_cell_1m: IVec2::ZERO,
            pending_shift_fine: IVec2::ZERO,
            impulses: Vec::new(),
            active_water: false,
            active_obstacles: Vec::new(),
            domain_populated: false,
            last_domain_origin: IVec2::ZERO,
            last_terrain_sig: 0,
            last_obstacle_sig: 0,
            last_obstacle_gen: 0,
        });

        app.add_systems(First, begin_wave_frame);
        app.add_systems(Update, crate::material::sync_water_materials);
        app.add_systems(
            PostUpdate,
            update_wave_window_and_domain::<S>.after(bevy::transform::TransformSystems::Propagate),
        );

        if let Some(mut shaders) = app.world_mut().get_resource_mut::<Assets<Shader>>() {
            let h = shaders.add(Shader::from_wgsl(WATER_WAVES_WGSL, "viber_water_waves.wgsl"));
            let _ = WAVE_COMPUTE_SHADER.set(h);
        }

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<ExtractedWaveFrame>()
            .add_systems(RenderStartup, init_wave_pipeline)
            .add_systems(ExtractSchedule, extract_wave_frame)
            .add_systems(Render, prepare_wave_bind_groups.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(RenderGraph, dispatch_water_waves.in_set(RenderGraphSystems::Begin));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iwave_kernel_is_zero_sum_symmetric_and_spectrally_non_negative() {
        let (packed, g_eff) = compute_iwave_kernel();
        assert!(g_eff > 10.0);

        let weight_at = |dx: isize, dz: isize| -> f64 {
            let idx = dz.unsigned_abs() * KERNEL_QUAD_SIDE + dx.unsigned_abs();
            packed[idx >> 2][idx & 3] as f64
        };

        let mut total_sum = 0.0_f64;
        for dz in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
            for dx in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
                total_sum += weight_at(dx, dz);
            }
        }
        assert!(
            total_sum.abs() < 1e-4,
            "kernel sum must vanish so flat water never drifts, got {total_sum}"
        );
        assert!(
            (weight_at(0, 0) - 1.0).abs() < 0.05,
            "center weight should stay near 1.0"
        );

        // Verify 2D discrete Fourier transform across the Brillouin zone [0, pi]^2 is non-negative.
        let mut min_spec = f64::INFINITY;
        let mut spec_low = 0.0;
        let mut spec_mid = 0.0;
        for kz_i in 0..=32 {
            let kz = (kz_i as f64 / 32.0) * std::f64::consts::PI;
            for kx_i in 0..=32 {
                let kx = (kx_i as f64 / 32.0) * std::f64::consts::PI;
                let mut re = 0.0_f64;
                for dz in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
                    for dx in -(KERNEL_RADIUS as isize)..=(KERNEL_RADIUS as isize) {
                        re += weight_at(dx, dz)
                            * (kx * (dx as f64) + kz * (dz as f64)).cos();
                    }
                }
                min_spec = min_spec.min(re);
                if kx_i == 4 && kz_i == 0 {
                    spec_low = re;
                }
                if kx_i == 10 && kz_i == 0 {
                    spec_mid = re;
                }
            }
        }
        assert!(
            min_spec >= -1e-4,
            "negative Brillouin eigenvalue {min_spec} would destabilize iWave solver"
        );
        assert!(
            spec_mid > spec_low && spec_low > 0.0,
            "dispersive response must grow with wavenumber: low={spec_low}, mid={spec_mid}"
        );
    }

    #[test]
    fn rolling_window_snaps_with_hysteresis_and_exact_fine_texel_shifts() {
        let mut w = WaterWaveWorld {
            wave_tex: Handle::default(),
            domain_tex: Handle::default(),
            obstacle_tex: Handle::default(),
            origin_cell_1m: IVec2::ZERO,
            pending_shift_fine: IVec2::ZERO,
            impulses: Vec::new(),
            active_water: false,
            active_obstacles: Vec::new(),
            domain_populated: false,
            last_domain_origin: IVec2::ZERO,
            last_terrain_sig: 0,
            last_obstacle_sig: 0,
            last_obstacle_gen: 0,
        };
        assert!(w.update_window_focus(Vec2::new(64.0, 64.0)));
        w.domain_populated = true;
        w.pending_shift_fine = IVec2::ZERO;
        let init_origin = w.origin_cell_1m;

        // Moving inside hysteresis band keeps window completely locked.
        assert!(!w.update_window_focus(Vec2::new(66.5, 62.0)));
        assert_eq!(w.origin_cell_1m, init_origin);
        assert_eq!(w.pending_shift_fine, IVec2::ZERO);

        // Moving outside hysteresis band steps in exact integer meters (4 fine texels / m).
        assert!(w.update_window_focus(Vec2::new(70.0, 64.0)));
        let delta = w.origin_cell_1m - init_origin;
        assert_eq!(w.pending_shift_fine, delta * FINE_PER_DOMAIN);
    }

    #[test]
    fn water_waves_wgsl_parses_cleanly() {
        let parsed = naga::front::wgsl::parse_str(WATER_WAVES_WGSL);
        assert!(parsed.is_ok(), "water_waves.wgsl parse error: {:?}", parsed.err());
    }
}
