//! Painel "Vael: Ambiente": hora, clima e sol de preview.
//!
//! O céu volumétrico, as nuvens e a chuva do `bevy_elements` só correm no
//! Play; no editor o painel mostra um sol direcional na posição astronómica
//! da hora/data/latitude da cena (estimativa de `bevy_ext_elements`).

use bevy::prelude::*;
use jackdaw_extension::prelude::*;
use jackdaw_extension::scene::EditorHidden;

use bevy_ext_elements::{ElementsSettings, StartWeather};

use crate::ui;

pub const WINDOW_ID: &str = "vael.environment";
const TYPE: &str = "bevy_ext_elements::ElementsSettings";

#[derive(Component)]
pub(crate) struct EnvStatus;

/// Sol de preview ligado/desligado.
#[derive(Resource)]
pub(crate) struct SunPreview {
    enabled: bool,
}

impl Default for SunPreview {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Component)]
struct SunPreviewLight;

pub(crate) fn app(app: &mut App) {
    app.init_resource::<SunPreview>().add_systems(
        Update,
        drive_sun_preview.run_if(in_state(jackdaw_extension::AppState::Editor)),
    );
}

pub(crate) fn build(world: &mut World, parent: Entity) {
    ui::column(world, parent, |root| {
        ui::heading(root, "Ambiente (bevy_elements)");
        ui::status(root, EnvStatus);
        ui::label(root, "Hora de arranque");
        ui::row(
            root,
            &[
                ("−1 h", "vael.env.hour_minus"),
                ("+1 h", "vael.env.hour_plus"),
                ("05:30", "vael.env.hour_dawn"),
                ("08:00", "vael.env.hour_morning"),
                ("12:00", "vael.env.hour_noon"),
                ("17:00", "vael.env.hour_afternoon"),
                ("20:00", "vael.env.hour_dusk"),
                ("23:00", "vael.env.hour_night"),
            ],
        );
        ui::label(root, "Clima de arranque");
        ui::row(
            root,
            &[
                ("Auto", "vael.env.weather_auto"),
                ("Limpo", "vael.env.weather_clear"),
                ("Nuvens", "vael.env.weather_partly_cloudy"),
                ("Encoberto", "vael.env.weather_overcast"),
                ("Nevoeiro", "vael.env.weather_fog"),
                ("Chuvisco", "vael.env.weather_drizzle"),
                ("Chuva", "vael.env.weather_rain"),
                ("Trovoada", "vael.env.weather_thunderstorm"),
                ("Neve", "vael.env.weather_snow"),
                ("Nevasca", "vael.env.weather_blizzard"),
                ("Poeira", "vael.env.weather_dust_storm"),
            ],
        );
        ui::row(root, &[("Sol de preview (liga/desliga)", "vael.env.toggle_sun")]);
        ui::label(
            root,
            "Em Definições › Ambiente, o inspector permite editar o perfil climático, \
             a semente da previsão, a latitude, a data, a escala do tempo e a qualidade. \
             Grava (Ctrl+S) para ver o resultado no Play.",
        );
    });
}

pub(crate) fn refresh(
    settings: Query<&ElementsSettings>,
    sun: Res<SunPreview>,
    mut status: Query<&mut Text, With<EnvStatus>>,
) {
    let Ok(mut status) = status.single_mut() else {
        return;
    };
    let text = match settings.iter().next() {
        None => "Sem ElementsSettings nesta cena (abre o world.bsn).".to_string(),
        Some(s) => {
            let hour = s.start_hour.rem_euclid(24.0);
            let elevation = s.sun_direction(hour).y.asin().to_degrees();
            let minutes = (hour * 60.0).round() as u32;
            format!(
                "{:02}:{:02} · {:02}/{:02}/{} · {:.1}°{} · clima {} · ×{:.0}\n\
                 Perfil {:?} · semente {} · Sol a {:.0}° de elevação · preview {}",
                (minutes / 60) % 24,
                minutes % 60,
                s.start_day,
                s.start_month,
                s.start_year,
                s.latitude_deg.abs(),
                if s.latitude_deg >= 0.0 { "N" } else { "S" },
                s.start_weather.label(),
                s.time_scale,
                s.climate,
                s.weather_seed,
                elevation,
                if sun.enabled { "ligado" } else { "desligado" },
            )
        }
    };
    if status.0 != text {
        status.0 = text;
    }
}

/// Luz direcional `EditorHidden` na posição do Sol da cena.
fn drive_sun_preview(
    mut commands: Commands,
    preview: Res<SunPreview>,
    settings: Query<&ElementsSettings>,
    mut lights: Query<(Entity, &mut Transform, &mut DirectionalLight), With<SunPreviewLight>>,
) {
    let settings = settings.iter().next();
    let Some(settings) = settings.filter(|_| preview.enabled) else {
        for (entity, ..) in &lights {
            commands.entity(entity).despawn();
        }
        return;
    };
    let dir = settings.sun_direction(settings.start_hour);
    let elevation = dir.y.asin().to_degrees();
    // Crepúsculo: a luz cai até 6° abaixo do horizonte e aquece.
    let day = ((elevation + 6.0) / 16.0).clamp(0.0, 1.0);
    let warm = (1.0 - (elevation / 25.0).clamp(0.0, 1.0)).powf(1.5);
    let color = Color::srgb(1.0, 1.0 - 0.25 * warm, 1.0 - 0.55 * warm);
    let illuminance = 400.0 + 9600.0 * day;
    let transform = Transform::from_translation(dir * 500.0).looking_at(Vec3::ZERO, Vec3::Y);

    if let Some((_, mut current, mut light)) = lights.iter_mut().next() {
        if *current != transform {
            *current = transform;
        }
        if light.illuminance != illuminance || light.color != color {
            light.illuminance = illuminance;
            light.color = color;
        }
        return;
    }
    commands.spawn((
        Name::new("Vael: sol de preview"),
        SunPreviewLight,
        EditorHidden,
        DirectionalLight {
            illuminance,
            color,
            shadow_maps_enabled: true,
            ..default()
        },
        transform,
    ));
}

fn set_hour(world: &mut World, hour: impl FnOnce(f64) -> f64) -> OperatorResult {
    let Some(entity) = ui::settings_entity::<ElementsSettings>(world) else {
        return ui::missing::<ElementsSettings>(world);
    };
    let current = world
        .get::<ElementsSettings>(entity)
        .map(|s| s.start_hour)
        .unwrap_or(8.0);
    let next = hour(current).rem_euclid(24.0);
    ui::set_field(world, entity, TYPE, "start_hour", format!("{next}"))
}

fn set_weather(world: &mut World, weather: StartWeather) -> OperatorResult {
    let Some(entity) = ui::settings_entity::<ElementsSettings>(world) else {
        return ui::missing::<ElementsSettings>(world);
    };
    ui::set_field(world, entity, TYPE, "start_weather", format!("\"{weather:?}\""))
}

macro_rules! hour_op {
    ($name:ident, $id:tt, $label:tt, $f:expr) => {
        #[operator(id = $id, label = $label, allows_undo = false)]
        pub(crate) fn $name(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
            set_hour(world, $f)
        }
    };
}

macro_rules! weather_op {
    ($name:ident, $id:tt, $label:tt, $weather:expr) => {
        #[operator(id = $id, label = $label, allows_undo = false)]
        pub(crate) fn $name(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
            set_weather(world, $weather)
        }
    };
}

hour_op!(hour_minus, "vael.env.hour_minus", "Ambiente: −1 hora", |h| h - 1.0);
hour_op!(hour_plus, "vael.env.hour_plus", "Ambiente: +1 hora", |h| h + 1.0);
hour_op!(hour_dawn, "vael.env.hour_dawn", "Ambiente: madrugada", |_| 5.5);
hour_op!(hour_morning, "vael.env.hour_morning", "Ambiente: manhã", |_| 8.0);
hour_op!(hour_noon, "vael.env.hour_noon", "Ambiente: meio-dia", |_| 12.0);
hour_op!(hour_afternoon, "vael.env.hour_afternoon", "Ambiente: tarde", |_| 17.0);
hour_op!(hour_dusk, "vael.env.hour_dusk", "Ambiente: crepúsculo", |_| 20.0);
hour_op!(hour_night, "vael.env.hour_night", "Ambiente: noite", |_| 23.0);

weather_op!(weather_auto, "vael.env.weather_auto", "Clima: auto", StartWeather::Auto);
weather_op!(weather_clear, "vael.env.weather_clear", "Clima: limpo", StartWeather::Clear);
weather_op!(
    weather_partly_cloudy,
    "vael.env.weather_partly_cloudy",
    "Clima: nuvens",
    StartWeather::PartlyCloudy
);
weather_op!(weather_overcast, "vael.env.weather_overcast", "Clima: encoberto", StartWeather::Overcast);
weather_op!(weather_fog, "vael.env.weather_fog", "Clima: nevoeiro", StartWeather::Fog);
weather_op!(weather_drizzle, "vael.env.weather_drizzle", "Clima: chuvisco", StartWeather::Drizzle);
weather_op!(weather_rain, "vael.env.weather_rain", "Clima: chuva", StartWeather::Rain);
weather_op!(
    weather_thunderstorm,
    "vael.env.weather_thunderstorm",
    "Clima: trovoada",
    StartWeather::Thunderstorm
);
weather_op!(weather_snow, "vael.env.weather_snow", "Clima: neve", StartWeather::Snow);
weather_op!(weather_blizzard, "vael.env.weather_blizzard", "Clima: nevasca", StartWeather::Blizzard);
weather_op!(
    weather_dust_storm,
    "vael.env.weather_dust_storm",
    "Clima: poeira",
    StartWeather::DustStorm
);

#[operator(
    id = "vael.env.toggle_sun",
    label = "Ambiente: sol de preview",
    allows_undo = false
)]
pub(crate) fn toggle_sun(_params: In<OperatorParameters>, mut sun: ResMut<SunPreview>) -> OperatorResult {
    sun.enabled = !sun.enabled;
    OperatorResult::Finished
}

pub(crate) fn register(ctx: &mut ExtensionRegistrar<'_>) {
    ctx.register_operator::<HourMinusOp>()
        .register_operator::<HourPlusOp>()
        .register_operator::<HourDawnOp>()
        .register_operator::<HourMorningOp>()
        .register_operator::<HourNoonOp>()
        .register_operator::<HourAfternoonOp>()
        .register_operator::<HourDuskOp>()
        .register_operator::<HourNightOp>()
        .register_operator::<WeatherAutoOp>()
        .register_operator::<WeatherClearOp>()
        .register_operator::<WeatherPartlyCloudyOp>()
        .register_operator::<WeatherOvercastOp>()
        .register_operator::<WeatherFogOp>()
        .register_operator::<WeatherDrizzleOp>()
        .register_operator::<WeatherRainOp>()
        .register_operator::<WeatherThunderstormOp>()
        .register_operator::<WeatherSnowOp>()
        .register_operator::<WeatherBlizzardOp>()
        .register_operator::<WeatherDustStormOp>()
        .register_operator::<ToggleSunOp>();
}
