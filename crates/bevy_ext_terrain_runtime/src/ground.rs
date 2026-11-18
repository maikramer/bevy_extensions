//! Chão que responde ao clima, sem o terreno do `bevy_elements`.
//!
//! O manto é uma grelha CPU à volta do foco (256² células de 2 m). O jogo
//! escreve [`SnowClimate`], [`SnowFocus`] e (opcional) [`SnowShelter`]; este
//! módulo integra neve, derrete, larga encostas, empurra um fluxo simples de
//! vento e não deixa nevar debaixo dos telhados. A **profundidade** (metros,
//! codificada em R8 com raiz quadrada) sobe para o material único de chunk:
//! o vertex levanta o chão por ela e o fragment pinta a neve. Fora da janela
//! o chão leva a profundidade de referência (o pack de um plano aberto ao
//! clima do foco). Rastos e sombra de nuvem não são simulados
//! aqui: o jogo copia os mapas do `bevy_elements` para [`GroundTrailField`] e
//! [`GroundCloudField`], e [`publish_ground`] mete-os no mesmo material.
//!
//! Com o flag a 0 o fragmento não lê o mapa — o chão fica o de hoje.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::layer_material::TerrainChunkMaterial;
use super::mesh::HeightField;
use super::runtime::{TerrainChunkMaterials, TerrainRuntime};

/// Células de lado da janela de neve. O shader usa o mesmo número.
pub const SNOW_CELLS: usize = 256;
/// Lado de uma célula, metros.
pub const SNOW_CELL_M: f32 = 2.0;
/// Profundidade que o byte 255 representa, metros. O byte guarda
/// `sqrt(d / max)`: precisão de décimos de mm junto a zero.
pub const DEPTH_ENCODE_MAX_M: f32 = 1.5;
/// Lado das telhas do mapa de abrigo, células.
pub const SHELTER_TILE: usize = 64;
/// Albedo linear da neve, o mesmo do `bevy_flora` / `bevy_elements`.
pub const SNOW_ALBEDO: [f32; 3] = [0.80, 0.83, 0.88];
/// Rugosidade da neve.
pub const SNOW_ROUGHNESS: f32 = 0.9;
/// Profundidade geométrica que lê como cobertura total, metros.
pub const FULL_COVER_DEPTH_M: f32 = 0.05;
/// Queda de temperatura com a altitude, °C por metro (6,5 °C/km).
pub const LAPSE_C_PER_M: f32 = 0.0065;
/// Abaixo disto a encosta guarda a neve toda.
pub const SHED_LO_DEG: f32 = 35.0;
/// A partir disto a encosta não guarda neve.
pub const SHED_HI_DEG: f32 = 55.0;
/// 1 mm de água vira 10 mm de neve fresca (densidade 100 kg/m³).
pub const DEPTH_PER_WATER_MM: f32 = 0.01;
/// Derretimento por grau-dia, metros de neve por °C por dia de jogo.
pub const MELT_M_PER_C_PER_DAY: f32 = 0.04;
/// Metros de neve que 1 mm de chuva derrete.
pub const RAIN_MELT_M_PER_MM: f32 = 0.01;
/// Vento (m/s) a partir do qual a neve desliza para a célula vizinha.
const DRIFT_WIND_MS: f32 = 5.0;
/// A queda física (1,5 mm/h) só cobre o chão ao fim de minutos reais com o
/// relógio a 72×. O passo ao vivo multiplica a queda por isto (por omissão;
/// [`SnowClimate::fall_gain`] manda) para a neve ler-se em dezenas de
/// segundos. O derretimento não leva o ganho.
pub const VISUAL_FALL_GAIN: f32 = 8.0;
/// Passos do manto por segundo real, no máximo. Cada passo integra o tempo de
/// jogo acumulado desde o anterior.
const STEP_HZ: f32 = 12.0;
/// O pack numa encosta vertical some nesta janela de tempo de jogo, segundos.
const SHED_SECONDS: f32 = 600.0;

/// Clima que o jogo escreve. `enabled == false` (ou o resource ausente) deixa
/// o manto a zero e o shader não o desenha.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct SnowClimate {
    pub enabled: bool,
    /// Precipitação que cai como neve, mm/h de água.
    pub snowfall_mm_h: f32,
    /// Chuva líquida, mm/h. Derrete o pack.
    pub rain_mm_h: f32,
    /// Temperatura à altura do foco, °C.
    pub temperature_c: f32,
    /// Vento para onde sopra, m/s, XZ.
    pub wind_ms: Vec2,
    /// Sol acima do horizonte, 0 noite .. 1 zénite.
    pub sun: f32,
    /// Segundos de jogo a integrar neste frame. 0 não avança.
    pub game_dt: f32,
    /// Ganho da queda ao vivo; 0 usa [`VISUAL_FALL_GAIN`].
    pub fall_gain: f32,
}

/// Pede um manto já assente: todas as células (e a referência) passam a
/// `depth_m` vezes a retenção da encosta e o abrigo. Consumido no passo
/// seguinte.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct SnowSeed {
    pub depth_m: f32,
}

/// Onde não neva: o chão debaixo de telhados, alpendres e copas. Mapa global
/// esparso (telhas de [`SHELTER_TILE`]² células de [`SNOW_CELL_M`]), 0
/// aberto .. 255 coberto. O jogo rasteriza nele (normalmente as faces de
/// cima dos props) e incrementa `generation`; o manto relê o abrigo da
/// janela quando ela muda.
#[derive(Resource, Clone, Debug, Default)]
pub struct SnowShelter {
    tiles: std::collections::HashMap<IVec2, Box<[u8]>>,
    pub generation: u64,
}

impl SnowShelter {
    /// Abrigo 0..1 na célula global `cell`.
    #[must_use]
    pub fn at_cell(&self, cell: IVec2) -> f32 {
        let tile = cell.div_euclid(IVec2::splat(SHELTER_TILE as i32));
        let local = cell.rem_euclid(IVec2::splat(SHELTER_TILE as i32));
        self.tiles.get(&tile).map_or(0.0, |data| {
            f32::from(data[local.y as usize * SHELTER_TILE + local.x as usize]) / 255.0
        })
    }

    /// Abrigo 0..1 no ponto `xz` do mundo.
    #[must_use]
    pub fn at(&self, xz: Vec2) -> f32 {
        self.at_cell((xz / SNOW_CELL_M).floor().as_ivec2())
    }

    /// Marca uma célula de abrigo total. O chamador agrupa as mudanças numa geração.
    pub fn cover_cell(&mut self, cell: IVec2) -> bool {
        self.raise(cell, u8::MAX)
    }

    fn raise(&mut self, cell: IVec2, value: u8) -> bool {
        let tile = cell.div_euclid(IVec2::splat(SHELTER_TILE as i32));
        let local = cell.rem_euclid(IVec2::splat(SHELTER_TILE as i32));
        let data = self
            .tiles
            .entry(tile)
            .or_insert_with(|| vec![0u8; SHELTER_TILE * SHELTER_TILE].into_boxed_slice());
        let slot = &mut data[local.y as usize * SHELTER_TILE + local.x as usize];
        let changed = value > *slot;
        *slot = (*slot).max(value);
        changed
    }

    /// Marca a célula que contém `xz` com `strength` 0..1 (fica o máximo).
    /// Não mexe em `generation`.
    pub fn add_point(&mut self, xz: Vec2, strength: f32) {
        let value = (strength.clamp(0.0, 1.0) * 255.0).round() as u8;
        if value > 0 {
            self.raise((xz / SNOW_CELL_M).floor().as_ivec2(), value);
        }
    }

    /// Marca as células cujo centro cai no triângulo XZ `tri` com `strength`
    /// 0..1 (fica o máximo). Não mexe em `generation`.
    pub fn add_triangle(&mut self, tri: [Vec2; 3], strength: f32) {
        let value = (strength.clamp(0.0, 1.0) * 255.0).round() as u8;
        if value == 0 {
            return;
        }
        let min = tri[0].min(tri[1]).min(tri[2]);
        let max = tri[0].max(tri[1]).max(tri[2]);
        let c0 = (min / SNOW_CELL_M - Vec2::splat(0.5)).ceil().as_ivec2();
        let c1 = (max / SNOW_CELL_M - Vec2::splat(0.5)).floor().as_ivec2();
        if c1.x < c0.x || c1.y < c0.y {
            // Triângulo menor que uma célula: marca a do centróide.
            let centre = (tri[0] + tri[1] + tri[2]) / 3.0;
            self.raise((centre / SNOW_CELL_M).floor().as_ivec2(), value);
            return;
        }
        for z in c0.y..=c1.y {
            for x in c0.x..=c1.x {
                let p = (Vec2::new(x as f32, z as f32) + Vec2::splat(0.5)) * SNOW_CELL_M;
                if point_in_triangle(p, tri) {
                    self.raise(IVec2::new(x, z), value);
                }
            }
        }
    }

    /// Esquece tudo (troca de mundo/cena).
    pub fn clear(&mut self) {
        self.tiles.clear();
        self.generation = self.generation.wrapping_add(1);
    }
}

fn point_in_triangle(p: Vec2, t: [Vec2; 3]) -> bool {
    let d = |a: Vec2, b: Vec2, c: Vec2| (a.x - c.x) * (b.y - c.y) - (b.x - c.x) * (a.y - c.y);
    let d1 = d(p, t[0], t[1]);
    let d2 = d(p, t[1], t[2]);
    let d3 = d(p, t[2], t[0]);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// XZ do mundo que a janela segue (o herói).
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct SnowFocus {
    pub xz: Vec2,
}

/// Mapa de rastos que o jogo copia de `TrailState`.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct GroundTrailField {
    pub image: Handle<Image>,
    pub texel_m: f32,
    pub texels: u32,
    pub centre: Vec2,
    pub wetness: f32,
    pub enabled: bool,
}

/// Mapa de sombra de nuvem que o jogo copia de `CloudShadows`.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct GroundCloudField {
    pub image: Handle<Image>,
    pub origin: Vec2,
    pub extent: f32,
    /// Direção para a luz com que o mapa foi gerado (já levantada ao mínimo).
    pub light: Vec3,
    pub mean_transmittance: f32,
    pub enabled: bool,
}

/// Texturas neutras e a grelha do manto. O material nasce com elas.
#[derive(Resource, Clone, Debug)]
pub struct GroundTextures {
    pub snow: Handle<Image>,
    pub trail_neutral: Handle<Image>,
    pub cloud_neutral: Handle<Image>,
}

/// Estado do manto. A grelha é profundidade geométrica, metros.
#[derive(Resource)]
pub struct SnowState {
    depth: Vec<f32>,
    heights: Vec<f32>,
    slope_retention: Vec<f32>,
    /// Abrigo 0..1 por célula da janela (cópia do [`SnowShelter`]).
    shelter: Vec<f32>,
    retention_limit: Vec<f32>,
    shelter_generation: Option<u64>,
    /// Pack de um plano aberto ao clima do foco: o que nasce nas células
    /// novas e o que o shader usa fora da janela.
    reference: f32,
    /// Tempo de jogo por integrar e o instante real do último passo.
    pending_dt: f32,
    last_step: f32,
    origin: Vec2,
    placed: bool,
    heights_ready: bool,
    /// O shader desenha quando isto é verdade.
    drawing: bool,
    /// Pode sobrar neve local numa encosta depois de a referência derreter.
    has_local_snow: bool,
    image: Handle<Image>,
    uploaded: Vec<u8>,
}

impl SnowState {
    fn has_snow(&self) -> bool {
        self.reference > 0.0 || self.has_local_snow
    }

    fn new(image: Handle<Image>) -> Self {
        let n = SNOW_CELLS * SNOW_CELLS;
        Self {
            depth: vec![0.0; n],
            heights: vec![0.0; n],
            slope_retention: vec![1.0; n],
            shelter: vec![0.0; n],
            retention_limit: vec![f32::INFINITY; n],
            shelter_generation: None,
            reference: 0.0,
            pending_dt: 0.0,
            last_step: f32::NEG_INFINITY,
            origin: Vec2::ZERO,
            placed: false,
            heights_ready: false,
            drawing: false,
            has_local_snow: false,
            image,
            uploaded: Vec::new(),
        }
    }

    /// O fragmento deve misturar neve.
    #[must_use]
    pub fn draws(&self) -> bool {
        self.drawing
    }

    /// Canto mínimo da janela, mundo XZ.
    #[must_use]
    pub fn origin(&self) -> Vec2 {
        self.origin
    }

    /// Cobertura 0..1 em `xz`. Fora da janela é a da referência.
    #[must_use]
    pub fn coverage_at(&self, xz: Vec2) -> f32 {
        snow_coverage(self.depth_at(xz))
    }

    /// Profundidade em `xz`, metros (bilinear). Fora da janela é a da
    /// referência.
    #[must_use]
    pub fn depth_at(&self, xz: Vec2) -> f32 {
        if !self.drawing {
            return 0.0;
        }
        depth_at(&self.depth, self.origin, xz).unwrap_or(self.reference)
    }

    /// Pack de referência (plano aberto ao clima do foco), metros.
    #[must_use]
    pub fn reference_depth(&self) -> f32 {
        if self.drawing { self.reference } else { 0.0 }
    }

    /// Média da cobertura nos pontos que caem dentro da janela.
    ///
    /// Pontos fora da janela leem a referência. Sem pontos devolve 0.
    #[must_use]
    pub fn mean_points(&self, points: &[Vec2]) -> f32 {
        if points.is_empty() {
            return 0.0;
        }
        let sum: f32 = points.iter().map(|point| self.coverage_at(*point)).sum();
        sum / points.len() as f32
    }
}

/// Cria as texturas (ou reaproveita) e marca as alturas para reamostrar.
pub fn ensure_textures(world: &mut World, images: &mut Assets<Image>) -> GroundTextures {
    let textures = if let Some(textures) = world.get_resource::<GroundTextures>() {
        textures.clone()
    } else {
        let snow = images.add(coverage_image());
        let textures = GroundTextures {
            snow: snow.clone(),
            trail_neutral: images.add(neutral_rgba(0)),
            cloud_neutral: images.add(neutral_rgba(255)),
        };
        world.insert_resource(textures.clone());
        textures
    };
    if let Some(mut state) = world.get_resource_mut::<SnowState>() {
        state.heights_ready = false;
    } else {
        world.insert_resource(SnowState::new(textures.snow.clone()));
    }
    textures
}

fn coverage_image() -> Image {
    let mut image = Image::new(
        Extent3d {
            width: SNOW_CELLS as u32,
            height: SNOW_CELLS as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; SNOW_CELLS * SNOW_CELLS],
        TextureFormat::R8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = clamp_linear();
    image
}

fn neutral_rgba(value: u8) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![value, value, value, 255],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = clamp_linear();
    image
}

fn clamp_linear() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Nearest,
        ..default()
    })
}

/// Integra o manto. Sem [`SnowState`] (mundo ainda sem material de camadas)
/// não faz nada.
#[allow(clippy::too_many_arguments)]
pub fn simulate_snow(
    mut commands: Commands,
    real: Res<Time<Real>>,
    climate: Option<Res<SnowClimate>>,
    focus: Option<Res<SnowFocus>>,
    terrain: Option<Res<TerrainRuntime>>,
    shelter: Option<Res<SnowShelter>>,
    seed: Option<Res<SnowSeed>>,
    state: Option<ResMut<SnowState>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut state) = state else {
        return;
    };
    // Edições vivas, troca de terreno e pavimentação invalidam a área em cache.
    if terrain.as_ref().is_some_and(|runtime| runtime.is_changed()) {
        state.heights_ready = false;
    }
    let climate = climate.as_deref().copied().unwrap_or_default();
    if !climate.enabled {
        if state.drawing || state.has_snow() {
            state.depth.fill(0.0);
            state.reference = 0.0;
            state.drawing = false;
            upload(&mut state, &mut images);
        }
        return;
    }
    let focus = focus.map(|f| f.xz).unwrap_or(Vec2::ZERO);
    if seed.is_none() && climate.snowfall_mm_h <= 0.0 && !state.has_snow() {
        // Zero continua zero ao mover a janela. As alturas e os abrigos
        // da nova posição só são necessários quando voltar a nevar.
        let origin = snap_origin(focus);
        if !state.placed || origin != state.origin {
            state.origin = origin;
            state.placed = true;
            state.heights_ready = false;
            state.shelter_generation = None;
        }
        state.drawing |= terrain.is_some();
        if real.elapsed_secs() - state.last_step >= 1.0 / STEP_HZ {
            state.pending_dt = 0.0;
        } else {
            state.pending_dt += climate.game_dt.clamp(0.0, 120.0);
        }
        return;
    }
    let mut reuse_overlap = false;
    if !state.placed {
        state.origin = snap_origin(focus);
        state.placed = true;
        state.heights_ready = false;
        state.shelter_generation = None;
        let reference = state.reference;
        state.depth.fill(reference);
    } else {
        let next = snap_origin(focus);
        let dx = ((next.x - state.origin.x) / SNOW_CELL_M).round() as i32;
        let dz = ((next.y - state.origin.y) / SNOW_CELL_M).round() as i32;
        if dx != 0 || dz != 0 {
            // Células novas nascem NaN e recebem a referência depois de
            // reamostrar alturas e abrigo.
            if state.heights_ready {
                state.heights = shift_grid_fill(&state.heights, SNOW_CELLS, dx, dz, f32::NAN);
                state.retention_limit =
                    shift_grid_fill(&state.retention_limit, SNOW_CELLS, dx, dz, f32::INFINITY);
                reuse_overlap = true;
            }
            state.depth = shift_grid_fill(&state.depth, SNOW_CELLS, dx, dz, f32::NAN);
            state.origin = next;
            state.heights_ready = false;
            state.shelter_generation = None;
        }
    }
    let refresh = !state.heights_ready;
    if refresh {
        if let Some(terrain) = terrain.as_deref() {
            let origin = state.origin;
            let SnowState {
                heights,
                retention_limit,
                ..
            } = &mut *state;
            if !reuse_overlap {
                heights.fill(f32::NAN);
            }
            sample_heights(terrain, origin, heights, retention_limit);
            let state = &mut *state;
            for (i, retention) in state.slope_retention.iter_mut().enumerate() {
                *retention =
                    slope_retention(slope_deg(&state.heights, i % SNOW_CELLS, i / SNOW_CELLS));
            }
            state.heights_ready = true;
        }
    }
    let shelter_key = Some(shelter.as_deref().map_or(u64::MAX, |s| s.generation));
    let shelter_changed = state.shelter_generation != shelter_key;
    if shelter_changed {
        let origin = state.origin;
        match shelter.as_deref() {
            Some(map) => sample_shelter(map, origin, &mut state.shelter),
            None => state.shelter.fill(0.0),
        }
        state.shelter_generation = shelter_key;
    }
    if let Some(seed) = seed.as_deref() {
        let depth = seed.depth_m.max(0.0);
        state.reference = depth;
        state.depth.fill(f32::NAN);
        commands.remove_resource::<SnowSeed>();
    }
    if state.heights_ready && (refresh || seed.is_some()) {
        let state = &mut *state;
        for (i, depth) in state.depth.iter_mut().enumerate() {
            if depth.is_nan() {
                *depth = state.reference * state.slope_retention[i] * (1.0 - state.shelter[i]);
            }
        }
    }

    state.pending_dt += climate.game_dt.clamp(0.0, 120.0);
    let now = real.elapsed_secs();
    let due = now - state.last_step >= 1.0 / STEP_HZ;
    let has_pack = state.reference > 1e-5 || state.depth.iter().any(|d| *d > 1e-5);
    let working = due
        && state.pending_dt > 0.0
        && state.heights_ready
        && (climate.snowfall_mm_h > 0.0 || has_pack);
    if working {
        let dt = state.pending_dt.min(240.0);
        state.pending_dt = 0.0;
        state.last_step = now;
        let gain = if climate.fall_gain > 0.0 {
            climate.fall_gain
        } else {
            VISUAL_FALL_GAIN
        };
        let fall = climate.snowfall_mm_h * gain;
        state.reference = step_depth(
            state.reference,
            fall,
            climate.rain_mm_h,
            climate.temperature_c,
            climate.sun,
            0.0,
            dt,
        );
        let focus_h = height_nearest(&state.heights, state.origin, focus);
        let state = &mut *state;
        for (i, depth) in state.depth.iter_mut().enumerate() {
            let exposure = 1.0 - state.shelter[i];
            let temp = cell_temperature(climate.temperature_c, state.heights[i], focus_h);
            *depth = step_retained_depth(
                *depth,
                fall * exposure,
                climate.rain_mm_h * exposure,
                temp,
                climate.sun,
                state.slope_retention[i],
                dt,
            );
        }
        advect(
            &mut state.depth,
            SNOW_CELLS,
            climate.wind_ms,
            SNOW_CELL_M,
            dt,
        );
    } else if !due {
        // Sem passo neste frame: o tempo fica pendente.
    } else if !working {
        state.pending_dt = 0.0;
    }
    // Compacted streets keep a dusting; roofs keep the open-air pack.
    // Apply after drifting and seeding, including late-streamed shelters.
    if state.heights_ready
        && (working || refresh || shelter_changed || seed.is_some() || !state.drawing)
    {
        let SnowState {
            depth,
            retention_limit,
            shelter,
            ..
        } = &mut *state;
        for ((d, limit), shelter) in depth.iter_mut().zip(retention_limit).zip(shelter) {
            *d = if *shelter >= 0.99 { 0.0 } else { d.min(*limit) };
        }
        state.drawing = true;
        upload(&mut state, &mut images);
    }
}

fn sample_shelter(map: &SnowShelter, origin: Vec2, out: &mut [f32]) {
    let base = (origin / SNOW_CELL_M).round().as_ivec2();
    // Uma procura por segmento de telha, em vez de uma por célula.
    let tile_size = SHELTER_TILE as i32;
    for (z, row) in out.chunks_exact_mut(SNOW_CELLS).enumerate() {
        let world_z = base.y + z as i32;
        let tile_z = world_z.div_euclid(tile_size);
        let row_offset = world_z.rem_euclid(tile_size) as usize * SHELTER_TILE;
        let mut x = 0;
        while x < row.len() {
            let world_x = base.x + x as i32;
            let local_x = world_x.rem_euclid(tile_size) as usize;
            let len = (SHELTER_TILE - local_x).min(row.len() - x);
            let target = &mut row[x..x + len];
            if let Some(tile) = map.tiles.get(&IVec2::new(world_x.div_euclid(tile_size), tile_z)) {
                let start = row_offset + local_x;
                for (dst, &value) in target.iter_mut().zip(&tile[start..start + len]) {
                    *dst = f32::from(value) / 255.0;
                }
            } else {
                target.fill(0.0);
            }
            x += len;
        }
    }
}

fn sample_heights(
    terrain: &TerrainRuntime,
    origin: Vec2,
    out: &mut [f32],
    retention_limit: &mut [f32],
) {
    let base = terrain.base();
    for z in 0..SNOW_CELLS {
        for x in 0..SNOW_CELLS {
            if !out[z * SNOW_CELLS + x].is_nan() {
                continue;
            }
            let wx = origin.x + (x as f32 + 0.5) * SNOW_CELL_M;
            let wz = origin.y + (z as f32 + 0.5) * SNOW_CELL_M;
            out[z * SNOW_CELLS + x] = base.sample(wx, wz);
            let at = Vec2::new(wx, wz);
            retention_limit[z * SNOW_CELLS + x] = if terrain.paving.covers(at) {
                0.012
            } else if [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y]
                .iter()
                .any(|side| terrain.paving.covers(at + *side * SNOW_CELL_M))
            {
                0.025
            } else {
                f32::INFINITY
            };
        }
    }
}

fn upload(state: &mut SnowState, images: &mut Assets<Image>) {
    let mut bytes = Vec::with_capacity(state.depth.len());
    state.has_local_snow = false;
    for depth in &state.depth {
        state.has_local_snow |= *depth > 0.0;
        let depth = if state.drawing { *depth } else { 0.0 };
        bytes.push(encode_depth(depth));
    }
    if bytes == state.uploaded {
        return;
    }
    if let Some(mut image) = images.get_mut(&state.image) {
        image.data = Some(bytes.clone());
    }
    state.uploaded = bytes;
}

/// Último uniform publicado, para não reconstruir o bind group à toa.
#[derive(Clone, Copy)]
pub(crate) struct Published {
    material: AssetId<TerrainChunkMaterial>,
    snow: Vec4,
    snow_b: Vec4,
    trail: Vec4,
    trail_b: Vec4,
    cloud: Vec4,
    cloud_b: Vec4,
    snow_id: AssetId<Image>,
    trail_id: AssetId<Image>,
    cloud_id: AssetId<Image>,
}

/// Escreve janelas e texturas no material único. Só toca no asset quando
/// algum valor mudou.
pub(crate) fn publish_ground(
    state: Option<Res<SnowState>>,
    textures: Option<Res<GroundTextures>>,
    trails: Option<Res<GroundTrailField>>,
    clouds: Option<Res<GroundCloudField>>,
    chunks: Option<Res<TerrainChunkMaterials>>,
    mut materials: ResMut<Assets<TerrainChunkMaterial>>,
    mut last: Local<Option<Published>>,
) {
    let Some(chunks) = chunks else {
        return;
    };
    let Some(layers) = &chunks.layer else {
        return;
    };
    let Some(textures) = textures else {
        return;
    };
    let material_id = layers.material.id();
    if last.is_some_and(|prev| prev.material != material_id) {
        *last = None;
    }

    let (snow_params, snow_b, snow_id) = match state.as_deref() {
        Some(state) if state.draws() && state.has_snow() => (
            Vec4::new(state.origin().x, state.origin().y, SNOW_CELL_M, 1.0),
            // Referência fora da janela, escala da codificação, ganho do
            // levantamento do chão (1 = a profundidade real).
            Vec4::new(state.reference_depth(), DEPTH_ENCODE_MAX_M, 1.0, 0.0),
            textures.snow.id(),
        ),
        _ => (Vec4::ZERO, Vec4::ZERO, textures.snow.id()),
    };
    let (trail_params, trail_b, trail_id) = match trails.as_deref() {
        Some(field) if field.enabled && field.texels > 0 && field.texel_m > 0.0 => (
            Vec4::new(field.centre.x, field.centre.y, field.texel_m, 1.0),
            Vec4::new(field.texels as f32, field.wetness.clamp(0.0, 1.0), 0.0, 0.0),
            field.image.id(),
        ),
        _ => (Vec4::ZERO, Vec4::ZERO, textures.trail_neutral.id()),
    };
    let (cloud_params, cloud_b, cloud_id) = match clouds.as_deref() {
        Some(field) if field.enabled && field.extent > 1.0 && field.light.y > 0.1 => (
            Vec4::new(
                field.origin.x,
                field.origin.y,
                field.extent,
                field.mean_transmittance.clamp(0.0, 1.0),
            ),
            Vec4::new(field.light.x, field.light.y, field.light.z, 1.0),
            field.image.id(),
        ),
        _ => (Vec4::ZERO, Vec4::ZERO, textures.cloud_neutral.id()),
    };

    let same = last.is_some_and(|prev| {
        prev.snow == snow_params
            && prev.snow_b == snow_b
            && prev.trail == trail_params
            && prev.trail_b == trail_b
            && prev.cloud == cloud_params
            && prev.cloud_b == cloud_b
            && prev.snow_id == snow_id
            && prev.trail_id == trail_id
            && prev.cloud_id == cloud_id
    });
    if same {
        return;
    }

    let Some(mut material) = materials.get_mut(&layers.material) else {
        return;
    };
    if material.snow.id() != snow_id {
        material.snow = textures.snow.clone();
    }
    if material.trail.id() != trail_id {
        material.trail = trails
            .as_ref()
            .filter(|field| field.enabled)
            .map(|field| field.image.clone())
            .unwrap_or_else(|| textures.trail_neutral.clone());
    }
    if material.cloud.id() != cloud_id {
        material.cloud = clouds
            .as_ref()
            .filter(|field| field.enabled)
            .map(|field| field.image.clone())
            .unwrap_or_else(|| textures.cloud_neutral.clone());
    }
    material.params.snow = snow_params;
    material.params.snow_b = snow_b;
    material.params.trail = trail_params;
    material.params.trail_b = trail_b;
    material.params.cloud = cloud_params;
    material.params.cloud_b = cloud_b;
    *last = Some(Published {
        material: material_id,
        snow: snow_params,
        snow_b,
        trail: trail_params,
        trail_b,
        cloud: cloud_params,
        cloud_b,
        snow_id,
        trail_id,
        cloud_id,
    });
}

/// Canto mínimo da janela, alinhado à grelha, com o foco no centro.
#[must_use]
pub fn snap_origin(focus: Vec2) -> Vec2 {
    let span = SNOW_CELLS as f32 * SNOW_CELL_M;
    let min = focus - Vec2::splat(span * 0.5);
    Vec2::new(
        (min.x / SNOW_CELL_M).floor() * SNOW_CELL_M,
        (min.y / SNOW_CELL_M).floor() * SNOW_CELL_M,
    )
}

/// Desloca a grelha quando a origem anda `cell_dx`/`cell_dz` células.
/// Células novas nascem a 0.
#[must_use]
pub fn shift_grid(src: &[f32], cells: usize, cell_dx: i32, cell_dz: i32) -> Vec<f32> {
    shift_grid_fill(src, cells, cell_dx, cell_dz, 0.0)
}

/// [`shift_grid`] com as células novas a `fill`.
#[must_use]
pub fn shift_grid_fill(
    src: &[f32],
    cells: usize,
    cell_dx: i32,
    cell_dz: i32,
    fill: f32,
) -> Vec<f32> {
    let mut out = vec![fill; cells * cells];
    let dx = cell_dx.unsigned_abs() as usize;
    let dz = cell_dz.unsigned_abs() as usize;
    if dx < cells && dz < cells {
        let from = if cell_dz > 0 { dz * cells } else { 0 } + if cell_dx > 0 { dx } else { 0 };
        let to = if cell_dz < 0 { dz * cells } else { 0 } + if cell_dx < 0 { dx } else { 0 };
        let width = cells - dx;
        for offset in (0..(cells - dz) * cells).step_by(cells) {
            out[to + offset..to + offset + width]
                .copy_from_slice(&src[from + offset..from + offset + width]);
        }
    }
    out
}

fn inside(origin: Vec2, xz: Vec2) -> bool {
    let span = SNOW_CELLS as f32 * SNOW_CELL_M;
    xz.x >= origin.x && xz.y >= origin.y && xz.x < origin.x + span && xz.y < origin.y + span
}

/// Quanto da queda a encosta guarda. 1 no plano, 0 a partir de [`SHED_HI_DEG`].
#[must_use]
pub fn slope_retention(slope_deg: f32) -> f32 {
    1.0 - smoothstep(SHED_LO_DEG, SHED_HI_DEG, slope_deg)
}

/// Temperatura da célula: a do foco, menos o lapse da diferença de cota.
#[must_use]
pub fn cell_temperature(temp_c: f32, height_m: f32, focus_height_m: f32) -> f32 {
    temp_c - LAPSE_C_PER_M * (height_m - focus_height_m)
}

/// Um passo de uma célula. `dt_s` é tempo de jogo. A profundidade volta ≥ 0.
#[must_use]
pub fn step_depth(
    depth_m: f32,
    snowfall_mm_h: f32,
    rain_mm_h: f32,
    temp_c: f32,
    sun: f32,
    slope_deg: f32,
    dt_s: f32,
) -> f32 {
    step_retained_depth(
        depth_m, snowfall_mm_h, rain_mm_h, temp_c, sun, slope_retention(slope_deg), dt_s,
    )
}

fn step_retained_depth(
    depth_m: f32,
    snowfall_mm_h: f32,
    rain_mm_h: f32,
    temp_c: f32,
    sun: f32,
    retention: f32,
    dt_s: f32,
) -> f32 {
    if dt_s <= 0.0 {
        return depth_m.max(0.0);
    }
    let hours = dt_s / 3600.0;
    let shed = if retention < 1.0 {
        let k = (1.0 - retention) * dt_s / SHED_SECONDS;
        (-k).exp()
    } else {
        1.0
    };
    let mut depth = depth_m.max(0.0) * shed;
    depth += snowfall_mm_h.max(0.0) * DEPTH_PER_WATER_MM * hours * retention;
    if depth > 0.0 && temp_c > 0.0 {
        let sun_k = 0.35 + 0.65 * sun.clamp(0.0, 1.0);
        let melt = MELT_M_PER_C_PER_DAY * temp_c * (hours / 24.0) * sun_k;
        let rain = rain_mm_h.max(0.0) * RAIN_MELT_M_PER_MM * hours;
        depth = (depth - melt - rain).max(0.0);
    }
    depth.min(1.5)
}

/// Cobertura 0..1. `smoothstep(0, 0.05 m, profundidade)`.
#[must_use]
pub fn snow_coverage(depth_m: f32) -> f32 {
    smoothstep(0.0, FULL_COVER_DEPTH_M, depth_m.max(0.0))
}

/// Fração do pack que o vento passa à célula vizinha neste passo.
#[must_use]
pub fn drift_fraction(wind_ms: Vec2, cell_m: f32, dt_s: f32) -> f32 {
    let speed = wind_ms.length();
    if speed < DRIFT_WIND_MS || dt_s <= 0.0 || cell_m <= 0.0 {
        return 0.0;
    }
    (((speed - DRIFT_WIND_MS) / cell_m) * dt_s * 0.15).clamp(0.0, 0.25)
}

/// Empurra neve para a vizinha a favor do vento. O que sai da janela perde-se.
pub fn advect(depth: &mut [f32], cells: usize, wind_ms: Vec2, cell_m: f32, dt_s: f32) {
    let frac = drift_fraction(wind_ms, cell_m, dt_s);
    if frac <= 0.0 {
        return;
    }
    let dir = wind_ms / wind_ms.length();
    let sx = if dir.x > 0.25 {
        1
    } else if dir.x < -0.25 {
        -1
    } else {
        0
    };
    let sz = if dir.y > 0.25 {
        1
    } else if dir.y < -0.25 {
        -1
    } else {
        0
    };
    if sx == 0 && sz == 0 {
        return;
    }
    let mut next = depth.to_vec();
    for z in 0..cells {
        for x in 0..cells {
            let i = z * cells + x;
            let give = depth[i] * frac;
            if give <= 0.0 {
                continue;
            }
            next[i] -= give;
            let nx = x as i32 + sx;
            let nz = z as i32 + sz;
            if nx >= 0 && nz >= 0 && (nx as usize) < cells && (nz as usize) < cells {
                next[nz as usize * cells + nx as usize] += give;
            }
        }
    }
    depth.copy_from_slice(&next);
}

/// Neve que assenta numa face. Parede (`normal_y` baixo) fica quase seca.
/// `enabled == false` devolve 0 — o chão não muda.
#[must_use]
pub fn facing_snow(coverage: f32, normal_y: f32, enabled: bool) -> f32 {
    if !enabled {
        return 0.0;
    }
    let coverage = coverage.clamp(0.0, 1.0);
    let lo = 0.85 - 0.6 * coverage;
    coverage * smoothstep(lo, lo + 0.2, normal_y)
}

/// Albedo e rugosidade com o manto. Flag desligado devolve os valores de entrada.
#[must_use]
pub fn apply_snow(
    albedo: [f32; 3],
    rough: f32,
    coverage: f32,
    normal_y: f32,
    enabled: bool,
) -> ([f32; 3], f32) {
    let k = facing_snow(coverage, normal_y, enabled);
    if k <= 0.0 {
        return (albedo, rough);
    }
    let albedo = mix3(albedo, SNOW_ALBEDO, k);
    let rough = rough * (1.0 - k) + SNOW_ROUGHNESS * k;
    (albedo, rough)
}

/// Escala da luz já média das nuvens. Flag desligado devolve 1 (o pixel igual).
#[must_use]
pub fn cloud_shadow_ratio(transmittance: f32, mean: f32, enabled: bool) -> f32 {
    if !enabled {
        return 1.0;
    }
    let here = 0.25 + 0.75 * transmittance.clamp(0.0, 1.0);
    let lights = (0.25 + 0.75 * mean.clamp(0.0, 1.0)).max(0.01);
    (here / lights).clamp(0.0, 4.0)
}

/// Sulco. Na neve destapa o chão; na lama escurece. Flag desligado não mexe.
#[must_use]
pub fn apply_trail(
    albedo: [f32; 3],
    rough: f32,
    snow_k: f32,
    pressed_m: f32,
    wetness: f32,
    enabled: bool,
) -> ([f32; 3], f32, f32) {
    if !enabled || pressed_m <= 0.0 {
        return (albedo, rough, snow_k);
    }
    let dig = smoothstep(0.005, 0.03, pressed_m);
    let snow_k = snow_k * (1.0 - dig);
    if wetness > 0.35 && snow_k < 0.25 {
        let mud = dig * wetness.clamp(0.0, 1.0);
        let scale = 1.0 - 0.28 * mud;
        let albedo = [albedo[0] * scale, albedo[1] * scale, albedo[2] * scale];
        let rough = rough * (1.0 - mud) + 0.35 * mud;
        return (albedo, rough, snow_k);
    }
    (albedo, rough, snow_k)
}

/// Byte do texel para `depth_m`: `sqrt(d / max)`.
#[must_use]
pub fn encode_depth(depth_m: f32) -> u8 {
    let v = (depth_m.max(0.0) / DEPTH_ENCODE_MAX_M)
        .sqrt()
        .clamp(0.0, 1.0);
    (v * 255.0).round() as u8
}

/// Inverso de [`encode_depth`] (o shader faz o mesmo: `v² · max`).
#[must_use]
pub fn decode_depth(byte: u8) -> f32 {
    let v = f32::from(byte) / 255.0;
    v * v * DEPTH_ENCODE_MAX_M
}

fn depth_at(depth: &[f32], origin: Vec2, xz: Vec2) -> Option<f32> {
    if !inside(origin, xz) || depth.len() != SNOW_CELLS * SNOW_CELLS {
        return None;
    }
    let fx = ((xz.x - origin.x) / SNOW_CELL_M - 0.5).clamp(0.0, (SNOW_CELLS - 1) as f32);
    let fz = ((xz.y - origin.y) / SNOW_CELL_M - 0.5).clamp(0.0, (SNOW_CELLS - 1) as f32);
    let x0 = fx.floor() as usize;
    let z0 = fz.floor() as usize;
    let x1 = (x0 + 1).min(SNOW_CELLS - 1);
    let z1 = (z0 + 1).min(SNOW_CELLS - 1);
    let tx = fx - x0 as f32;
    let tz = fz - z0 as f32;
    let sample = |x, z| depth[z * SNOW_CELLS + x];
    let d0 = sample(x0, z0) * (1.0 - tx) + sample(x1, z0) * tx;
    let d1 = sample(x0, z1) * (1.0 - tx) + sample(x1, z1) * tx;
    let d = d0 * (1.0 - tz) + d1 * tz;
    Some(if d.is_nan() { 0.0 } else { d })
}

fn height_nearest(heights: &[f32], origin: Vec2, xz: Vec2) -> f32 {
    if heights.len() != SNOW_CELLS * SNOW_CELLS {
        return 0.0;
    }
    let fx = ((xz.x - origin.x) / SNOW_CELL_M - 0.5).clamp(0.0, (SNOW_CELLS - 1) as f32);
    let fz = ((xz.y - origin.y) / SNOW_CELL_M - 0.5).clamp(0.0, (SNOW_CELLS - 1) as f32);
    heights[fz.round() as usize * SNOW_CELLS + fx.round() as usize]
}

fn slope_deg(heights: &[f32], x: usize, z: usize) -> f32 {
    let h = |x: isize, z: isize| {
        let x = x.clamp(0, SNOW_CELLS as isize - 1) as usize;
        let z = z.clamp(0, SNOW_CELLS as isize - 1) as usize;
        heights[z * SNOW_CELLS + x]
    };
    let xi = x as isize;
    let zi = z as isize;
    let dx = (h(xi + 1, zi) - h(xi - 1, zi)) / (2.0 * SNOW_CELL_M);
    let dz = (h(xi, zi + 1) - h(xi, zi - 1)) / (2.0 * SNOW_CELL_M);
    dx.hypot(dz).atan().to_degrees()
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] * (1.0 - t) + b[0] * t,
        a[1] * (1.0 - t) + b[1] * t,
        a[2] * (1.0 - t) + b[2] * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::super::layer_material::TERRAIN_CHUNK_WGSL;
    use super::*;

    #[test]
    fn shelter_rows_match_single_cell_reads_across_tile_edges() {
        let mut map = SnowShelter::default();
        for z in -66..16 {
            for x in -130..12 {
                map.raise(IVec2::new(x, z), ((x * x + z * z) % 256) as u8);
            }
        }
        let base = IVec2::new(-133, -71);
        let mut values = vec![0.0; SNOW_CELLS * SNOW_CELLS];
        sample_shelter(&map, base.as_vec2() * SNOW_CELL_M, &mut values);
        for (i, &value) in values.iter().enumerate() {
            let offset = IVec2::new((i % SNOW_CELLS) as i32, (i / SNOW_CELLS) as i32);
            assert_eq!(value, map.at_cell(base + offset));
        }
        assert!(map.cover_cell(IVec2::new(12, -200)));
        assert!(!map.cover_cell(IVec2::new(12, -200)));
    }

    #[test]
    fn shifted_rows_match_cell_lookup_in_all_directions() {
        let src: Vec<_> = (0..16).map(|i| i as f32).collect();
        for dz in -5..=5 {
            for dx in -5..=5 {
                let moved = shift_grid_fill(&src, 4, dx, dz, -1.0);
                for (i, &v) in moved.iter().enumerate() {
                    let x = (i % 4) as i32 + dx;
                    let z = (i / 4) as i32 + dz;
                    let expected = if (0..4).contains(&x) && (0..4).contains(&z) {
                        src[(z * 4 + x) as usize]
                    } else { -1.0 };
                    assert_eq!(v, expected);
                }
            }
        }
    }

    #[test]
    fn dry_window_resumes_snow_at_the_new_focus() {
        let mut app = App::new();
        let mut images = Assets::<Image>::default();
        ensure_textures(app.world_mut(), &mut images);
        let grid = crate::brush::BrushGrid::new(vec![0; 4], 2, 2, 64.0, 10.0, 0.0).unwrap();
        app.insert_resource(images)
            .init_resource::<Time<Real>>()
            .insert_resource(TerrainRuntime {
                grid: grid.into(),
                spec: Default::default(),
                roads: Vec::new(),
                water: Vec::new(),
                pads: Vec::new(),
                paving: Default::default(),
                voxel: Default::default(),
                deltas: Default::default(),
                paint: Default::default(),
                splat_tuning: Default::default(),
            })
            .insert_resource(SnowClimate { enabled: true, ..default() })
            .add_systems(Update, simulate_snow);
        app.update();
        let focus = Vec2::splat(100.0);
        app.insert_resource(SnowFocus { xz: focus });
        app.update();
        let state = app.world().resource::<SnowState>();
        assert_eq!(state.origin(), snap_origin(focus));
        assert!(!state.heights_ready && !state.has_snow());

        app.insert_resource(SnowSeed { depth_m: 0.3 });
        app.update();
        let state = app.world().resource::<SnowState>();
        assert!(state.heights_ready && state.has_snow());
        assert!((state.depth_at(focus) - 0.3).abs() < 1e-5);

        // A referência pode chegar a zero antes de derreter a última célula.
        app.world_mut().resource_mut::<SnowState>().reference = 0.0;
        app.update();
        assert!(app.world().resource::<SnowState>().has_snow());
        app.world_mut().resource_mut::<SnowClimate>().enabled = false;
        app.update();
        assert!(!app.world().resource::<SnowState>().has_snow());
    }

    #[test]
    fn depth_survives_the_byte() {
        for d in [0.0, 0.002, 0.05, 0.3, 1.2, 1.5] {
            let back = decode_depth(encode_depth(d));
            assert!((back - d).abs() < 0.012 + d * 0.01, "{d} → {back}");
        }
        assert_eq!(encode_depth(9.0), 255);
    }

    #[test]
    fn shelter_marks_cells_under_a_roof() {
        let mut map = SnowShelter::default();
        map.add_triangle(
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(0.0, 10.0),
            ],
            1.0,
        );
        assert_eq!(map.at(Vec2::new(1.0, 1.0)), 1.0);
        assert_eq!(map.at(Vec2::new(9.0, 9.0)), 0.0);
        map.add_point(Vec2::new(-3.0, -3.0), 0.5);
        assert!((map.at(Vec2::new(-3.5, -2.5)) - 0.5).abs() < 0.01);
    }

    #[test]
    fn cold_flat_ground_accumulates() {
        let depth = step_depth(0.0, 1.5, 0.0, -5.0, 0.0, 0.0, 3.0 * 3600.0);
        assert!(
            (depth - 0.045).abs() < 1e-4,
            "3 h de neve a 1,5 mm/h no plano frio: {depth}"
        );
    }

    #[test]
    fn steep_slope_sheds_the_pack() {
        let depth = step_depth(0.2, 1.5, 0.0, -10.0, 0.0, 70.0, 3600.0);
        assert!(
            depth < 0.01,
            "encosta de 70° larga a neve numa hora: {depth}"
        );
        assert_eq!(slope_retention(70.0), 0.0);
        assert_eq!(slope_retention(0.0), 1.0);
    }

    #[test]
    fn warmth_and_rain_melt() {
        let warm = step_depth(0.05, 0.0, 0.0, 4.0, 1.0, 0.0, 24.0 * 3600.0);
        assert_eq!(warm, 0.0, "um dia a +4 °C ao sol derrete o pack");
        let rain = step_depth(0.05, 0.0, 4.0, 1.0, 0.0, 0.0, 2.0 * 3600.0);
        assert_eq!(rain, 0.0, "chuva de 4 mm/h derrete 5 cm em duas horas");
    }

    #[test]
    fn a_high_peak_keeps_snow_while_the_valley_melts() {
        let valley = cell_temperature(3.0, 0.0, 0.0);
        let peak = cell_temperature(3.0, 800.0, 0.0);
        assert!(
            valley > 0.0 && peak < 0.0,
            "vale {valley} °C, pico {peak} °C"
        );
        let valley_depth = step_depth(0.05, 0.0, 0.0, valley, 1.0, 0.0, 24.0 * 3600.0);
        let peak_depth = step_depth(0.02, 1.5, 0.0, peak, 1.0, 10.0, 3.0 * 3600.0);
        assert_eq!(valley_depth, 0.0, "o vale derrete");
        assert!(
            peak_depth > 0.02,
            "o pico continua a acumular: {peak_depth}"
        );
    }

    #[test]
    fn wind_moves_mass_downwind_and_keeps_the_sum() {
        let mut depth = vec![0.0; 16];
        depth[2 * 4 + 2] = 1.0;
        // (speed - 5) / 4 * 1 * 0.15 = 0.2 → speed = 5 + 16/3.
        let wind = Vec2::new(5.0 + 16.0 / 3.0, 0.0);
        assert!((drift_fraction(wind, 4.0, 1.0) - 0.2).abs() < 1e-4);
        advect(&mut depth, 4, wind, 4.0, 1.0);
        assert!((depth[2 * 4 + 2] - 0.8).abs() < 1e-4);
        assert!((depth[2 * 4 + 3] - 0.2).abs() < 1e-4);
        let sum: f32 = depth.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4, "a massa conserva-se: {sum}");
    }

    #[test]
    fn disabled_flags_leave_the_ground_unchanged() {
        let albedo = [0.2, 0.4, 0.1];
        assert_eq!(apply_snow(albedo, 0.8, 1.0, 1.0, false), (albedo, 0.8));
        assert_eq!(
            apply_trail(albedo, 0.8, 1.0, 0.05, 1.0, false),
            (albedo, 0.8, 1.0)
        );
        assert_eq!(cloud_shadow_ratio(0.0, 1.0, false), 1.0);
        let (snowy, rough) = apply_snow(albedo, 0.4, 1.0, 1.0, true);
        assert_eq!(snowy, SNOW_ALBEDO);
        assert!((rough - SNOW_ROUGHNESS).abs() < 1e-5);
        let (wall, wall_rough) = apply_snow(albedo, 0.4, 1.0, 0.0, true);
        assert_eq!(wall, albedo);
        assert_eq!(wall_rough, 0.4);
    }

    #[test]
    fn cloud_ratio_matches_the_broken_cloud_formula() {
        assert!((cloud_shadow_ratio(0.5, 0.5, true) - 1.0).abs() < 1e-5);
        assert!((cloud_shadow_ratio(0.0, 1.0, true) - 0.25).abs() < 1e-5);
    }

    #[test]
    fn a_bootprint_clears_snow_and_mud_darkens() {
        let (_, _, snow_k) = apply_trail([1.0, 1.0, 1.0], 0.8, 1.0, 0.04, 0.0, true);
        assert!(snow_k < 1e-4, "o sulco destapa a neve: {snow_k}");
        let (albedo, _, _) = apply_trail([1.0, 1.0, 1.0], 0.8, 0.0, 0.04, 1.0, true);
        assert!(albedo[0] < 0.8, "lama mais escura: {albedo:?}");
        let same = apply_trail([0.3, 0.3, 0.3], 0.6, 0.4, 0.0, 1.0, true);
        assert_eq!(same, ([0.3, 0.3, 0.3], 0.6, 0.4));
    }

    #[test]
    fn shifting_the_window_keeps_the_overlapping_pack() {
        let mut src = vec![0.0; 9];
        src[0 * 3 + 1] = 0.4;
        let out = shift_grid(&src, 3, 1, 0);
        assert!((out[0] - 0.4).abs() < 1e-6);
        assert_eq!(out[1], 0.0);
        let origin = snap_origin(Vec2::new(10.0, -6.0));
        assert_eq!(origin.x % SNOW_CELL_M, 0.0);
        assert!(origin.x <= 10.0 && origin.x + SNOW_CELLS as f32 * SNOW_CELL_M >= 10.0);
    }

    #[test]
    fn the_shader_uses_the_same_grid_and_keeps_the_flag() {
        assert!(
            TERRAIN_CHUNK_WGSL.contains(&format!("const SNOW_CELLS: f32 = {SNOW_CELLS}.0;")),
            "o shader e a grelha CPU têm de ter o mesmo lado"
        );
        assert!(TERRAIN_CHUNK_WGSL.contains("vec3<f32>(0.80, 0.83, 0.88)"));
        assert!(
            TERRAIN_CHUNK_WGSL.contains("params.snow.w > 0.5"),
            "flag a 0 não pode amostrar o manto"
        );
        assert!(TERRAIN_CHUNK_WGSL.contains("params.trail.w > 0.5"));
        assert!(TERRAIN_CHUNK_WGSL.contains("params.cloud_b.w > 0.5"));
    }
}
