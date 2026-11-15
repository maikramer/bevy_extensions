//! Ponte do material de água da crate `bevy_ext_water` para o mundo do Vael.
//!
//! O material (dois passes Multiply/Add), os uniforms e o sync de ambiente
//! vivem na crate. Aqui fica apenas o que depende do IR de cena desta engine:
//! ler `<DayCycle>`, `<Weather>`, `<Lake>` e a `<HudElement tag="sky">` para
//! configurar a especialização do shader.

pub use bevy_ext_water::material::{
    WATER_WGSL, WaterConfigResource, WaterExtension, WaterMaterial, WaterSurfaceConfig,
    WaterUniforms,
};

use crate::recipes::{EntityKind, EntitySpec};

/// Configuração do shader de água a partir do mundo declarativo (dia/clima,
/// amplitude das ondas e sol estático).
pub fn water_surface_config_from_world(entities: &[EntitySpec]) -> WaterSurfaceConfig {
    let mut config = WaterSurfaceConfig::default();
    walk_entities(entities, &mut config);
    config
}

fn walk_entities(specs: &[EntitySpec], config: &mut WaterSurfaceConfig) {
    for spec in specs {
        match &spec.kind {
            EntityKind::DayCycle {
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
            EntityKind::Weather {
                wind,
                wind_strength,
                ..
            } => {
                config.wind = *wind;
                config.wind_strength = *wind_strength;
            }
            EntityKind::Lake { spec } => {
                config.wave_amp = config.wave_amp.max(spec.ripple.clamp(0.2, 2.0));
            }
            EntityKind::HudElement { tag, attrs } if tag == "sky" => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::PhysicsSpec;
    use crate::recipes::TransformSpec;

    fn spec(kind: EntityKind) -> EntitySpec {
        EntitySpec {
            prototype_instance: None,
            name: None,
            tag: None,
            script: None,
            props: Vec::new(),
            links: Default::default(),
            transform: TransformSpec::default(),
            physics: PhysicsSpec::default(),
            kind,
            children: Vec::new(),
            source: None,
        }
    }

    fn with_child(mut parent: EntitySpec, child: EntitySpec) -> EntitySpec {
        parent.children.push(child);
        parent
    }

    /// `<DayCycle>`/`<Weather>`/`<Lake>` preenchem a config com que o shader
    /// é especializado; sem eles ficam os defaults da crate.
    #[test]
    fn water_surface_config_reads_the_declarative_world() {
        let world = vec![with_child(
            with_child(
                spec(EntityKind::Group),
                spec(EntityKind::DayCycle {
                    minute_of_day: 1140.0,
                    minutes_per_real_second: 3.0,
                    dawn_minute: 330.0,
                    dusk_minute: 1170.0,
                    ambient_day: 0.26,
                    ambient_night: 0.07,
                    drive_ambient: true,
                    max_sun_elevation: 55.0,
                    sun_azimuth_base: 180.0,
                    min_sun_elevation: 8.0,
                    day_speed_scale: 1.0,
                    night_speed_scale: 1.0,
                    night_cap_minute: 0.0,
                    mode: crate::worldsys::DayCycleMode::Continuous,
                    day_minute: 720.0,
                    night_minute: 1320.0,
                    transition_seconds: 4.0,
                    shadow_refresh_hz: 0.0,
                }),
            ),
            spec(EntityKind::Weather {
                wind: [1.0, 0.0],
                wind_strength: 3.0,
                clouds: 0.25,
                rain: 0.0,
                cycle: true,
            }),
        )];
        let config = water_surface_config_from_world(&world);
        assert!(config.drive);
        assert_eq!(config.clock_start, 1140.0);
        assert_eq!(config.max_elev, 55.0);
        assert_eq!(config.min_elev, 8.0);
        assert_eq!(config.wind, [1.0, 0.0]);
        assert_eq!(config.wind_strength, 3.0);
        let bare = water_surface_config_from_world(&[spec(EntityKind::Group)]);
        assert!(!bare.drive);
        assert_eq!(bare.wind_strength, 1.5);
    }
}
