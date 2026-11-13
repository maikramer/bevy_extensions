//! Drivers do mundo extraídos do `worldsys` do monólito: relógio
//! (`daycycle_drive`), sol/lua (`sun_drive`), clima (`weather_drive`),
//! atmosfera (`atmosphere_drive`) e interior (`interior_lighting_*`).
//!
//! Os tipos de contrato de sala/fronteira (`WorldBorderConfig`,
//! `InteriorSceneConfig`, `InteriorLighting`, `INTERIOR_FLOOR_Y`) já vivem no
//! `bevy_ext_core` e são reexportados aqui. Os tipos de estado dos drivers
//! (`DayCycleState`, `WeatherState`, `AtmosphereState`, `SunState`) migraram
//! COM os sistemas: são a sua superfície de leitura/escrita. Os tipos de
//! bioma (`BiomeRegionData`/`BiomeRegions`) continuam no monólito durante a
//! transição (o scene loader é quem os insere); o mesmo para
//! `TerrainRuntime`.
//!
//! TypePath históricos preservados com `#[type_path = "viber::worldsys"]`.

use bevy::prelude::*;
pub use bevy_ext_core::{
    INTERIOR_CAMERA_DISTANCE, INTERIOR_CAMERA_PITCH_DEG, INTERIOR_FLOOR_Y, InteriorLighting,
    InteriorSceneConfig, WorldBorderConfig,
};
// RNG determinístico da engine ("mesma seed, mesmo mundo") — o mesmo
// SplitMix64 que o `spawner` do monólito reexporta do core.
use bevy_ext_core::rng::Rng;
use bevy_ext_core::Player as PlayerMarker;
// Amostragem de terreno (runtime de terreno), biomas com TypePath congelado
// no postfx, o mesmo `point_in_polygon` e o ganho do IBL (neste crate).
use bevy_ext_postfx::env::point_in_polygon;
use bevy_ext_terrain_runtime::runtime::TerrainRuntime;
use crate::ibl::IBL_INTENSITY;
pub use bevy_ext_postfx::env::BiomeRegions;

/// Como o `<DayCycle>` conduz o VISUAL (sol, céu, névoa, grade).
///
/// O relógio (`minute_of_day`) avança sempre; este modo decide apenas o
/// minuto que alimenta a atmosfera, a luz direcional, o IBL e o tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
#[type_path = "viber::worldsys"]
pub enum DayCycleMode {
    /// O sol percorre o arco em tempo real (comportamento histórico).
    #[default]
    Continuous,
    /// Dois estados fixos (dia/noite). O visual salta de âncora para âncora
    /// numa transição curta (`transition-seconds`) quando o relógio cruza o
    /// limiar de dia/noite; entre transições o sol NÃO se move.
    DayNight,
}

/// Âncoras do modo [`DayCycleMode::DayNight`].
#[derive(Debug, Clone, Copy, PartialEq, Reflect)]
#[type_path = "viber::worldsys"]
pub struct DayNightConfig {
    /// Minuto visual do estado de DIA (default 12:00).
    pub day_minute: f32,
    /// Minuto visual do estado de NOITE (default 22:00).
    pub night_minute: f32,
    /// Duração da travessia dia↔noite em segundos reais (0 = corte seco).
    pub transition_seconds: f32,
}

impl Default for DayNightConfig {
    fn default() -> Self {
        Self {
            day_minute: 720.0,
            night_minute: 1320.0,
            transition_seconds: 4.0,
        }
    }
}

const DAY_MINUTES: f32 = 24.0 * 60.0;

/// `<DayCycle>` clock: advances minute-of-day and ramps the ambient light.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct DayCycleState {
    pub minute_of_day: f32,
    pub minutes_per_real_second: f32,
    pub dawn_minute: f32,
    pub dusk_minute: f32,
    pub ambient_day: f32,
    pub ambient_night: f32,
    /// Ambient brightness the world authored (`<AmbientLight brightness>`),
    /// used as the full-day anchor for the day/night ramp. `0` until the
    /// first drive tick captures it.
    pub ambient_reference: f32,
    pub drive_ambient: bool,
    /// Elevação máxima do sol ao meio-dia (graus).
    pub max_sun_elevation: f32,
    /// Azimute do nascer do sol (graus).
    pub sun_azimuth_base: f32,
    /// Piso da elevação usada pela LUZ direcional (`min-sun-elevation`) —
    /// o sol astronómico continua a descer; só a luz não segue abaixo.
    pub min_sun_elevation: f32,
    /// Multiplicador do relógio durante o DIA (`day-speed-scale`). `<1`
    /// alonga o dia em tempo real; `1` = `minutes-per-real-second` puro.
    pub day_speed_scale: f32,
    /// Multiplicador do relógio durante a NOITE (`night-speed-scale`).
    /// `>1` encurta a noite em tempo real.
    pub night_speed_scale: f32,
    /// Minuto do dia em que o **visual** da noite CONGELA
    /// (`night-cap-minute`, 0 = desligado). O relógio continua a andar para
    /// quests/HUD; sol, céu, névoa, ambiente e luzes usam este minuto durante
    /// a noite. Era o pedido "às 20:00 já é a escuridão máxima, não continua".
    /// Ignorado no modo [`DayCycleMode::DayNight`] (as âncoras mandam).
    pub night_cap_minute: f32,
    /// `continuous` (default) ou `day-night`.
    pub mode: DayCycleMode,
    /// Âncoras do modo `day-night`.
    pub day_night: DayNightConfig,
    /// Minuto visual em curso. Em `continuous` é o `visual_minute()` de
    /// sempre; em `day-night` fica parado numa âncora e só atravessa na
    /// transição.
    pub visual_now: f32,
    /// Âncora de onde a transição actual partiu.
    pub visual_from: f32,
    /// Progresso da transição `0..1` (`1` = assentado numa âncora).
    pub visual_transition: f32,
    /// `true` = o estado visual actual é o DIA (âncora `day_minute`).
    pub visual_day: bool,
    /// Cadência de actualização das sombras do sol no modo `day-night`
    /// (`shadow-refresh-hz`, 0 = todos os frames). O cache é implementado
    /// pelo jogo (Vael), não pela engine: o `DayCycleState` só publica o
    /// pedido.
    pub shadow_refresh_hz: f32,
}

impl DayCycleState {
    /// Minuto usado pela ATMOSFERA/SOL/AMBIENTE. Durante a noite (depois do
    /// cap ou antes do amanhecer) devolve o `night-cap-minute`; de dia
    /// devolve o relógio real. O `minute_of_day` continua a avançar.
    ///
    /// No modo `day-night` devolve o minuto visual da âncora/transição — que
    /// fica PARADO fora das transições.
    pub fn visual_minute(&self) -> f32 {
        if self.is_day_night() {
            return self.visual_now;
        }
        let cap = self.night_cap_minute;
        if cap <= 0.0 || cap >= DAY_MINUTES {
            return self.minute_of_day;
        }
        let m = self.minute_of_day;
        if m >= cap || m < self.dawn_minute {
            cap
        } else {
            m
        }
    }

    /// `true` no modo de dois estados.
    pub fn is_day_night(&self) -> bool {
        self.mode == DayCycleMode::DayNight
    }

    /// Factor dia/noite do minuto VISUAL (`0` noite, `1` dia). É a curva que
    /// o ambiente, o tint dos props, a relva, o terreno e o orçamento de
    /// sombras devem ler — em `continuous` coincide com a de antes; em
    /// `day-night` fica parada nas âncoras e acompanha a transição.
    pub fn visual_daylight(&self) -> f32 {
        daylight_factor(self.visual_minute(), self.dawn_minute, self.dusk_minute)
    }

    /// Liga o modo de dois estados e assenta já na âncora do relógio actual
    /// (o arranque não faz uma travessia de 12:00 para 22:00).
    pub fn with_day_night(mut self, config: DayNightConfig) -> Self {
        self.mode = DayCycleMode::DayNight;
        self.day_night = config;
        self.snap_visual();
        self
    }

    /// Asenta o visual na âncora do estado actual.
    fn snap_visual(&mut self) {
        // O limiar é estrito: no minuto exacto do crepúsculo (factor 0.5) o
        // visual já é noite. Com `>=` a transição dia→noite nunca arrancava
        // no minuto autorado do `dusk`.
        self.visual_day =
            daylight_factor(self.minute_of_day, self.dawn_minute, self.dusk_minute) > 0.5;
        self.visual_from = if self.visual_day {
            self.day_night.day_minute
        } else {
            self.day_night.night_minute
        }
        .rem_euclid(DAY_MINUTES);
        self.visual_now = self.visual_from;
        self.visual_transition = 1.0;
    }

    /// Avança a travessia dia↔noite (só no modo `day-night`).
    ///
    /// O sentido é sempre o do RELÓGIO: dia 12:00 → noite 22:00 passa pelo
    /// pôr-do-sol; noite 22:00 → dia 12:00 passa pela meia-noite e pelo
    /// nascer do sol. É um "avanço rápido" de alguns segundos, não um corte.
    /// Uma travessia em curso fica latched até completar; um `set_clock` que
    /// mude de fase a meio só é aplicado no fim, não a re-targeta.
    pub fn advance_visual(&mut self, dt: f32) {
        if !self.is_day_night() {
            return;
        }
        // O limiar 0.5 é o mesmo no crepúsculo e no amanhecer. A avaliação só
        // corre com a travessia ASSENTADA: a meio, o alvo fica latched e o
        // `daylight` oscilante do minuto exacto não faz o visual saltar de
        // noite para dia outra vez (20:00 → 05:00 no mesmo frame).
        if self.visual_transition >= 1.0 {
            let daylight = daylight_factor(self.minute_of_day, self.dawn_minute, self.dusk_minute);
            // Hysteresis: quem está em dia só troca quando desce abaixo de
            // 0.5; quem está em noite só troca quando chega a 0.5.
            let want_day = if self.visual_day {
                daylight > 0.5
            } else {
                daylight >= 0.5
            };
            if want_day != self.visual_day {
                // Nova travessia a partir de onde o visual está agora.
                self.visual_day = want_day;
                self.visual_from = self.visual_now;
                self.visual_transition = 0.0;
            }
        }
        let target = if self.visual_day {
            self.day_night.day_minute
        } else {
            self.day_night.night_minute
        }
        .rem_euclid(DAY_MINUTES);
        if self.visual_transition >= 1.0 {
            // Assentado: mantém a âncora (uma edição de `day-minute`/
            // `night-minute` no editor reflecte-se sem esperar pelo relógio).
            self.visual_now = target;
            return;
        }
        let seconds = self.day_night.transition_seconds;
        if seconds <= f32::EPSILON {
            self.visual_now = target;
            self.visual_transition = 1.0;
            return;
        }
        self.visual_transition = (self.visual_transition + dt / seconds).clamp(0.0, 1.0);
        let t = smooth_step(0.0, 1.0, self.visual_transition);
        let delta = (target - self.visual_from).rem_euclid(DAY_MINUTES);
        self.visual_now = (self.visual_from + delta * t).rem_euclid(DAY_MINUTES);
        if self.visual_transition >= 1.0 {
            self.visual_now = target;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        minute_of_day: f32,
        minutes_per_real_second: f32,
        dawn_minute: f32,
        dusk_minute: f32,
        ambient_day: f32,
        ambient_night: f32,
        drive_ambient: bool,
        max_sun_elevation: f32,
        sun_azimuth_base: f32,
        min_sun_elevation: f32,
        day_speed_scale: f32,
        night_speed_scale: f32,
        night_cap_minute: f32,
    ) -> Self {
        Self {
            minute_of_day,
            minutes_per_real_second,
            dawn_minute,
            dusk_minute,
            ambient_day,
            ambient_night,
            // Captured from the live ambient light on the first drive tick.
            ambient_reference: 0.0,
            drive_ambient,
            max_sun_elevation,
            sun_azimuth_base,
            min_sun_elevation,
            day_speed_scale,
            night_speed_scale,
            night_cap_minute,
            mode: DayCycleMode::Continuous,
            day_night: DayNightConfig::default(),
            visual_now: minute_of_day,
            visual_from: minute_of_day,
            visual_transition: 1.0,
            visual_day: daylight_factor(minute_of_day, dawn_minute, dusk_minute) > 0.5,
            shadow_refresh_hz: 0.0,
        }
    }
}

/// Daylight factor `0.0` (night) → `1.0` (day) with 60-minute dawn/dusk ramps.
///
/// Circular over the 24 h clock: a dawn at 00:30 starts its ramp at 23:30
/// and a dusk at 23:40 keeps fading past midnight (the linear version jumped
/// to half light at 00:00).
pub fn daylight_factor(minute: f32, dawn: f32, dusk: f32) -> f32 {
    const RAMP: f32 = 60.0;
    const DAY: f32 = 24.0 * 60.0;
    let day_len = dusk - dawn;
    if day_len >= DAY {
        return 1.0;
    }
    let day_len = day_len.max(0.0);
    let since_dawn = (minute - dawn).rem_euclid(DAY);
    if since_dawn < day_len {
        return 1.0;
    }
    let fade_out = 1.0 - (since_dawn - day_len) / RAMP;
    let fade_in = 1.0 - (DAY - since_dawn) / RAMP;
    fade_out.max(fade_in).clamp(0.0, 1.0)
}
/// `<Weather>` wind/cloud/rain config.
///
/// `rain` é INTENSIDADE CONTÍNUA 0..1 (não bool): 0 = seco, 1 = tempestade.
/// Com `cycle` a falso é estática como autorada (backwards compat); com
/// `cycle` a verdadeiro o [`weather_drive`] rola novos alvos e faz o lerp
/// de [`WEATHER_TRANSITION_SECS`].
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct WeatherState {
    pub wind: [f32; 2],
    pub wind_strength: f32,
    pub clouds: f32,
    pub rain: f32,
    pub cycle: bool,
}

/// Período do ciclo de weather (`<Weather cycle>`) em segundos.
///
/// NOTA de contrato (WS-A): o attr `cycle` é parseado como BOOL no parser
/// (`recipes::finish_weather`, ficheiro de outro workstream), pelo que
/// "cycle em segundos" não é autorável — `cycle="1"` liga o ciclo com ESTE
/// período; `cycle="0"` mantém a chuva estática como declarada.
pub const WEATHER_CYCLE_DEFAULT_SECS: f32 = 240.0;
/// Duração do lerp entre o alvo de chuva antigo e o novo (s).
pub const WEATHER_TRANSITION_SECS: f32 = 10.0;

/// Estado do scheduler do `<Weather cycle>` — criado preguiçosamente pelo
/// [`weather_drive`] quando o mundo declara o ciclo.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct WeatherScheduler {
    /// Seed do mundo (a do `<Terrain>`, 0 sem terreno) — "mesma seed, mesmo
    /// mundo" é lei: o k-ésimo ciclo produz SEMPRE o mesmo alvo.
    pub seed: u64,
    /// Índice do ciclo corrente (0 = o estado autoral; os rollpoints são 1, 2…).
    pub index: u64,
    pub period: f32,
    pub timer: f32,
    /// Intensidade de chuva ALVO do ciclo corrente (0..1).
    pub target: f32,
}

impl WeatherScheduler {
    pub fn new(seed: u64, initial_rain: f32, period: f32) -> Self {
        let period = if period > 0.0 {
            period
        } else {
            WEATHER_CYCLE_DEFAULT_SECS
        };
        Self {
            seed,
            index: 0,
            period,
            timer: period,
            target: initial_rain.clamp(0.0, 1.0),
        }
    }

    /// Avança `dt` e devolve o alvo corrente (rolando um novo quando o timer
    /// esgota). Função pura sobre `&mut self` para teste.
    pub fn tick(&mut self, dt: f32) -> f32 {
        self.timer -= dt;
        if self.timer <= 0.0 {
            self.timer += self.period;
            self.index += 1;
            self.target = weather_cycle_target(self.seed, self.index);
        }
        self.target
    }
}

/// Alvo de intensidade de chuva do `index`-ésimo ciclo — SplitMix64 (o RNG
/// determinístico do spawner) semeado por `seed XOR índice·golden`: a mesma
/// seed+índice dá SEMPRE o mesmo alvo, mundos diferentes divergem.
///
/// O quadrado enviesa para tempo mais seco do que chuvoso (média ~0.33; a
/// raiz daria o oposto, ~0.67) mas preserva tempestades ocasionais (r → 1
/// só quando o uniforme chega perto de 1).
pub fn weather_cycle_target(seed: u64, index: u64) -> f32 {
    let mut rng = Rng::new((seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1);
    let u = rng.next_f32();
    u * u
}

/// Aproxima `current` de `target` à velocidade CONSTANTE `1/transition` por
/// segundo — nunca ultrapassa o alvo, nunca regride ao secar (monótono).
pub fn rain_toward(current: f32, target: f32, dt: f32, transition: f32) -> f32 {
    let current = current.clamp(0.0, 1.0);
    let delta = target.clamp(0.0, 1.0) - current;
    if delta.abs() <= f32::EPSILON || dt <= 0.0 {
        return current;
    }
    let step = (dt / transition.max(f32::EPSILON)).min(delta.abs());
    current + delta.signum() * step
}

/// Roda o scheduler e escreve a intensidade contínua em `WeatherState.rain`.
///
/// Mundos SEM `<Weather cycle>` (ou `cycle="0"`) nunca chegam aqui: chuva
/// estática como autorada. Corre ANTES de [`atmosphere_drive`] (mesmo frame).
#[allow(clippy::needless_pass_by_value)]
pub fn weather_drive(
    time: Res<Time>,
    mut commands: Commands,
    weather: Option<ResMut<WeatherState>>,
    scheduler: Option<ResMut<WeatherScheduler>>,
    runtime: Option<Res<TerrainRuntime>>,
    regions: Option<Res<BiomeRegions>>,
    players: Query<&GlobalTransform, With<PlayerMarker>>,
) {
    let Some(mut weather) = weather else {
        return;
    };
    if !weather.cycle {
        return;
    }
    let dt = time.delta_secs().clamp(0.0, 0.5);
    let Some(mut scheduler) = scheduler else {
        // 1.º tick com ciclo: semeia pelo mundo (seed do terreno; 0 sem ele)
        // e parte do estado autoral — sem salto no frame de arranque.
        let seed = runtime.as_ref().map(|r| r.spec.seed).unwrap_or(0);
        commands.insert_resource(WeatherScheduler::new(seed, weather.rain, 0.0));
        return;
    };
    let target = scheduler.tick(dt);
    // `rain-scale` do bioma onde o herói está: o deserto seca o céu dele sem
    // mexer no ciclo do mundo (o scheduler continua determinístico).
    let scale = players
        .single()
        .ok()
        .and_then(|p| {
            let t = p.translation();
            rain_scale_at(regions.as_deref(), t.x, t.z)
        })
        .unwrap_or(1.0);
    weather.rain = rain_toward(weather.rain, target * scale, dt, WEATHER_TRANSITION_SECS);
}

/// `rain-scale` da primeira `<BiomeRegion>` que contém `(x, z)` e o declara
/// (`None` = sem região ou sem fator — a chuva do mundo passa inteira).
pub fn rain_scale_at(regions: Option<&BiomeRegions>, x: f32, z: f32) -> Option<f32> {
    regions?
        .list
        .iter()
        .filter(|r| r.rain_scale.is_some())
        .find(|r| point_in_polygon(x, z, &r.polygon))
        .and_then(|r| r.rain_scale)
}
/// Intensidade do env map (IBL) dentro de um interior.
///
/// O cubemap do céu é a razão pela qual a sala ficava AZUL à noite mesmo com
/// o ambiente no máximo: é ele que pinta a radiância do céu nas paredes. Numa
/// divisão, o céu não entra — fica um resto mínimo para o PBR não perder o
/// especular por completo.
pub const INTERIOR_IBL_INTENSITY: f32 = 0.05;

/// Aplica o regime de luz de interior DEPOIS dos drivers do mundo: sol a
/// zero (a sala vive do ambiente, como o utilizador pediu) e env map do céu
/// quase apagado. Ao sair, o IBL volta ao valor do mundo — o sol é reescrito
/// pelo [`sun_drive`] no frame seguinte, não precisa de restauro.
pub fn interior_lighting_apply(
    state: Res<InteriorLighting>,
    mut suns: Query<(Entity, &mut bevy::light::DirectionalLight)>,
    mut probes: Query<&mut GeneratedEnvironmentMapLight>,
    mut was_active: bevy::ecs::system::Local<bool>,
    mut saved_suns: bevy::ecs::system::Local<Vec<(Entity, f32)>>,
) {
    if state.active {
        if !*was_active {
            saved_suns.clear();
        }
        for (entity, mut sun) in &mut suns {
            // Guarda a lux autoral UMA vez para restaurar à saída. O comentário
            // assumia que o `sun_drive` a reescrevia, mas sem `<DayCycle>`
            // ele retorna cedo e o sol ficava a zero para sempre.
            if !saved_suns.iter().any(|(saved, _)| *saved == entity) {
                saved_suns.push((entity, sun.illuminance));
            }
            sun.illuminance = 0.0;
        }
        for mut probe in &mut probes {
            probe.intensity = INTERIOR_IBL_INTENSITY;
        }
        *was_active = true;
    } else if *was_active {
        for (entity, lux) in saved_suns.drain(..) {
            if let Ok((_, mut sun)) = suns.get_mut(entity) {
                sun.illuminance = lux;
            }
        }
        for mut probe in &mut probes {
            probe.intensity = IBL_INTENSITY;
        }
        *was_active = false;
    }
}

/// Elevação de sol EQUIVALENTE usada para a paleta dentro de um interior.
///
/// Não é o sol da sala (esse está a zero): é a hora que o grading, a
/// exposição e a névoa devem assumir lá dentro — meio-dia alto, neutro.
pub const INTERIOR_SUN_ELEVATION_DEG: f32 = 45.0;

/// Cota do soalho das salas (m) — o piso DURO da bolsa de interior.
///
/// As salas são ilhas sem terreno por baixo: sem este piso, sair do soalho
/// (pela porta, por um vão) era cair para sempre. A laje do gerador está a
/// 0,12 e o herói assenta a 0,6.

/// Brilho do ambiente de interior (lux) — a sala inteira legível sem sol.
///
/// A escala é a do SOL da cena (28 000 lux no `simple-rpg`), não a do
/// ambiente autoral (~110): medido a 620 lux a capela à noite lia 45/255
/// (contra 200 de dia) — continuava a ser noite lá dentro. A 2 400 a sala
/// fica plana e legível, que é o que a câmara de sala pede — e a escala tem
/// de ser a do SOL (28 000 lux) porque a exposição automática NÃO levanta a
/// noite (o teto `postfx::NIGHT_LIFT_CAP_EV` existe para o frame não ficar
/// branco): com 2 400 lux a sala continuava a 61/255.
pub const INTERIOR_AMBIENT_BRIGHTNESS: f32 = 2000.0;

/// Ambiente de interior efetivo (`VIBER_INTERIOR_AMBIENT`, para afinar sem
/// recompilar).
pub fn interior_ambient_brightness() -> f32 {
    std::env::var("VIBER_INTERIOR_AMBIENT")
        .ok()
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(INTERIOR_AMBIENT_BRIGHTNESS)
}
/// Cor do ambiente de interior: neutra e ligeiramente quente (pedra + velas).
pub const INTERIOR_AMBIENT_COLOR: [f32; 3] = [1.0, 0.96, 0.90];

/// Marca [`InteriorLighting`] a cada frame pela posição do herói.
pub fn interior_lighting_drive(
    mut state: ResMut<InteriorLighting>,
    scene: Option<Res<InteriorSceneConfig>>,
    players: Query<&bevy::prelude::GlobalTransform, With<PlayerMarker>>,
) {
    let inside = match (scene.as_deref(), players.iter().next()) {
        (Some(scene), Some(player)) => {
            let p = player.translation();
            scene.contains(p.x, p.z)
        }
        _ => false,
    };
    state.active = inside;
}
/// Iluminância da luz direcional à noite, como fração da que o mundo
/// autorou (`<DirectionalLight illuminance>`): a **lua**. Baixa o bastante
/// para uma tocha valer a pena, alta o bastante para o chão continuar a ler-se.
pub const MOONLIGHT_RATIO: f32 = 0.06;
/// Elevação mínima (graus) da luz direcional.
///
/// O sol real desce a −25° à noite; apontar a luz direcional para lá
/// iluminava a cena POR BAIXO — o chão (normal +Y) apagava por completo
/// enquanto paredes e props ainda apanhavam luz de raspão, e o especular
/// varria a imagem conforme a câmara rodava ("mais escuro/mais colorido
/// consoante o ângulo"). A luz nunca passa abaixo deste ângulo.
pub const MIN_LIGHT_ELEVATION_DEG: f32 = 8.0;
/// Elevação da lua (graus) — alta, para banhar o chão em vez de o rasar.
pub const MOON_ELEVATION_DEG: f32 = 52.0;
/// Cor linear da luz da lua (azul frio).
pub const MOON_COLOR: [f32; 3] = [0.40, 0.54, 0.86];

/// Iluminância/cor que o mundo autorou para uma `DirectionalLight`, capturada
/// antes de [`sun_drive`] começar a escrevê-las.
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::worldsys"]
pub struct SunLightBase {
    pub illuminance: f32,
    pub color: Color,
}

/// Posição do sol (e estado dia/noite) calculada a partir do relógio.
#[derive(Debug, Clone, Copy, Resource, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct SunState {
    /// Direção PARA o sol, normalizada (mundo, Y-up).
    pub dir: Vec3,
    pub elevation_deg: f32,
    /// 0 = dia pleno, 1 = noite plena.
    pub night: f32,
}

/// Elevação (graus) do sol para o minuto do dia: sobe `max_elevation` ao
/// meio-dia e desce abaixo do horizonte à noite.
///
/// dawn/dusk degenerados não podem produzir NaN: `dawn==dusk` zera o arco
/// diurno e `dawn=0`/`dusk=1440` zera o noturno — ambos dividiam por 0 e o
/// NaN propagava-se ao sol, à luz direcional e ao ambiente. 1 min é o menor
/// arco com sentido; para além do clamp a semântica é a mesma.
pub fn sun_elevation(minute: f32, dawn: f32, dusk: f32, max_elevation: f32) -> f32 {
    const NIGHT_HALF_ARC: f32 = 25.0;
    let day_len = (dusk - dawn).max(1.0);
    if minute >= dawn && minute < dusk {
        let t = (minute - dawn) / day_len;
        (std::f32::consts::PI * t).sin() * max_elevation
    } else {
        let dusk_len = (24.0 * 60.0 - dusk + dawn).max(1.0);
        let t = if minute >= dusk {
            (minute - dusk) / dusk_len
        } else {
            (minute + (24.0 * 60.0 - dusk)) / dusk_len
        };
        -(std::f32::consts::PI * t).sin().abs() * NIGHT_HALF_ARC
    }
}

/// Azimute (graus) do sol: avança 180° durante o dia, 180° durante a noite.
/// Mesmos clamps de arco do [`sun_elevation`] — dawn/dusk degenerados nunca
/// dividem por 0.
pub fn sun_azimuth(minute: f32, dawn: f32, dusk: f32, base: f32) -> f32 {
    let day_len = (dusk - dawn).max(1.0);
    if minute >= dawn && minute < dusk {
        base + 180.0 * (minute - dawn) / day_len
    } else {
        let dusk_len = (24.0 * 60.0 - dusk + dawn).max(1.0);
        let t = if minute >= dusk {
            (minute - dusk) / dusk_len
        } else {
            (minute + (24.0 * 60.0 - dusk)) / dusk_len
        };
        base + 180.0 + 180.0 * t
    }
}

/// Advance the day clock and ramp `GlobalAmbientLight.brightness`.
///
/// O ambiente leva um FILL dourado/noturno: o sol da cena vale ~10k lux e o
/// `AmbientLight` autoral ~110 — a razão 1:90 esmaga qualquer sombra em
/// silhueta preta (o gap "primeiro plano em preto puro" do golden hour).
/// O fill usa a elevação do sol ([`SunState`], um frame velho — é suave) e
/// replica a curva golden de [`atmosphere_for_elevation`] para que céu,
/// névoa e ambiente concordem sobre que horas são.
#[allow(clippy::needless_pass_by_value)]
pub fn daycycle_drive(
    time: Res<Time>,
    clock: Option<ResMut<DayCycleState>>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
    sun: Res<SunState>,
    interior: Option<Res<InteriorLighting>>,
    mut ambient_color_ref: Local<[f32; 3]>,
    mut ambient_color_captured: Local<bool>,
) {
    let (Some(mut clock), Some(mut ambient)) = (clock, ambient) else {
        return;
    };
    // minutes_per_real_second é MINUTOS de jogo por segundo real (default
    // 1.2 → dia de 20 min): o /60 antigo tratava-o como fração de segundo
    // e o dia durava ~20 h — o dusk autoral nunca chegava numa sessão.
    // `day-speed-scale` / `night-speed-scale` (XML) multiplicam a velocidade
    // por fase: 0.5 faz o dia durar o DOBRO em tempo real e 2.0 faz a noite
    // durar METADE. O corte é a meio da rampa dawn/dusk (daylight 0.5), com
    // um passo de VELOCIDADE (não de posição) — não há salto no relógio.
    // rem_euclid: um relógio a andar para trás (velocidade negativa) ficava
    // com minutos negativos e o sol/rampas liam noite eterna.
    let daylight = daylight_factor(clock.minute_of_day, clock.dawn_minute, clock.dusk_minute);
    let speed_scale = if daylight >= 0.5 {
        clock.day_speed_scale
    } else {
        clock.night_speed_scale
    };
    clock.minute_of_day = (clock.minute_of_day
        + clock.minutes_per_real_second * speed_scale.max(0.0) * time.delta_secs())
    .rem_euclid(DAY_MINUTES);
    // Modo de dois estados: avança a travessia dia↔noite ANTES de o resto do
    // frame ler `visual_minute()` (sol, atmosfera, ambiente, IBL). Sem isto a
    // transição ficava um frame atrás e o `drive_ambient=false` saltava-a.
    clock.advance_visual(time.delta_secs());
    if !clock.drive_ambient {
        return;
    }
    // A cor AUTORAL do ambiente é capturada antes de qualquer escrita —
    // inclusive antes da saída antecipada do interior. Se a primeira frame
    // já for dentro de uma sala, a referência ficava a cor do interior e o
    // lerp noturno partia dela para sempre.
    if !*ambient_color_captured {
        let c = ambient.color.to_srgba();
        *ambient_color_ref = [c.red, c.green, c.blue];
        *ambient_color_captured = true;
    }
    // Dentro de um interior o relógio continua a andar (as quests e o mundo lá
    // fora dependem dele), mas a LUZ da sala não: ambiente fixo, sem noite.
    if interior.is_some_and(|state| state.active) {
        if clock.ambient_reference <= 0.0 {
            clock.ambient_reference = ambient.brightness.max(1.0);
        }
        ambient.brightness = interior_ambient_brightness();
        ambient.color = Color::srgb(
            INTERIOR_AMBIENT_COLOR[0],
            INTERIOR_AMBIENT_COLOR[1],
            INTERIOR_AMBIENT_COLOR[2],
        );
        return;
    }
    // Capture the world's own ambient before this system starts writing it.
    if clock.ambient_reference <= 0.0 {
        clock.ambient_reference = ambient.brightness.max(1.0);
    }
    let day = clock.visual_daylight();
    // `ambient-day-intensity` / `ambient-night-intensity` are the legacy
    // three.js ambient *intensities* — a 0..1 scale. Bevy's
    // `AmbientLight::brightness` is in lux and the same worlds author it in the
    // hundreds (`simple-rpg`: `brightness="110"`). Writing 0.26 straight into
    // it dropped the ambient by ~400x and left the whole village in the dark.
    //
    // So the pair is used as a day/night *ratio* against the brightness the
    // world authored: full day keeps that value, night falls to
    // `night / day` of it.
    let scale = if clock.ambient_day > f32::EPSILON {
        (day * clock.ambient_day + (1.0 - day) * clock.ambient_night) / clock.ambient_day
    } else {
        day
    };
    // Fill de sombras: o sol da cena é 10k lux e o ambiente autoral ~110 —
    // sem isto a sombra é preto puro. Golden hour mantém o ×2.2 de pico
    // (2.8 no peso, dia não mexe). À noite o peso desce de 1.6 para 1.0
    // (×2.0 sobre o rácio night/day): com a lanterna do viajante a 60k lm e
    // as tochas ×8, o fill alto só servia para lavar o herói e as paredes
    // num véu azul-claro (medido 2026-10-21: chão 0,44 às 22:00). O clamp a
    // 3 impede o fill de lavar o dia.
    let night = 1.0 - (sun.elevation_deg / 6.0).clamp(0.0, 1.0);
    let golden = (-(((sun.elevation_deg - 4.0) / 8.0).powi(2))).exp() * (1.0 - night);
    let fill = 2.8 * golden + 1.0 * night;
    ambient.brightness = clock.ambient_reference * (scale * (1.0 + fill)).clamp(0.0, 3.0);
    // Cor do ambiente: esfria para azul-profundo à noite (a cor autoral é
    // capturada acima, UMA vez — flag explícita: a sentinela `== 0.0`
    // recapturava para sempre quando a cor autoral ERA preta/azul-pura).
    // Golden entra no lerp com peso leve: o split quente/frio existe, mas a
    // base da muralha em sombra não pode ficar turva (r13 — transição do
    // lift suavizada; sombras longas mantêm-se pela LUZ, não pelo ambiente).
    // **Azul mais fundo (2026-10-21; era 0.30/0.40/0.72):** os valores
    // antigos liam como azul-claro de dia enevoado, não como noite.
    let cool_mix = (night.clamp(0.0, 1.0) * 0.8 + golden * 0.3).min(1.0);
    let r = ambient_color_ref[0] + (0.20 - ambient_color_ref[0]) * cool_mix;
    let g = ambient_color_ref[1] + (0.28 - ambient_color_ref[1]) * cool_mix;
    let b = ambient_color_ref[2] + (0.55 - ambient_color_ref[2]) * cool_mix;
    ambient.color = Color::srgba(r, g, b, 1.0);
}

/// Publish [`SunState`] from the day clock and aim the directional light.
///
/// NOTE (Claude): `main.rs` already scheduled this system but the body had not
/// been written yet, so the binary did not build. This is the straightforward
/// composition of the `sun_elevation` / `sun_azimuth` helpers that were already
/// here — replace it if the intended behaviour differs.
#[allow(clippy::needless_pass_by_value)]
pub fn sun_drive(
    clock: Option<Res<DayCycleState>>,
    mut sun: ResMut<SunState>,
    mut lights: Query<(
        Entity,
        &mut Transform,
        &mut DirectionalLight,
        Option<&SunLightBase>,
    )>,
    mut commands: Commands,
) {
    let Some(clock) = clock else {
        return;
    };
    // O `night-cap-minute` congela o VISUAL (sol/céu/névoa/ambiente) no
    // minuto do cap; o relógio continua a andar para quests/HUD.
    let visual = clock.visual_minute();
    let elevation_deg = sun_elevation(
        visual,
        clock.dawn_minute,
        clock.dusk_minute,
        clock.max_sun_elevation,
    );
    let azimuth_deg = sun_azimuth(
        visual,
        clock.dawn_minute,
        clock.dusk_minute,
        clock.sun_azimuth_base,
    );
    // Direction *towards* the sun (Y-up) — a posição astronómica real, que é
    // o que o céu procedural e a lógica de jogo leem.
    sun.dir = sky_direction(elevation_deg, azimuth_deg);
    sun.elevation_deg = elevation_deg;
    // Night ramps in as the sun drops below the horizon.
    sun.night = (1.0 - (elevation_deg / 6.0).clamp(0.0, 1.0)).clamp(0.0, 1.0);

    // A luz direcional NÃO segue o sol abaixo do horizonte: passa a ser a
    // lua (alta, fria, fraca) com um crossfade pelo mesmo ramp dawn/dusk do
    // ambiente. Sem isto a noite ficava com o chão preto e o resto lavado.
    let day = daylight_factor(visual, clock.dawn_minute, clock.dusk_minute);
    let sun_dir = sky_direction(
        elevation_deg.max(clock.min_sun_elevation.max(0.0)),
        azimuth_deg,
    );
    let moon_dir = sky_direction(MOON_ELEVATION_DEG, azimuth_deg + 180.0);
    let light_dir = moon_dir
        .lerp(sun_dir, day)
        .try_normalize()
        .unwrap_or(moon_dir);
    let scale = MOONLIGHT_RATIO + (1.0 - MOONLIGHT_RATIO) * day;

    // Sunlight travels from the sun into the scene; bevy shines a directional
    // light along the entity's -Z (same convention as `recipes::spawn`).
    let rotation = Quat::from_rotation_arc(-Vec3::Z, -light_dir);
    for (entity, mut transform, mut light, base) in &mut lights {
        let base = match base {
            Some(base) => *base,
            None => {
                let captured = SunLightBase {
                    illuminance: light.illuminance,
                    color: light.color,
                };
                commands.entity(entity).insert(captured);
                captured
            }
        };
        transform.rotation = rotation;
        light.illuminance = base.illuminance * scale;
        light.color = mix_linear(MOON_COLOR, base.color, day);
    }
}
// ── Atmosfera: paleta única partilhada por céu, fog e grading ───────────
//
// O gap contra o BOTW não era "faltam nuvens" — era **perspetiva aérea**.
// No baseline, o que estava a 300 m tinha a mesma cor e o mesmo contraste do
// que estava a 3 m, e o céu era um degradê que ignorava o relógio do mundo
// (o shader derivava a hora de `globals.time`, que o `set_clock` não mexe;
// às 23:02 ainda se via o disco solar).
//
// [`AtmosphereState`] resolve as duas coisas: é a ÚNICA fonte da paleta da
// hora — zénite, horizonte, cor do sol, fog, exposição — e é lida por
// `sky.rs` (uniform do domo), `ambient.rs` (DistanceFog na câmara) e
// `postfx.rs` (exposição/bloom). Céu, névoa e grading deixam de poder
// discordar sobre que horas são.

/// Cor linear (RGB) — mistura em espaço linear, como toda a iluminação.
type Rgb = [f32; 3];

fn mix3(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn smooth_step(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Paleta por fase (linear). Os valores do horizonte são deliberadamente
// PÁLIDOS e pouco saturados — é isso que faz uma crista a 400 m separar-se
// da crista a 800 m em vez de as duas lerem como a mesma maquete verde.
const ZENITH_DAY: Rgb = [0.085, 0.255, 0.62];
const HORIZON_DAY: Rgb = [0.60, 0.755, 0.90];
const ZENITH_GOLD: Rgb = [0.095, 0.16, 0.42];
const HORIZON_GOLD: Rgb = [0.44, 0.26, 0.12];
// Noite AZUL legível (a assinatura BOTW): o céu noturno não é preto — é um
// azul profundo que se lê como céu, e é esse contraste que faz a lua, as
// estrelas e as fogueiras valerem ouro. Antes era [0.008,0.016,0.055] /
// [0.035,0.062,0.135] = "só escuro" (gap nº 9 do gauntlet).
//
// **×0,45 (2026-10-21).** A paleta do céu e da névoa NÃO passa pela exposição
// do PBR (o domo é material custom e o `DistanceFog` é somado depois de
// `view.exposure`); o CDL da noite escurece os objetos PBR mas NÃO esta
// paleta. Os valores antigos liam-se como um véu azul-claro de dia enevoado
// (medido: céu 0,50 e chão 0,44 às 22:00) e tapavam até as fogueiras. A
// paleta noturna/crepuscular é por isso autorada em espaço de EXIBIÇÃO —
// aqui 0,0225 linear ≈ 0,16 sRGB. O dia e a golden hour não são tocados.
const ZENITH_NIGHT: Rgb = [0.0036, 0.0072, 0.0225];
const HORIZON_NIGHT: Rgb = [0.0135, 0.0225, 0.0495];
/// Azul do crepúsculo (blue hour) — o sol já se pôs, o céu ainda não é noite.
const ZENITH_TWILIGHT: Rgb = [0.0090, 0.0203, 0.0608];
const HORIZON_TWILIGHT: Rgb = [0.0900, 0.0698, 0.1350];

const SUN_TINT_HIGH: Rgb = [1.0, 0.96, 0.88];
const SUN_TINT_LOW: Rgb = [1.0, 0.46, 0.14];
const MOON_TINT: Rgb = [0.62, 0.72, 1.0];

/// Paleta atmosférica da hora corrente — publicada por [`atmosphere_drive`].
#[derive(Debug, Clone, Copy, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct AtmosphereState {
    /// Direção PARA o sol (mundo, Y-up) — a astronómica, pode estar abaixo.
    pub sun_dir: Vec3,
    /// Direção PARA a lua (oposta ao sol, elevada).
    pub moon_dir: Vec3,
    /// `1` dia pleno, `0` noite plena (rampa pela elevação do sol).
    pub day: f32,
    /// `1` noite plena.
    pub night: f32,
    /// Pico quando o sol raspa o horizonte (hora dourada / crepúsculo).
    pub golden: f32,
    /// Cor do céu no zénite / no horizonte (linear).
    pub zenith: Rgb,
    pub horizon: Rgb,
    /// Cor da névoa de distância (linear) — o horizonte, ligeiramente
    /// dessaturado, é o que dá a perspetiva aérea.
    pub fog: Rgb,
    /// Cor do inscattering do sol na névoa (o lado do mundo virado ao sol
    /// fica dourado ao pôr-do-sol; é o que faltava nas "paredes cinzentas").
    pub sun_tint: Rgb,
    /// Multiplicador de exposição sugerido (1 = base do mundo).
    pub exposure_scale: f32,
    /// Reforço de bloom sugerido (fontes quentes à noite / sol rasante).
    pub bloom_boost: f32,
}

impl Default for AtmosphereState {
    fn default() -> Self {
        Self {
            sun_dir: Vec3::Y,
            moon_dir: -Vec3::Y,
            day: 1.0,
            night: 0.0,
            golden: 0.0,
            zenith: ZENITH_DAY,
            horizon: HORIZON_DAY,
            fog: HORIZON_DAY,
            sun_tint: SUN_TINT_HIGH,
            exposure_scale: 1.0,
            bloom_boost: 0.0,
        }
    }
}

/// Resolve a paleta atmosférica para uma elevação solar (graus).
///
/// Separado do sistema para ser testável sem `App`: as transições (noite →
/// crepúsculo → dourado → dia) são o coração do "não é só o dia com o brilho
/// baixado".
pub fn atmosphere_for_elevation(elevation_deg: f32) -> AtmosphereState {
    // Rampas pela elevação do sol, não pelo relógio: assim o mesmo código
    // serve mundos com dawn/dusk diferentes.
    let day = smooth_step(-3.0, 10.0, elevation_deg);
    let night = 1.0 - smooth_step(-9.0, -1.0, elevation_deg);
    // Dourado: pico com o sol entre ~0° e ~10°.
    let golden = (-(((elevation_deg - 4.0) / 8.0).powi(2))).exp() * (1.0 - night);
    // Crepúsculo (blue hour): o sol já desceu mas o céu ainda tem luz.
    let twilight = smooth_step(-12.0, -2.0, elevation_deg) * (1.0 - day);

    // Noite → crepúsculo → dia, com o dourado sobreposto por cima do dia.
    let mut zenith = mix3(ZENITH_NIGHT, ZENITH_TWILIGHT, twilight);
    let mut horizon = mix3(HORIZON_NIGHT, HORIZON_TWILIGHT, twilight);
    zenith = mix3(zenith, ZENITH_DAY, day);
    horizon = mix3(horizon, HORIZON_DAY, day);
    zenith = mix3(zenith, ZENITH_GOLD, golden * 0.85);
    horizon = mix3(horizon, HORIZON_GOLD, golden * 0.9);

    // Névoa = horizonte puxado ao azul e dessaturado: a "cortina" que separa
    // as camadas de serra. De dia é fria; ao pôr-do-sol herda o dourado do
    // horizonte (mas menos saturado, senão o mundo inteiro vira laranja).
    // A dessaturação cai para METADE à noite: 22 % de luma sobre um céu já
    // escuro lia-se como véu cinzento no longe (relato 2026-10-21).
    let luma = horizon[0] * 0.25 + horizon[1] * 0.5 + horizon[2] * 0.25;
    let mut fog = mix3(horizon, [luma, luma, luma], 0.22 * (1.0 - 0.5 * night));
    // Um empurrão de azul/violeta na névoa — a assinatura da perspetiva
    // aérea do BOTW. De dia 0.7; à noite 0.55, senão a névoa (e o horizonte
    // que ela funde) lia CINZENTO em vez de azul-noite (relato 2026-10-21).
    fog = mix3(
        fog,
        [fog[0] * 0.90, fog[1] * 0.98, fog[2] * 1.14],
        (day * 0.7 + night * 0.55).min(1.0),
    );

    let sun_tint = mix3(
        SUN_TINT_LOW,
        SUN_TINT_HIGH,
        smooth_step(2.0, 26.0, elevation_deg),
    );

    // Exposição: a noite fecha ~1 stop (é o que faz uma fogueira valer ouro)
    // sem cair no preto, e a golden hour abre ~meio stop. O +1.60 do r3
    // abria 1.3 stops na golden e estourava o frame inteiro assim que o
    // inscattering entrava (a névoa é somada DEPOIS da exposição).
    let exposure_scale = 0.52 + 0.48 * day + 0.55 * golden;
    // Bloom: mais à noite (lua/lanternas/fogueiras com halo) e com o sol
    // rasante (glare largo na água e nas cristas).
    let bloom_boost = 0.16 * night + 0.18 * golden;

    AtmosphereState {
        sun_dir: Vec3::Y,
        moon_dir: -Vec3::Y,
        day,
        night,
        golden,
        zenith,
        horizon,
        fog,
        sun_tint: if night > 0.5 {
            mix3(MOON_TINT, sun_tint, 1.0 - night)
        } else {
            sun_tint
        },
        exposure_scale,
        bloom_boost,
    }
}

/// Publica [`AtmosphereState`] a partir do relógio (corre depois de
/// [`sun_drive`] e DEPOIS de [`weather_drive`] — a chuva do frame entra na
/// paleta no mesmo tick).
#[allow(clippy::needless_pass_by_value)]
pub fn atmosphere_drive(
    clock: Option<Res<DayCycleState>>,
    sun: Res<SunState>,
    weather: Option<Res<WeatherState>>,
    interior: Option<Res<InteriorLighting>>,
    mut atmosphere: ResMut<AtmosphereState>,
) {
    // Dentro de um interior a PALETA também é de dia: o `AtmosphereState`
    // manda no grading, na exposição e na névoa, e com a paleta da noite a
    // sala continuava azul-escura por muito que o ambiente subisse (medido:
    // 99/255 com R53 G109 B133 já com sol e IBL a zero).
    let inside = interior.is_some_and(|state| state.active);
    let elevation = if inside {
        INTERIOR_SUN_ELEVATION_DEG
    } else if clock.is_some() {
        sun.elevation_deg
    } else {
        // Sem <DayCycle> o mundo é estático: dia pleno.
        45.0
    };
    let mut next = atmosphere_for_elevation(elevation);
    next.sun_dir = if sun.dir.length_squared() > 0.0 {
        sun.dir
    } else {
        Vec3::Y
    };
    // A lua nasce oposta ao sol e alta — o mesmo arco que a luz da lua usa
    // em `sun_drive`, para que disco e sombra apontem ao mesmo sítio.
    let azimuth = next.sun_dir.x.atan2(next.sun_dir.z).to_degrees();
    next.moon_dir = sky_direction(MOON_ELEVATION_DEG, azimuth + 180.0);
    // CHUVA (WS-A): um toque de exposição, não um véu. Era −25 % (tempestade
    // a meio da noite apagava o chão). 8 % ainda lê "céu fechado"; a névoa
    // e as gotas é que dizem que chove.
    // Só recursos runtime: o shader em disco NUNCA é re-escrito aqui.
    // Não chove dentro de casa: a bolsa já é isenta de chuva, e fechar a
    // exposição por causa do tempo lá fora não faz sentido nenhum.
    let rain = if inside {
        0.0
    } else {
        weather.map(|w| w.rain.clamp(0.0, 1.0)).unwrap_or(0.0)
    };
    next.exposure_scale *= 1.0 - 0.08 * rain;
    *atmosphere = next;
}

/// Direção unitária (mundo, Y-up) para uma elevação/azimute em graus.
fn sky_direction(elevation_deg: f32, azimuth_deg: f32) -> Vec3 {
    let (el, az) = (elevation_deg.to_radians(), azimuth_deg.to_radians());
    Vec3::new(el.cos() * az.sin(), el.sin(), el.cos() * az.cos()).normalize_or_zero()
}

/// Interpola `night` (RGB linear) → `day` em espaço linear por `t`.
fn mix_linear(night: [f32; 3], day: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let d = day.to_linear();
    Color::LinearRgba(bevy::color::LinearRgba::rgb(
        night[0] + (d.red - night[0]) * t,
        night[1] + (d.green - night[1]) * t,
        night[2] + (d.blue - night[2]) * t,
    ))
}

// ── plugin ──────────────────────────────────────────────────────────────

/// Wiring dos drivers do mundo. A ordem replicada é a do `app.rs` do
/// monólito: o flag de interior decide ANTES do relógio; o regime de
/// interior aplica DEPOIS do sol; o clima rola ANTES da atmosfera (a chuva
/// do frame entra na paleta no mesmo tick).
///
/// Sem o `timed(Group::World, …)` do profiler do monólito — quem quiser os
/// tempos por sistema registra os wrappers por cima; os sistemas são
/// públicos e a ordem não muda.
pub struct WorldsysPlugin;

impl Plugin for WorldsysPlugin {
    fn build(&self, app: &mut App) {
        use bevy::app::Update;
        app.init_resource::<SunState>().add_systems(
            Update,
            (
                interior_lighting_drive.before(daycycle_drive),
                daycycle_drive,
                // Regime de interior por CIMA dos drivers do mundo (sol/IBL).
                interior_lighting_apply
                    .after(sun_drive)
                    .after(daycycle_drive),
                sun_drive,
                // O `weather_drive` rola a chuva antes de a paleta ler o
                // `WeatherState` (mesmo frame, sem salto).
                weather_drive.before(atmosphere_drive),
                atmosphere_drive,
            ),
        );
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ext_postfx::env::BiomeRegionData;

    /// Câmara de interior: o alvo é o CENTRO DA SALA, quantizado pela grelha
    /// — é o que faz o enquadramento saltar de sala em sala (JRPG 16 bits) em
    /// vez de seguir o herói.
    #[test]
    fn interior_room_center_quantizes_to_the_grid() {
        let scene = InteriorSceneConfig {
            min: [2560.0, 2560.0],
            max: [2740.0, 2725.0],
            room_size: [60.0, 55.0],
            room_origin: [2600.0, 2600.0],
            ..InteriorSceneConfig::default()
        };
        let a = scene.room_center(2596.0, 2604.0);
        let b = scene.room_center(2612.0, 2620.0);
        assert_eq!(a, [2600.0, 2600.0]);
        assert_eq!(a, b);
        let c = scene.room_center(2650.0, 2604.0);
        assert_eq!(c, [2660.0, 2600.0]);
        assert_ne!(a, c);
    }

    /// Sem `room-size` autorado, a bolsa inteira é uma sala só (o centro é
    /// sempre o mesmo ponto).
    #[test]
    fn interior_without_room_size_is_a_single_room() {
        let scene = InteriorSceneConfig {
            min: [0.0, 0.0],
            max: [100.0, 80.0],
            ..InteriorSceneConfig::default()
        };
        assert_eq!(scene.room_center(5.0, 5.0), [50.0, 40.0]);
        assert_eq!(scene.room_center(95.0, 75.0), [50.0, 40.0]);
    }

    #[test]
    fn test_daylight_factor_ramps() {
        assert_eq!(daylight_factor(100.0, 330.0, 1170.0), 0.0); // night
        assert_eq!(daylight_factor(600.0, 330.0, 1170.0), 1.0); // mid-day
        let mid = daylight_factor(300.0, 330.0, 1170.0); // dawn ramp
        assert!((0.0..1.0).contains(&mid));
        assert_eq!(daylight_factor(1231.0, 330.0, 1170.0), 0.0); // after dusk ramp
    }

    fn baked_clock(minute: f32, night_cap: f32) -> DayCycleState {
        DayCycleState::from_parts(
            minute, 0.0, 330.0, 1170.0, 0.26, 0.07, true, 62.0, 205.0, 8.0, 1.0, 1.0, night_cap,
        )
        .with_day_night(DayNightConfig {
            day_minute: 720.0,
            night_minute: 1320.0,
            transition_seconds: 4.0,
        })
    }

    /// O arranque assenta logo na âncora do estado do relógio: nada de uma
    /// travessia de 12:00 para 22:00 no primeiro frame.
    #[test]
    fn test_day_night_starts_settled_on_clock_phase() {
        let clock = baked_clock(480.0, 0.0);
        assert_eq!(clock.visual_minute(), 720.0);
        assert_eq!(clock.visual_daylight(), 1.0);
        assert!(clock.visual_day);

        let night = baked_clock(1320.0, 0.0);
        assert_eq!(night.visual_minute(), 1320.0);
        assert_eq!(night.visual_daylight(), 0.0);
        assert!(!night.visual_day);
    }

    /// A travessia dia→noite avança o relógio visual (pôr-do-sol), e a
    /// noite→dia avança pela meia-noite/amanhecer — nunca anda para trás.
    #[test]
    fn test_day_night_fast_forwards_forward_only() {
        let mut clock = baked_clock(480.0, 0.0);
        // Cruza o limiar das 20:00 (daylight 0.5): começa a travessia.
        clock.minute_of_day = 1200.0;
        clock.advance_visual(0.0);
        assert_eq!(clock.visual_minute(), 720.0, "t=0 parte da âncora");
        clock.advance_visual(2.0);
        let mid = clock.visual_minute();
        assert!(
            (mid - 1020.0).abs() < 1.0,
            "meio da travessia = 17:00, veio {mid}"
        );
        clock.advance_visual(2.0);
        assert_eq!(clock.visual_minute(), 1320.0);
        assert_eq!(clock.visual_daylight(), 0.0);

        // Cruza as 05:00: 22:00 → 12:00 pelo caminho da meia-noite.
        clock.minute_of_day = 300.0;
        clock.advance_visual(2.0);
        let mid = clock.visual_minute();
        assert!(
            (mid - 300.0).abs() < 1.0,
            "meio da travessia = 05:00, veio {mid}"
        );
        clock.advance_visual(2.0);
        assert_eq!(clock.visual_minute(), 720.0);
        assert_eq!(clock.visual_daylight(), 1.0);
    }

    /// `transition-seconds="0"` é um corte seco, não uma divisão por zero.
    #[test]
    fn test_day_night_zero_transition_snaps() {
        let mut clock = baked_clock(480.0, 0.0);
        clock.day_night.transition_seconds = 0.0;
        clock.minute_of_day = 1200.0;
        clock.advance_visual(0.0);
        assert_eq!(clock.visual_minute(), 1320.0);
        assert_eq!(clock.visual_transition, 1.0);
    }

    /// No modo `day-night` o `night-cap-minute` não manda: a âncora é a
    /// verdade visual (o cap só existe para o modo contínuo).
    #[test]
    fn test_day_night_ignores_night_cap() {
        let clock = baked_clock(1380.0, 1200.0);
        assert_eq!(clock.visual_minute(), 1320.0);
        // Contínuo continua a congelar no cap.
        let continuous = DayCycleState::from_parts(
            1380.0, 0.0, 330.0, 1170.0, 0.26, 0.07, true, 62.0, 205.0, 8.0, 1.0, 1.0, 1200.0,
        );
        assert_eq!(continuous.visual_minute(), 1200.0);
    }

    /// O modo contínuo não é tocado pela máquina de estados.
    #[test]
    fn test_continuous_mode_ignores_advance_visual() {
        let mut clock = DayCycleState::from_parts(
            600.0, 0.0, 330.0, 1170.0, 0.26, 0.07, true, 62.0, 205.0, 8.0, 1.0, 1.0, 0.0,
        );
        clock.advance_visual(10.0);
        assert_eq!(clock.visual_minute(), 600.0);
        assert_eq!(clock.visual_transition, 1.0);
    }

    /// Rampas que atravessam a meia-noite são contínuas (sem salto às 00:00).
    #[test]
    fn test_daylight_factor_wraps_midnight() {
        // dawn 00:30 → a rampa começa às 23:30 do dia anterior.
        let before = daylight_factor(1439.9, 30.0, 1170.0);
        let after = daylight_factor(0.0, 30.0, 1170.0);
        assert!(
            (before - 0.5).abs() < 0.01 && (after - 0.5).abs() < 0.01,
            "{before} {after}"
        );
        assert!((daylight_factor(1410.0, 30.0, 1170.0) - 0.0).abs() < 1e-6);
        // dusk 23:40 → ainda a escurecer às 00:10.
        let fading = daylight_factor(10.0, 330.0, 1420.0);
        assert!((fading - 0.5).abs() < 1e-4, "{fading}");
        assert_eq!(daylight_factor(700.0, 0.0, 1440.0), 1.0, "dia inteiro");
    }

    /// dawn/dusk degenerados nunca produzem NaN/inf no arco solar — o NaN
    /// propagava-se à luz direcional e ao ambiente (ronda 2 de bugs).
    #[test]
    fn test_sun_arc_survives_degenerate_dawn_dusk() {
        for (dawn, dusk) in [
            (330.0, 330.0),
            (1170.0, 330.0),
            (0.0, 1440.0),
            (1440.0, 0.0),
        ] {
            for minute in [0.0, 60.0, 330.0, 719.5, 1170.0, 1439.0] {
                let el = sun_elevation(minute, dawn, dusk, 62.0);
                let az = sun_azimuth(minute, dawn, dusk, 205.0);
                assert!(
                    el.is_finite(),
                    "elevação finita dawn={dawn} dusk={dusk} m={minute}: {el}"
                );
                assert!(
                    az.is_finite(),
                    "azimute finito dawn={dawn} dusk={dusk} m={minute}: {az}"
                );
            }
        }
        // O caso patológico real: dawn=0/dusk=1440 zerava o dusk_len e a
        // meia-noite do relógio dava 0/0.
        assert!(sun_elevation(0.0, 0.0, 1440.0, 62.0).is_finite());
        // `daylight_factor` não divide por (dusk−dawn) — locked para não
        // regredir para uma versão que divida.
        for (dawn, dusk) in [(330.0, 330.0), (1170.0, 330.0), (0.0, 1440.0)] {
            for minute in [0.0, 300.0, 600.0, 1200.0, 1439.0] {
                assert!(daylight_factor(minute, dawn, dusk).is_finite());
            }
        }
    }

    /// Lei do repo: mesma seed + mesmo índice → mesmo alvo de chuva.
    #[test]
    fn test_weather_cycle_target_is_deterministic() {
        for seed in [0u64, 42, 0xDEAD_BEEF] {
            for index in [0u64, 1, 7, 1000] {
                let a = weather_cycle_target(seed, index);
                let b = weather_cycle_target(seed, index);
                assert_eq!(a, b, "seed {seed} índice {index} tem de repetir");
                assert!((0.0..=1.0).contains(&a), "alvo em 0..1: {a}");
            }
        }
        // Índices diferentes (mesma seed) divergem — é o que faz o tempo mudar.
        assert_ne!(weather_cycle_target(7, 1), weather_cycle_target(7, 2));
        // Sementes diferentes no mesmo índice também.
        assert_ne!(weather_cycle_target(1, 5), weather_cycle_target(2, 5));
        // Enviesado para seco (quadrado do uniforme): média bem abaixo de 0.5.
        let mean: f32 = (0..256u64).map(|i| weather_cycle_target(9, i)).sum::<f32>() / 256.0;
        assert!(mean < 0.45, "média seca: {mean}");
    }

    /// O lerp de chuva aproxima-se à velocidade constante, sem overshoot.
    #[test]
    fn test_rain_scale_at_picks_the_region_under_the_point() {
        let region = |id: &str, polygon: Vec<[f32; 2]>, rain_scale: Option<f32>| {
            BiomeRegionData {
            id: id.into(),
            display_name: String::new(),
            polygon,
            fog_density: 0.0,
            tint: None,
            pp_exposure: None,
            pp_bloom_strength: None,
                rain_scale,
            }
        };
        let regions = BiomeRegions {
            list: vec![
                region(
                    "desert",
                    vec![[0.0, -10.0], [0.0, 10.0], [100.0, 10.0], [100.0, -10.0]],
                    Some(0.0),
                ),
                region(
                    "forest",
                    vec![[-100.0, -10.0], [-100.0, 10.0], [0.0, 10.0], [0.0, -10.0]],
                    None,
                ),
            ],
        };
        assert_eq!(rain_scale_at(Some(&regions), 50.0, 0.0), Some(0.0));
        // Região sem `rain-scale` e fora de tudo: a chuva do mundo passa.
        assert_eq!(rain_scale_at(Some(&regions), -50.0, 0.0), None);
        assert_eq!(rain_scale_at(Some(&regions), 500.0, 500.0), None);
        assert_eq!(rain_scale_at(None, 50.0, 0.0), None);
    }

    #[test]
    fn test_rain_toward_is_monotonic_and_clamped() {
        let mut value = 0.0_f32;
        let mut previous = value;
        for _ in 0..200 {
            value = rain_toward(value, 1.0, 0.1, 10.0);
            assert!(value >= previous, "nunca regride a subir");
            assert!(value <= 1.0, "nunca ultrapassa o alvo");
            previous = value;
        }
        assert!((value - 1.0).abs() < 1e-6, "converge: {value}");
        // Descida igualmente monótona, e dt gigante NÃO salta o alvo.
        let one_step = rain_toward(1.0, 0.0, 100.0, 10.0);
        assert_eq!(one_step, 0.0);
        // Fora de gama satura em vez de explodir.
        assert_eq!(rain_toward(-3.0, 0.5, 1.0, 10.0), 0.0 + 0.1);
        assert_eq!(rain_toward(2.0, 0.5, 1.0, 10.0), 0.9);
        // dt 0 é no-op.
        assert_eq!(rain_toward(0.3, 0.9, 0.0, 10.0), 0.3);
    }

    /// O scheduler rola um novo alvo a cada `period` s, com determinismo
    /// "mesma seed, mesmo mundo" e default 240 s.
    #[test]
    fn test_weather_scheduler_rolls_each_period() {
        let mut scheduler = WeatherScheduler::new(77, 0.0, 30.0);
        assert_eq!(scheduler.target, 0.0, "parte do estado autoral");
        let mut last = scheduler.target;
        for _ in 0..29 {
            last = scheduler.tick(1.0);
        }
        assert_eq!(scheduler.index, 0, "antes do período não rola");
        scheduler.tick(1.0);
        assert_eq!(scheduler.index, 1, "rolou no fim do 1.º período");
        assert_eq!(scheduler.target, weather_cycle_target(77, 1));
        assert!(
            (scheduler.timer - 30.0).abs() < 1e-4,
            "timer recomeçado cheio: {}",
            scheduler.timer
        );
        let _ = last;

        // dt maior que o período rola EXATAMENTE um ciclo por tick (sem
        // espiral de mortes por acumulação).
        scheduler.tick(120.0);
        assert_eq!(scheduler.index, 2);

        // Default: período 0 → 240 s.
        assert_eq!(
            WeatherScheduler::new(0, 0.0, 0.0).period,
            WEATHER_CYCLE_DEFAULT_SECS
        );
        // E a mesma seed+índice reproduce a mesma sequência inteira.
        let mut a = WeatherScheduler::new(123, 0.5, 10.0);
        let mut b = WeatherScheduler::new(123, 0.5, 10.0);
        for _ in 0..50 {
            assert_eq!(a.tick(3.0), b.tick(3.0));
        }
    }
    /// A chuva fecha exposição no [`atmosphere_drive`] (×(1−0.08·rain)).
    #[test]
    fn test_atmosphere_drive_couples_rain_to_exposure() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.init_resource::<SunState>();
        app.init_resource::<crate::worldsys::AtmosphereState>();
        app.insert_resource(WeatherState {
            wind: [0.0, 0.0],
            wind_strength: 0.0,
            clouds: 0.0,
            rain: 1.0,
            cycle: false,
        });
        app.add_systems(bevy::app::Update, atmosphere_drive);
        app.update();

        let dry = atmosphere_for_elevation(45.0).exposure_scale;
        let wet = app
            .world()
            .resource::<crate::worldsys::AtmosphereState>()
            .exposure_scale;
        assert!(
            (wet - dry * 0.92).abs() < 1e-5,
            "tempestade fecha 8 % da exposição: {wet} vs base {dry}"
        );
    }

    /// The day/night intensities are a ratio, not an absolute brightness.
    ///
    /// Regression: `ambient-day-intensity` is a three.js 0..1 intensity while
    /// Bevy's `AmbientLight::brightness` is in lux, and the same worlds author
    /// it in the hundreds. Writing 0.26 straight into it dropped the ambient
    /// by ~400x and left the village in the dark.
    #[test]
    fn test_daycycle_ambient_is_a_ratio_of_the_authored_brightness() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.insert_resource(GlobalAmbientLight {
            brightness: 110.0,
            ..Default::default()
        });
        app.insert_resource(DayCycleState::from_parts(
            600.0, // midday
            0.0,   // clock frozen, so the test is about the ramp only
            330.0, 1170.0, 0.26, 0.07, true, 62.0, 205.0, 8.0, 1.0, 1.0, 0.0,
        ));
        // Sol a pino: fill de sombras (golden/night) a 0 — o teste isola o
        // ramp dia/noite do ambiente, não o lift do fill.
        app.insert_resource(SunState {
            dir: Vec3::Y,
            elevation_deg: 90.0,
            night: 0.0,
        });
        app.add_systems(bevy::app::Update, daycycle_drive);

        app.update();
        let midday = app.world().resource::<GlobalAmbientLight>().brightness;
        assert!(
            (midday - 110.0).abs() < 1.0,
            "full day keeps the authored brightness, got {midday}"
        );

        // Midnight falls to the night/day ratio of it, not to 0.07 lux.
        app.world_mut()
            .resource_mut::<DayCycleState>()
            .minute_of_day = 60.0;
        app.update();
        let night = app.world().resource::<GlobalAmbientLight>().brightness;
        let expected = 110.0 * (0.07 / 0.26);
        assert!(
            (night - expected).abs() < 1.0,
            "night is the authored brightness scaled by night/day, got {night} (expected {expected})"
        );
        assert!(night > 10.0, "night is dim, not black");
    }

    /// À noite a direcional vira lua: vem de CIMA e fica fraca.
    ///
    /// Regressão: `sun_drive` apontava a luz para o sol astronómico, que à
    /// noite está a −25°. A cena era iluminada por baixo — o chão (normal +Y)
    /// ficava preto enquanto paredes e props apanhavam luz de raspão, com o
    /// especular a varrer a imagem conforme a câmara rodava.
    #[test]
    fn test_night_light_comes_from_above_and_dims() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.init_resource::<SunState>();
        app.insert_resource(DayCycleState::from_parts(
            600.0, // midday
            0.0,   // clock frozen: the test is about the ramp only
            330.0, 1170.0, 0.26, 0.13, true, 62.0, 205.0, 8.0, 1.0, 1.0, 0.0,
        ));
        let light = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    illuminance: 10_000.0,
                    ..Default::default()
                },
                Transform::default(),
            ))
            .id();
        app.add_systems(bevy::app::Update, sun_drive);

        app.update();
        let day_lux = app
            .world()
            .entity(light)
            .get::<DirectionalLight>()
            .unwrap()
            .illuminance;
        assert!(
            (day_lux - 10_000.0).abs() < 1.0,
            "full day keeps the authored illuminance, got {day_lux}"
        );

        app.world_mut()
            .resource_mut::<DayCycleState>()
            .minute_of_day = 60.0; // midnight
        app.update();

        let sun = *app.world().resource::<SunState>();
        assert!(
            sun.elevation_deg < 0.0,
            "the astronomical sun is still below the horizon at midnight"
        );

        let entity = app.world().entity(light);
        let night_lux = entity.get::<DirectionalLight>().unwrap().illuminance;
        let expected = 10_000.0 * MOONLIGHT_RATIO;
        assert!(
            (night_lux - expected).abs() < 1.0,
            "night dims to the moonlight ratio, got {night_lux} (expected {expected})"
        );
        assert!(night_lux > 0.0, "moonlight is dim, not off");

        // A luz viaja ao longo de -Z da entidade: -Z rodado tem de apontar
        // para BAIXO (componente Y negativa) — luz vinda de cima.
        let travel = entity.get::<Transform>().unwrap().rotation * Vec3::NEG_Z;
        assert!(
            travel.y < -0.5,
            "moonlight travels downwards onto the ground, got {travel:?}"
        );
    }
    /// A bolsa só isenta do clamp DENTRO do retângulo — e um retângulo
    /// degenerado (min == max) não isenta nada.
    #[test]
    fn test_interior_scene_contains() {
        let scene = InteriorSceneConfig {
            min: [2564.0, 2570.0],
            max: [2752.0, 2742.0],
            ..InteriorSceneConfig::default()
        };
        assert!(scene.contains(2658.0, 2656.0), "centro");
        assert!(scene.contains(2564.0, 2570.0), "canto (inclusivo)");
        assert!(!scene.contains(2563.0, 2656.0), "fora por x");
        assert!(!scene.contains(2658.0, 2743.0), "fora por z");
        assert!(!scene.contains(0.0, 0.0));
        assert!(!scene.contains(860.0, 780.0), "o Alto de Vael não é bolsa");
    }
}
