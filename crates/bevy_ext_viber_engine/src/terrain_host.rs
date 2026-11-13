//! Os walks do IR de parse que alimentam os materiais de terreno/água.
//!
//! O `bevy_ext_terrain_runtime` não depende do IR de parsing
//! (`EntitySpec`/`EntityKind`) — por isso os antigos
//! `WaterSurfaceConfig::from_world`/`TerrainChunkConfig::from_world` ficaram
//! no lado do Viber (TODOs lá registam a decisão). Este módulo é o lado do
//! Viber: os mesmos walks, sobre o IR `crate::recipes` e os tipos de terreno
//! do crate extraído (via shim `crate::terrain`).

use crate::terrain::layer_material::TerrainChunkConfig;
use crate::terrain::water_material::WaterSurfaceConfig;

/// Scan the parsed world for `<DayCycle>` and `<Weather>` and merge their
/// values over the defaults (same walk as `crate::sky::SkyConfig`).
pub fn water_surface_config_from_world(entities: &[crate::recipes::EntitySpec]) -> WaterSurfaceConfig {
    let mut config = WaterSurfaceConfig::default();
    walk_entities(entities, &mut config);
    config
}

fn walk_entities(specs: &[crate::recipes::EntitySpec], config: &mut WaterSurfaceConfig) {
    for spec in specs {
        match &spec.kind {
            crate::recipes::EntityKind::DayCycle {
                minute_of_day,
                minutes_per_real_second,
                dawn_minute,
                dusk_minute,
                max_sun_elevation,
                sun_azimuth_base,
                min_sun_elevation,
                ..
            } => {
                config.drive = true;
                config.clock_start = *minute_of_day;
                config.clock_speed = *minutes_per_real_second;
                config.dawn = *dawn_minute;
                config.dusk = *dusk_minute;
                config.max_elev = *max_sun_elevation;
                config.min_elev = *min_sun_elevation;
                config.az_base = *sun_azimuth_base;
            }
            crate::recipes::EntityKind::Weather {
                wind,
                wind_strength,
                ..
            } => {
                config.wind = *wind;
                config.wind_strength = *wind_strength;
            }
            // `ripple` do lago com o maior valor do mundo — é o multiplier
            // de amplitude das ondas (`CFG_WAVE_AMP`).
            crate::recipes::EntityKind::Lake { spec } => {
                config.wave_amp = config.wave_amp.max(spec.ripple.clamp(0.2, 2.0));
            }
            // `<Sky sun-elevation/sun-azimuth>`: sem `<DayCycle>` o domo usa
            // estes valores, mas a água ficava com os defaults 17°/205° e o
            // brilho especular apontava para um sol que não está no céu.
            crate::recipes::EntityKind::HudElement { tag, attrs } if tag == "sky" => {
                let f = |key: &str| {
                    attrs
                        .iter()
                        .find(|(k, _)| k == key)
                        .and_then(|(_, v)| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                };
                if let Some(v) = f("sun-elevation") {
                    config.sun_elevation = v;
                }
                if let Some(v) = f("sun-azimuth") {
                    config.sun_azimuth = v;
                }
            }
            _ => {}
        }
        walk_entities(&spec.children, config);
    }
}

/// First `<Terrain>` with `layers` (the same spec the bootstrap bakes
/// chunk splats for) — delega em [`TerrainChunkConfig::from_spec`].
pub fn terrain_chunk_config_from_world(
    entities: &[crate::recipes::EntitySpec],
) -> Option<TerrainChunkConfig> {
    fn walk(specs: &[crate::recipes::EntitySpec]) -> Option<crate::terrain::spec::TerrainSpec> {
        for spec in specs {
            if let crate::recipes::EntityKind::Terrain { spec } = &spec.kind
                && !spec.layers.is_empty()
            {
                return Some(spec.clone());
            }
            if let Some(found) = walk(&spec.children) {
                return Some(found);
            }
        }
        None
    }
    let spec = walk(entities)?;
    TerrainChunkConfig::from_spec(&spec)
}
