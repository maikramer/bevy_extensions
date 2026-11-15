//! Plugin, recursos e sistemas de runtime.
//!
//! O jogo só tem de: inserir um [`GrassProvider`] (o terreno), configurar o
//! plugin, e opcionalmente escrever [`GrassTint`] e o `bevy_grass::GrassWind`.
//! Tudo o resto — bake, campos, materiais, sombras, trampling — é daqui.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::{Camera3d, RenderTarget};
use bevy::color::Color;
use bevy::image::Image;
use bevy::light::DirectionalLight;
use bevy::math::{UVec2, Vec3};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy_grass::{
    GrassField as BgGrassField, GrassFieldOverrides, GrassGround, GrassPlugin as BgGrassPlugin,
    GrassQuality, GrassSettings as BgGrassSettings, GrassShadows, GrassStats, GrassStyle, GrassSun,
    GrassSupport, NoGrass,
};

use crate::bake::{FieldData, GrassBakeConfig, StaticMaps, bake_dynamic, bake_static};
use crate::specs::{GrassSettingsSpec, GrassStyleSpec, style_from_spec};
use crate::terrain::{FieldPlan, GrassFieldLayout, GrassLayout, GrassProvider, SingleFieldLayout};

/// Settings de runtime (mutáveis pelo jogo/profiler). O `GrassSettingsSpec`
/// autorado escreve aqui quando existe.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct GrassSettings {
    pub enabled: bool,
    /// Multiplica a densidade do tier de qualidade.
    pub density_scale: f32,
    pub quality: GrassQuality,
    pub shadows: GrassShadows,
    /// Multiplicador global do vento (por cima do `wind_scale` da paleta).
    pub wind_scale: f32,
    /// Teto de talos por câmara (`None` = o do tier).
    pub instance_capacity: Option<u32>,
    /// Teto de slices por câmara (`None` = o do tier).
    pub max_chunk_slices: Option<u32>,
}

impl Default for GrassSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            density_scale: 1.0,
            quality: GrassQuality::Medium,
            shadows: GrassShadows::Near,
            wind_scale: 1.0,
            instance_capacity: None,
            max_chunk_slices: None,
        }
    }
}

/// Specs de settings que o jogo carrega para um recurso (em vez de
/// componentes na ECS). Componentes na cena têm prioridade.
#[derive(Resource, Default, Clone, Debug)]
pub struct GrassSettingsSpecValue(pub Option<GrassSettingsSpec>);

/// Lista de paletas que o jogo carrega para um recurso. Componentes na cena
/// têm prioridade.
#[derive(Resource, Default, Clone, Debug)]
pub struct GrassStyleSpecSet(pub Vec<GrassStyleSpec>);

/// Tinta do dia/noite (multiplica as cores dos talos). O jogo escreve-a.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct GrassTint(pub [f32; 3]);

impl Default for GrassTint {
    fn default() -> Self {
        Self([1.0, 1.0, 1.0])
    }
}

/// Estado público (loading/QA/profiler) + estado interno do bake.
#[derive(Resource, Default)]
pub struct GrassState {
    /// Ainda há um bake por terminar.
    pub busy: bool,
    /// Talos desenhados na última leitura do `GrassStats`.
    pub blade_count: usize,
    /// Campos vivos neste momento.
    pub field_count: usize,
    /// Suporte do GPU (compute/indirect) reportado pelo `bevy_grass`.
    pub support: GrassSupport,
    task: Option<Task<BakeOutput>>,
    built_provider: Option<usize>,
    built_world_size: f32,
    built_revision: u64,
    built_plans: Vec<FieldPlan>,
    statics: Option<Arc<StaticMaps>>,
    assets: Vec<FieldAssets>,
    edit_timer: f32,
    support_logged: bool,
}

#[allow(dead_code)]
struct FieldAssets {
    heights: Handle<Image>,
    mask: Handle<Image>,
    style: Handle<GrassStyle>,
}

struct BakeOutput {
    provider: usize,
    world_size: f32,
    revision: u64,
    plans: Vec<FieldPlan>,
    statics: Arc<StaticMaps>,
    fields: Vec<FieldData>,
}

/// Marcador de uma entidade de campo criada pelo plugin (detalhe de
/// implementação; público só porque `maintain_grass` é público).
#[doc(hidden)]
#[derive(Component)]
pub struct GrassFieldEntity {
    pub(crate) biome: u16,
}

/// Estilos por bioma, sincronizados dos `GrassStyleSpec` da cena.
#[derive(Resource, Default)]
pub struct GrassStyleRegistry {
    styles: HashMap<u16, Handle<GrassStyle>>,
    wind_scale: HashMap<u16, f32>,
}

impl GrassStyleRegistry {
    /// Handle do estilo do bioma (se houver paleta autorada).
    #[must_use]
    pub fn handle(&self, biome: u16) -> Option<&Handle<GrassStyle>> {
        self.styles.get(&biome)
    }

    /// Multiplicador de vento do bioma (1.0 se não houver paleta).
    #[must_use]
    pub fn wind_scale(&self, biome: u16) -> f32 {
        self.wind_scale.get(&biome).copied().unwrap_or(1.0)
    }

    /// Nº de paletas.
    #[must_use]
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }
}

/// Ordem dos sistemas do crate; o jogo pode inserir os seus entre eles.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrassSystems {
    /// Specs autorados → settings/estilos/recursos.
    SyncSpecs,
    /// Decide quando refazer o bake (o jogo pode instalar o seu).
    Maintain,
    /// Termina o bake e spawna os campos.
    Poll,
    /// Tinta/vento dos campos vivos.
    Tint,
    /// Estatísticas.
    Stats,
}

/// Configuração do plugin.
#[derive(Clone)]
pub struct GrassPluginConfig {
    /// Layout dos campos (o jogo implementa; default = um campo do mundo).
    pub layout: Arc<dyn GrassFieldLayout>,
    pub bake: GrassBakeConfig,
    /// `false` deixa o jogo instalar `bevy_ext_grass::maintain_grass` no set
    /// `GrassSystems::Maintain` (para o embrulhar no profiler, por exemplo).
    pub install_maintain: bool,
}

impl Default for GrassPluginConfig {
    fn default() -> Self {
        Self {
            layout: Arc::new(SingleFieldLayout),
            bake: GrassBakeConfig::default(),
            install_maintain: true,
        }
    }
}

/// Plugin da relva.
#[derive(Default)]
pub struct GrassPlugin {
    pub config: GrassPluginConfig,
}

impl Plugin for GrassPlugin {
    fn build(&self, app: &mut App) {
        let settings = app
            .world()
            .get_resource::<GrassSettings>()
            .cloned()
            .unwrap_or_default();
        app.init_resource::<GrassSettings>()
            .init_resource::<GrassState>()
            .init_resource::<GrassTint>()
            .init_resource::<GrassStyleRegistry>()
            .init_resource::<GrassSettingsSpecValue>()
            .init_resource::<GrassStyleSpecSet>()
            .register_type::<crate::specs::GrassSettingsSpec>()
            .register_type::<crate::specs::GrassQualitySpec>()
            .register_type::<crate::specs::GrassStyleSpec>()
            .register_type::<crate::specs::GrassPlantSpec>()
            .insert_resource(self.config.bake)
            .insert_resource(GrassLayout(self.config.layout.clone()))
            .add_plugins(BgGrassPlugin {
                settings: settings_to_bg(&settings),
                wind: true,
                lights: true,
            })
            .configure_sets(
                Update,
                (
                    GrassSystems::SyncSpecs,
                    GrassSystems::Maintain,
                    GrassSystems::Poll,
                    GrassSystems::Tint,
                    GrassSystems::Stats,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    (sync_settings_spec, sync_style_specs, apply_settings).chain(),
                    tag_sun,
                    tag_cameras,
                )
                    .in_set(GrassSystems::SyncSpecs),
            )
            .add_systems(
                Update,
                (apply_field_overrides, update_stats).in_set(GrassSystems::Tint),
            )
            .add_systems(Update, poll_bake.in_set(GrassSystems::Poll));
        #[cfg(feature = "avian")]
        crate::trample::plugin(app);
        if self.config.install_maintain {
            app.add_systems(Update, maintain_grass.in_set(GrassSystems::Maintain));
        }
    }
}

/// Converte os settings de runtime no recurso do `bevy_grass`.
#[must_use]
pub fn settings_to_bg(settings: &GrassSettings) -> BgGrassSettings {
    let mut bg = BgGrassSettings::from(settings.quality);
    bg.shadows = settings.shadows;
    if let Some(capacity) = settings.instance_capacity {
        bg.instance_capacity = capacity.clamp(2, 4_000_000);
    }
    if let Some(slices) = settings.max_chunk_slices {
        bg.max_chunk_slices = slices.clamp(1, 65_535);
    }
    bg.density_scale = if settings.enabled {
        bg.density_scale * settings.density_scale.max(0.0)
    } else {
        0.0
    };
    bg
}

fn apply_settings(settings: Res<GrassSettings>, mut bg: ResMut<BgGrassSettings>) {
    if settings.is_changed() {
        *bg = settings_to_bg(&settings);
        if std::env::var("GRASS_DEBUG").as_deref() == Ok("1") {
            info!(
                "bevy_ext_grass: settings radius {:.1} m, densidade ×{:.2}, sombras {:?}, capacity {:?}, slices {:?}",
                bg.radius,
                bg.density_scale,
                bg.shadows,
                settings.instance_capacity,
                settings.max_chunk_slices
            );
        }
    }
}

/// `GrassSettingsSpec` autorado → [`GrassSettings`] de runtime.
fn sync_settings_spec(
    specs: Query<&GrassSettingsSpec>,
    mut removed: RemovedComponents<GrassSettingsSpec>,
    resource: Res<GrassSettingsSpecValue>,
    mut settings: ResMut<GrassSettings>,
) {
    let spec = specs.iter().next().or(resource.0.as_ref());
    if let Some(spec) = spec {
        let next = GrassSettings {
            enabled: spec.enabled,
            density_scale: spec.density_scale,
            quality: spec.quality.into(),
            shadows: spec.shadows_mode(),
            wind_scale: spec.wind_scale,
            instance_capacity: settings.instance_capacity,
            max_chunk_slices: settings.max_chunk_slices,
        };
        if *settings != next {
            *settings = next;
        }
    } else if removed.read().next().is_some() || resource.is_changed() {
        let caps = (settings.instance_capacity, settings.max_chunk_slices);
        let mut next = GrassSettings::default();
        next.instance_capacity = caps.0;
        next.max_chunk_slices = caps.1;
        *settings = next;
    }
}

/// `GrassStyleSpec` autorado → assets `GrassStyle` (um por bioma).
fn sync_style_specs(
    mut styles: ResMut<Assets<GrassStyle>>,
    specs: Query<&GrassStyleSpec>,
    changed: Query<(), Changed<GrassStyleSpec>>,
    mut removed: RemovedComponents<GrassStyleSpec>,
    resource: Res<GrassStyleSpecSet>,
    mut registry: ResMut<GrassStyleRegistry>,
) {
    let dirty =
        changed.iter().next().is_some() || removed.read().next().is_some() || resource.is_changed();
    if !dirty {
        return;
    }
    let chosen: Vec<&GrassStyleSpec> = if specs.is_empty() {
        resource.0.iter().collect()
    } else {
        specs.iter().collect()
    };
    let mut seen = Vec::new();
    for spec in chosen {
        let style = style_from_spec(spec);
        seen.push(spec.biome);
        match registry.styles.get(&spec.biome).cloned() {
            Some(handle) if styles.get(&handle).is_some() => {
                if let Some(mut asset) = styles.get_mut(&handle) {
                    *asset = style;
                }
            }
            _ => {
                let handle = styles.add(style);
                registry.styles.insert(spec.biome, handle);
            }
        }
        registry.wind_scale.insert(spec.biome, spec.wind_scale);
    }
    registry.styles.retain(|biome, _| seen.contains(biome));
    registry.wind_scale.retain(|biome, _| seen.contains(biome));
    info!(
        "bevy_ext_grass: {} paleta(s) de bioma carregada(s) dos specs da cena",
        seen.len()
    );
}

/// Decide quando refazer o bake: provider novo, mundo novo ou `revision()`
/// do terreno mudou (com debounce). Público para o jogo o instalar no set
/// `GrassSystems::Maintain` quando `install_maintain = false`.
#[allow(clippy::too_many_arguments)]
pub fn maintain_grass(
    mut commands: Commands,
    provider: Option<Res<GrassProvider>>,
    layout: Res<GrassLayout>,
    config: Res<GrassBakeConfig>,
    settings: Res<GrassSettings>,
    mut state: ResMut<GrassState>,
    time: Res<Time>,
    existing: Query<Entity, With<GrassFieldEntity>>,
) {
    if state.task.is_some() {
        return;
    }
    let Some(provider) = provider else {
        despawn_fields(&mut commands, &existing);
        state.busy = false;
        state.built_provider = None;
        state.field_count = 0;
        return;
    };
    if !settings.enabled {
        despawn_fields(&mut commands, &existing);
        state.busy = false;
        // `None` e não o provider atual: religar o toggle volta a construir.
        state.built_provider = None;
        state.field_count = 0;
        return;
    }

    let provider_id = Arc::as_ptr(&provider.0) as *const () as usize;
    let world_size = provider.world_size().max(1.0);
    let revision = provider.revision();
    let needs_provider = state.built_provider != Some(provider_id)
        || (state.built_world_size - world_size).abs() > 1e-3;
    let needs_edit = revision != state.built_revision;
    if !needs_provider && !needs_edit {
        state.edit_timer = 0.0;
        return;
    }
    if needs_edit && !needs_provider {
        state.edit_timer += time.delta_secs();
        if state.edit_timer < config.edit_debounce {
            return;
        }
    }
    state.edit_timer = 0.0;

    if needs_provider {
        despawn_fields(&mut commands, &existing);
    }

    let plans = layout.fields(&**provider);
    if std::env::var("GRASS_DEBUG").as_deref() == Ok("1") {
        let summary: Vec<(u16, [f32; 2], [f32; 2])> = plans
            .iter()
            .map(|plan| {
                (
                    plan.biome.0,
                    [plan.center.x, plan.center.y],
                    [plan.size.x, plan.size.y],
                )
            })
            .collect();
        info!("bevy_ext_grass: layout {summary:?}");
    }
    let reuse_static = if !needs_provider && state.built_plans == plans {
        state.statics.clone()
    } else {
        None
    };
    let terrain = provider.0.clone();
    let bake_config = *config;
    state.busy = true;
    state.task = Some(
        AsyncComputeTaskPool::get_or_init(Default::default).spawn(async move {
            let statics = match reuse_static {
                Some(statics) => statics,
                None => Arc::new(bake_static(&*terrain, &bake_config)),
            };
            let fields = bake_dynamic(&*terrain, &bake_config, &plans, &statics);
            BakeOutput {
                provider: provider_id,
                world_size,
                revision,
                plans,
                statics,
                fields,
            }
        }),
    );
}

/// Termina o bake e constrói as entidades de campo.
#[allow(clippy::too_many_arguments)]
fn poll_bake(
    mut commands: Commands,
    provider: Option<Res<GrassProvider>>,
    settings: Res<GrassSettings>,
    tint: Res<GrassTint>,
    mut state: ResMut<GrassState>,
    mut images: ResMut<Assets<Image>>,
    mut styles: ResMut<Assets<GrassStyle>>,
    mut registry: ResMut<GrassStyleRegistry>,
    existing: Query<Entity, With<GrassFieldEntity>>,
) {
    let Some(task) = state.task.as_mut() else {
        return;
    };
    let Some(out) = block_on(poll_once(&mut *task)) else {
        return;
    };
    state.task = None;

    let current = provider
        .as_ref()
        .map(|provider| Arc::as_ptr(&provider.0) as *const () as usize);
    if current != Some(out.provider) || !settings.enabled {
        // O provider/estado mudou enquanto o bake corria: o maintain do
        // próximo frame decide outra vez.
        state.busy = false;
        return;
    }

    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let tint_color = Color::linear_rgb(tint.0[0], tint.0[1], tint.0[2]);
    let mut assets = Vec::with_capacity(out.fields.len());
    for data in out.fields {
        let biome = data.plan.biome.0;
        let style = match registry.styles.get(&biome).cloned() {
            Some(handle) if styles.get(&handle).is_some() => handle,
            _ => {
                let handle = styles.add(GrassStyle::MEADOW);
                registry.styles.insert(biome, handle.clone());
                handle
            }
        };
        let wind_scale = registry.wind_scale(biome) * settings.wind_scale;
        let heights = images.add(heights_image(&data.heights, data.texels));
        let mask = images.add(density_image(&data.mask, data.texels));
        let transform =
            Transform::from_translation(Vec3::new(data.plan.center.x, 0.0, data.plan.center.y))
                .with_rotation(Quat::from_rotation_y(data.plan.yaw));
        commands.spawn((
            Name::new(format!("bevy_ext_grass: bioma {biome}")),
            GrassFieldEntity { biome },
            BgGrassField {
                size: data.plan.size,
                style: style.clone(),
                ground: GrassGround::Heightmap {
                    image: heights.clone(),
                    height_scale: 1.0,
                },
                density_map: Some(mask.clone()),
            },
            GrassFieldOverrides {
                tint: Some(tint_color),
                wind_scale,
                ..default()
            },
            transform,
            Visibility::default(),
        ));
        assets.push(FieldAssets {
            heights,
            mask,
            style,
        });
    }

    state.field_count = assets.len();
    state.assets = assets;
    state.built_provider = Some(out.provider);
    state.built_world_size = out.world_size;
    state.built_revision = out.revision;
    state.built_plans = out.plans;
    state.statics = Some(out.statics);
    state.busy = false;
}

fn apply_field_overrides(
    settings: Res<GrassSettings>,
    tint: Res<GrassTint>,
    registry: Res<GrassStyleRegistry>,
    mut fields: Query<(&GrassFieldEntity, &mut GrassFieldOverrides)>,
) {
    if !settings.is_changed() && !tint.is_changed() && !registry.is_changed() {
        return;
    }
    let color = Color::linear_rgb(tint.0[0], tint.0[1], tint.0[2]);
    for (field, mut overrides) in &mut fields {
        overrides.tint = Some(color);
        overrides.wind_scale = registry.wind_scale(field.biome) * settings.wind_scale;
    }
}

/// O sol da relva é o direcional que projeta sombras.
fn tag_sun(
    mut commands: Commands,
    tagged: Query<(), With<GrassSun>>,
    lights: Query<(Entity, &DirectionalLight)>,
) {
    if !tagged.is_empty() {
        return;
    }
    if let Some((entity, _)) = lights.iter().find(|(_, light)| light.shadow_maps_enabled) {
        commands.entity(entity).insert(GrassSun);
    }
}

/// Câmaras de render-para-textura (impostores, minimapas) não desenham relva.
fn tag_cameras(
    mut commands: Commands,
    cameras: Query<(Entity, &RenderTarget), (With<Camera3d>, Without<NoGrass>)>,
) {
    for (entity, target) in &cameras {
        if matches!(target, RenderTarget::Image(_)) {
            commands.entity(entity).insert(NoGrass);
        }
    }
}

fn update_stats(
    stats: Option<Res<GrassStats>>,
    support: Option<Res<GrassSupport>>,
    mut state: ResMut<GrassState>,
    mut stats_logged: Local<bool>,
) {
    if let Some(stats) = stats {
        state.blade_count = stats.gpu.total_blades() as usize;
        if !*stats_logged
            && stats.gpu.readings > 0
            && std::env::var("GRASS_DEBUG").as_deref() == Ok("1")
        {
            *stats_logged = true;
            info!(
                "bevy_ext_grass: {} talos (perto {}, meio {}, longe {}), {} slices, {} overflows",
                stats.gpu.total_blades(),
                stats.gpu.blades[0],
                stats.gpu.blades[1],
                stats.gpu.blades[2],
                stats.gpu.slices,
                stats.gpu.overflows
            );
        }
    }
    if let Some(support) = support {
        state.support = *support;
        if !state.support_logged && !matches!(state.support, GrassSupport::Unknown) {
            state.support_logged = true;
            if state.support.is_supported() {
                info!("bevy_ext_grass: GPU com compute/indirect — relva ativa");
            } else {
                warn!(
                    "bevy_ext_grass: GPU sem suporte para a relva ({:?}); nada será desenhado",
                    state.support
                );
            }
        }
    }
}

fn despawn_fields(commands: &mut Commands, existing: &Query<Entity, With<GrassFieldEntity>>) {
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
}

fn heights_image(heights: &[f32], texels: UVec2) -> Image {
    Image::new(
        Extent3d {
            width: texels.x,
            height: texels.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        heights.iter().flat_map(|h| h.to_le_bytes()).collect(),
        TextureFormat::R32Float,
        RenderAssetUsages::MAIN_WORLD,
    )
}

fn density_image(mask: &[u8], texels: UVec2) -> Image {
    Image::new(
        Extent3d {
            width: texels.x,
            height: texels.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        mask.to_vec(),
        TextureFormat::Rg8Unorm,
        RenderAssetUsages::MAIN_WORLD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_settings_zero_density() {
        let settings = GrassSettings {
            enabled: false,
            ..Default::default()
        };
        assert_eq!(settings_to_bg(&settings).density_scale, 0.0);
    }

    #[test]
    fn quality_and_caps_reach_upstream_settings() {
        let settings = GrassSettings {
            quality: GrassQuality::High,
            shadows: GrassShadows::Off,
            instance_capacity: Some(1234),
            max_chunk_slices: Some(99),
            density_scale: 0.5,
            ..Default::default()
        };
        let bg = settings_to_bg(&settings);
        assert_eq!(bg.radius, BgGrassSettings::HIGH.radius);
        assert_eq!(bg.shadows, GrassShadows::Off);
        assert_eq!(bg.instance_capacity, 1234);
        assert_eq!(bg.max_chunk_slices, 99);
        assert!((bg.density_scale - 0.5).abs() < 1e-6);
    }
}
