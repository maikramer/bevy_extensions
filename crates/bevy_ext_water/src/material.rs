//! Animated volumetric water material — a two-pass [`MaterialExtension`] over
//! [`StandardMaterial`] (`ExtendedMaterial`) paired with [`crate::waves`].
//!
//! Pass 1 (`AlphaMode::Multiply`, `depth_bias = 0.0`, `pass_params.x = 0.0`):
//! multiplies the underwater scene by wavelength-dependent Beer-Lambert
//! transmittance `(exp(-sigma * optical_path) * (1 - F) * (1 - foam))`
//! modulated by shallow sunlit Voronoi/interference caustics on the real
//! riverbed/submerged rocks/player boots from `prepass_depth`.
//!
//! Pass 2 (`AlphaMode::Add`, `depth_bias = 0.05`, `pass_params.x = 1.0`):
//! adds volumetric subsurface scattering, backlit wave crest SSS, organic
//! cellular triple-source foam, GGX sky/IBL/sun/moon/campfire reflections, and
//! exact fog-compensated horizon blend.

use bevy::asset::Asset;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, StandardMaterial};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::environment::WaterEnvironment;
use crate::waves::WaterWaveWorld;

/// Template WGSL da água (defaults; o jogo reescreve o bloco CONFIG com
/// os valores do mundo antes de o renderer o carregar).
pub const WATER_WGSL: &str = include_str!("water.wesl");

const CONFIG_BEGIN: &str = "// === WORLD CONFIG";
const CONFIG_END: &str = "// === END WORLD CONFIG ===";

/// Water surface = `StandardMaterial` + the volumetric animated-water extension.
pub type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterExtension>;

/// Live world-synced GPU uniforms for volumetric & interactive water.
#[derive(Debug, Clone, Copy, ShaderType)]
pub struct WaterUniforms {
    /// `(origin_x, origin_z, world_size_m, texel_m)` of interactive wave window.
    pub wave_window: Vec4,
    /// `(sun_dir.x, sun_dir.y, sun_dir.z, day_factor)` pointing toward active light.
    pub sun_dir_day: Vec4,
    /// `(sun_tint.r, sun_tint.g, sun_tint.b, glint_strength)`.
    pub sun_color_glint: Vec4,
    /// `(zenith.r, zenith.g, zenith.b, rain_intensity)`.
    pub sky_zenith_rain: Vec4,
    /// `(horizon.r, horizon.g, horizon.b, elapsed_time)`.
    pub sky_horizon_time: Vec4,
    /// `(wind_x, wind_z, wind_strength, wave_amp)`.
    pub wind_params: Vec4,
    /// `(pass_mode [0=absorb, 1=add], caustic_gain, scatter_gain, ocean [0 or 1])`.
    pub pass_params: Vec4,
}

impl Default for WaterUniforms {
    fn default() -> Self {
        Self::surface_default()
    }
}

impl WaterUniforms {
    /// Uniform defaults configured for Pass 1 (`AlphaMode::Multiply` absorption).
    pub fn absorb_default() -> Self {
        Self {
            pass_params: Vec4::new(0.0, 1.15, 1.0, 0.0),
            ..Self::surface_default()
        }
    }

    /// Uniform defaults configured for Pass 2 (`AlphaMode::Add` surface & scatter).
    pub fn surface_default() -> Self {
        Self {
            wave_window: Vec4::new(0.0, 0.0, 0.0, 0.25),
            sun_dir_day: Vec4::new(0.35, 0.72, 0.59, 1.0),
            sun_color_glint: Vec4::new(1.0, 0.95, 0.86, 1.0),
            sky_zenith_rain: Vec4::new(0.085, 0.255, 0.62, 0.0),
            sky_horizon_time: Vec4::new(0.60, 0.755, 0.90, 0.0),
            wind_params: Vec4::new(0.7, 0.25, 1.5, 1.0),
            pass_params: Vec4::new(1.0, 1.15, 1.0, 0.0),
        }
    }
}

/// Optional resource holding per-world authored wave config for runtime uniform sync.
#[derive(Resource, Debug, Clone, Default)]
pub struct WaterConfigResource(pub WaterSurfaceConfig);

/// Water material extension binding dynamic world uniforms, the GPU interactive
/// wave field (`wave_tex`), and the waterbed domain map (`domain_tex`).
#[derive(Debug, Clone, Default, Asset, TypePath, AsBindGroup)]
pub struct WaterExtension {
    #[uniform(100)]
    pub params: WaterUniforms,
    #[texture(101)]
    #[sampler(102)]
    pub wave_tex: Handle<Image>,
    #[texture(103)]
    #[sampler(104)]
    pub domain_tex: Handle<Image>,
    #[texture(105)]
    pub obstacle_tex: Handle<Image>,
}

impl MaterialExtension for WaterExtension {
    fn vertex_shader() -> ShaderRef {
        "shaders/water.wesl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/water.wesl".into()
    }

    /// Water stays OUT of the depth prepass so `prepass_depth` reads the true
    /// riverbed, submerged rocks, bridge pilings, and wading characters underneath.
    fn enable_prepass() -> bool {
        false
    }

    /// Translucent water does not cast hard opaque depth shadows onto the riverbed.
    fn enable_shadows() -> bool {
        false
    }

    fn enable_oit() -> bool {
        false
    }
}

/// Synchronizes live sky/sun/wind/rain/time and GPU wave-window uniforms into
/// every active [`WaterMaterial`] each frame without shader regeneration.
///
/// O host publica [`WaterEnvironment`] (o Vael liga-o ao `bevy_elements` e ao
/// `WeatherState`); sem ele cai-se na `DirectionalLight` da cena e nos
/// defaults da crate.
#[allow(clippy::too_many_arguments)]
pub fn sync_water_materials(
    materials: Option<ResMut<Assets<WaterMaterial>>>,
    waves: Option<Res<WaterWaveWorld>>,
    env: Option<Res<WaterEnvironment>>,
    time: Option<Res<Time>>,
    cfg: Option<Res<WaterConfigResource>>,
    lights: Query<(&GlobalTransform, &DirectionalLight)>,
) {
    let Some(mut materials) = materials else {
        return;
    };
    if materials.is_empty() {
        return;
    }

    let fallback = WaterEnvironment::default();
    let env = env.as_deref().unwrap_or(&fallback);
    // O shader (`active_time`) lê `globals.time` diretamente no GPU quando `w == 0`:
    // publicar `time.elapsed_secs()` aqui forçava `Assets<WaterMaterial>::iter_mut`
    // a marcar todo material como modificado TODOS os frames (recriando bind groups).
    let time_override = if time.is_some() { 0.0 } else { env.time };
    let base_cfg = cfg.as_ref().map(|c| &c.0);
    let wave_amp = base_cfg.map_or(env.wave_amp, |c| c.wave_amp);
    let rain = env.rain.clamp(0.0, 1.0);

    let (wind_x, wind_z, wind_strength) = if let Some(c) = base_cfg {
        (c.wind[0], c.wind[1], c.wind_strength)
    } else {
        (env.wind[0], env.wind[1], env.wind_strength)
    };

    // A direção/cor da luz: ambiente do host; sem ele, a `DirectionalLight`.
    let (sun_dir, day, glint_color, glint_strength) =
        if env.day > 0.0 || lights.is_empty() {
            let color = Vec3::from_array(env.sun_tint);
            let strength = (0.25 + 0.75 * env.day) * (1.0 - 0.65 * rain);
            (env.sun_dir.normalize_or(Vec3::Y), env.day, color, strength)
        } else if let Some((tf, dl)) = lights.iter().next() {
            let dir: Vec3 = tf.back().into();
            let c = dl.color.to_linear();
            let day_est = (dir.y * 3.0).clamp(0.0, 1.0);
            (
                dir.normalize_or(Vec3::Y),
                day_est,
                Vec3::new(c.red, c.green, c.blue),
                (0.3 + 0.7 * day_est) * (1.0 - 0.65 * rain),
            )
        } else {
            (
                Vec3::Y,
                1.0,
                Vec3::new(1.0, 0.95, 0.86),
                1.0 - 0.65 * rain,
            )
        };
    let zenith = Vec3::from_array(env.zenith);
    let horizon = Vec3::from_array(env.horizon);

    let wave_window = waves.as_ref().map_or(Vec4::ZERO, |w| {
        if w.active_water {
            w.window_vec4()
        } else {
            Vec4::ZERO
        }
    });
    let target_sun_dir_day = sun_dir.extend(day);
    let target_sun_color_glint = glint_color.extend(glint_strength);
    let target_sky_zenith_rain = zenith.extend(rain);
    let target_sky_horizon_time = horizon.extend(time_override);
    let target_wind_params = Vec4::new(wind_x, wind_z, wind_strength, wave_amp);

    let w_handles = waves.as_ref().map(|w| (&w.wave_tex, &w.domain_tex, &w.obstacle_tex));
    let near = |a: Vec4, b: Vec4| (a - b).abs().max_element() <= 1e-5;

    let mut dirty_ids: Vec<AssetId<WaterMaterial>> = Vec::new();
    for (id, mat) in materials.iter() {
        let p = &mat.extension.params;
        let params_same = near(p.wave_window, wave_window)
            && near(p.sun_dir_day, target_sun_dir_day)
            && near(p.sun_color_glint, target_sun_color_glint)
            && near(p.sky_zenith_rain, target_sky_zenith_rain)
            && near(p.sky_horizon_time, target_sky_horizon_time)
            && near(p.wind_params, target_wind_params);
        let tex_same = w_handles.is_none_or(|(wt, dt, ot)| {
            mat.extension.wave_tex == *wt
                && mat.extension.domain_tex == *dt
                && mat.extension.obstacle_tex == *ot
        });
        if !params_same || !tex_same {
            dirty_ids.push(id);
        }
    }
    if dirty_ids.is_empty() {
        return;
    }

    for id in dirty_ids {
        let Some(mut mat) = materials.get_mut(id) else {
            continue;
        };
        let p = &mut mat.extension.params;
        p.wave_window = wave_window;
        p.sun_dir_day = target_sun_dir_day;
        p.sun_color_glint = target_sun_color_glint;
        p.sky_zenith_rain = target_sky_zenith_rain;
        p.sky_horizon_time = target_sky_horizon_time;
        p.wind_params = target_wind_params;
        if let Some((wt, dt, ot)) = w_handles {
            if mat.extension.wave_tex != *wt {
                mat.extension.wave_tex = wt.clone();
            }
            if mat.extension.domain_tex != *dt {
                mat.extension.domain_tex = dt.clone();
            }
            if mat.extension.obstacle_tex != *ot {
                mat.extension.obstacle_tex = ot.clone();
            }
        }
    }
}

/// Per-world water configuration extracted from the world XML: the day clock
/// (glint direction follows the real directional light) and the `<Weather>`
/// wind (drives wave speed, slope and chop).
#[derive(Debug, Clone)]
pub struct WaterSurfaceConfig {
    /// `true` when a `<DayCycle>` drives the sun (shader derives the sun/moon
    /// glint from `Globals.time`); otherwise the static `<Sky>` sun.
    pub drive: bool,
    pub clock_start: f32,
    pub clock_speed: f32,
    pub dawn: f32,
    pub dusk: f32,
    pub max_elev: f32,
    /// Daytime floor of the directional light (`min-sun-elevation`).
    pub min_elev: f32,
    pub az_base: f32,
    /// Static sun used when there is no `<DayCycle>`.
    pub sun_elevation: f32,
    pub sun_azimuth: f32,
    pub wind: [f32; 2],
    pub wind_strength: f32,
    /// Amplitude das ondas — o maior `ripple` de qualquer `<Lake>` do mundo
    /// (0.2–2.0; default 1.0). Especializada como `CFG_WAVE_AMP`.
    pub wave_amp: f32,
}

impl Default for WaterSurfaceConfig {
    fn default() -> Self {
        Self {
            drive: false,
            clock_start: 480.0,
            clock_speed: 1.2,
            dawn: 330.0,
            dusk: 1170.0,
            max_elev: 62.0,
            min_elev: 8.0,
            az_base: 205.0,
            sun_elevation: 17.0,
            sun_azimuth: 205.0,
            wind: [0.7, 0.25],
            wind_strength: 1.5,
            wave_amp: 1.0,
        }
    }
}

impl WaterSurfaceConfig {
    /// Rewrite the CONFIG const block of the shader template with this
    /// world's values (the rest of the template is untouched).
    pub fn render_world_shader(&self) -> String {
        let w = self.wind;
        let block = format!(
            "{CONFIG_BEGIN} (generated by the game at each run — edit the XML, not this block) ===\n\
const CFG_DRIVE: f32 = {};\n\
const CFG_CLOCK_START: f32 = {};\n\
const CFG_CLOCK_SPEED: f32 = {};\n\
const CFG_DAWN: f32 = {};\n\
const CFG_DUSK: f32 = {};\n\
const CFG_MAX_ELEV: f32 = {};\n\
const CFG_MIN_ELEV: f32 = {};\n\
const CFG_AZ_BASE: f32 = {};\n\
const CFG_SUN_ELEV: f32 = {};\n\
const CFG_SUN_AZ: f32 = {};\n\
const CFG_WIND_X: f32 = {};\n\
const CFG_WIND_Z: f32 = {};\n\
const CFG_WIND_STRENGTH: f32 = {};\n\
const CFG_WAVE_AMP: f32 = {};\n\
{CONFIG_END}",
            self.drive as i32,
            self.clock_start,
            self.clock_speed,
            self.dawn,
            self.dusk,
            self.max_elev,
            self.min_elev,
            self.az_base,
            self.sun_elevation,
            self.sun_azimuth,
            w[0],
            w[1],
            self.wind_strength,
            self.wave_amp,
        );
        let template = WATER_WGSL;
        let Some(begin) = template.find(CONFIG_BEGIN) else {
            return template.to_string();
        };
        let Some(end_rel) = template[begin..].find(CONFIG_END) else {
            return template.to_string();
        };
        let end = begin + end_rel + CONFIG_END.len();
        let mut out = String::with_capacity(template.len() + block.len());
        out.push_str(&template[..begin]);
        out.push_str(&block);
        out.push_str(&template[end..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O shader gerado substitui o bloco CONFIG e mantém o resto intacto.
    #[test]
    fn test_render_world_shader_specializes_config() {
        let mut config = WaterSurfaceConfig::default();
        config.drive = true;
        config.clock_start = 1140.0;
        config.wind_strength = 2.5;
        let shader = config.render_world_shader();
        assert!(shader.contains("const CFG_DRIVE: f32 = 1;"));
        assert!(shader.contains("const CFG_CLOCK_START: f32 = 1140;"));
        assert!(shader.contains("const CFG_WIND_STRENGTH: f32 = 2.5;"));
        assert!(shader.contains("fn fragment"));
        assert!(shader.contains("wave_normal"));
        assert!(shader.contains("prepass_depth"));
        assert_eq!(shader.matches(CONFIG_END).count(), 1);
    }

    #[test]
    fn test_shader_template_has_no_debug_overlay() {
        for marker in ["DEBUG SPLIT", "DEBUG OVERLAY", "viewport.z / 3.0"] {
            assert!(
                !WATER_WGSL.contains(marker),
                "overlay de debug (`{marker}`) esquecido em water.wesl"
            );
        }
        assert!(WATER_WGSL.contains("out.color = main_pass_post_lighting_processing"));
        assert!(
            WATER_WGSL.contains("fn vertex(vertex_no_morph: Vertex) -> VertexOutput")
                && WATER_WGSL.contains("VERTEX_WAVE_GAIN"),
            "vertex shader da água com deslocamento de onda ausente"
        );
        assert!(!WaterExtension::enable_prepass());
        assert!(!WaterExtension::enable_shadows());
        assert!(!WaterExtension::enable_oit());
    }

    #[test]
    fn test_sync_water_materials_preserves_pass_mode_and_updates_env() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Assets::<WaterMaterial>::default());
        app.insert_resource(WaterEnvironment {
            wind: [0.6, -0.8],
            wind_strength: 2.2,
            rain: 0.45,
            sun_dir: Vec3::new(0.0, 0.6, 0.8),
            day: 0.9,
            zenith: [0.1, 0.3, 0.7],
            horizon: [0.5, 0.7, 0.9],
            sun_tint: [1.0, 0.9, 0.8],
            ..Default::default()
        });
        app.add_systems(Update, sync_water_materials);

        let (absorb_h, surface_h) = {
            let mut mats = app.world_mut().resource_mut::<Assets<WaterMaterial>>();
            let a = mats.add(WaterMaterial {
                base: StandardMaterial {
                    alpha_mode: bevy::material::AlphaMode::Multiply,
                    depth_bias: 0.0,
                    ..Default::default()
                },
                extension: WaterExtension {
                    params: WaterUniforms::absorb_default(),
                    ..Default::default()
                },
            });
            let s = mats.add(WaterMaterial {
                base: StandardMaterial {
                    alpha_mode: bevy::material::AlphaMode::Add,
                    depth_bias: 0.05,
                    ..Default::default()
                },
                extension: WaterExtension {
                    params: WaterUniforms::surface_default(),
                    ..Default::default()
                },
            });
            (a, s)
        };

        app.update();

        let mats = app.world().resource::<Assets<WaterMaterial>>();
        let absorb = mats.get(&absorb_h).unwrap();
        let surface = mats.get(&surface_h).unwrap();
        assert_eq!(absorb.extension.params.pass_params.x, 0.0);
        assert_eq!(surface.extension.params.pass_params.x, 1.0);
        assert!((surface.extension.params.sky_zenith_rain.w - 0.45).abs() < 1e-4);
        assert!((surface.extension.params.wind_params.z - 2.2).abs() < 1e-4);
        assert!((surface.extension.params.sun_dir_day.w - 0.9).abs() < 1e-4);
    }
}
