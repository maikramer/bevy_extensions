//! Definições de ambiente autoradas na cena para o `bevy_elements`.
//!
//! [`ElementsSettings`] é um componente `Reflect` numa entidade do `.bsn`:
//! o Jackdaw edita-o no inspector e o jogo lê-o (via
//! `bevy_ext_world::bsn::settings`) em vez de constantes no código. Este
//! crate não depende do `bevy_elements` — é só o contrato e uma estimativa
//! da posição do Sol para o preview do editor ([`sun_direction`]).

use bevy::prelude::*;

/// Relógio, local e clima de arranque do mundo.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_elements"]
pub struct ElementsSettings {
    /// Latitude em graus (norte positivo). Define a altura do Sol e a
    /// duração do dia.
    pub latitude_deg: f64,
    /// Data civil de arranque.
    pub start_year: i32,
    pub start_month: u8,
    pub start_day: u8,
    /// Hora local de arranque (0–24).
    pub start_hour: f64,
    /// Segundos de jogo por segundo real (72 = dia de 20 min).
    pub time_scale: f32,
    /// Regime que determina a temperatura e a frequência de cada tipo de tempo.
    #[reflect(default)]
    pub climate: ClimateProfile,
    /// Semente da sequência meteorológica horária.
    #[reflect(default)]
    pub weather_seed: u32,
    /// Clima forçado no arranque; `Auto` deixa o diretor do crate decidir.
    pub start_weather: StartWeather,
    /// Orçamento de GPU do céu/nuvens/névoa.
    pub quality: ElementsQualityLevel,
}

impl Default for ElementsSettings {
    /// Os valores que o Vael tinha em constantes: 50° N, solstício de junho,
    /// 08:00, dia de 20 min, qualidade média.
    fn default() -> Self {
        Self {
            latitude_deg: 50.0,
            start_year: 2026,
            start_month: 6,
            start_day: 21,
            start_hour: 8.0,
            time_scale: 72.0,
            climate: ClimateProfile::Temperate,
            weather_seed: 0,
            start_weather: StartWeather::Auto,
            quality: ElementsQualityLevel::Medium,
        }
    }
}

/// Perfis climáticos do `bevy_elements`.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_elements"]
pub enum ClimateProfile {
    #[default]
    Temperate,
    Alpine,
    Desert,
    Tropical,
    Tundra,
}

/// Os presets de clima do `bevy_elements` (mesmos nomes) mais `Auto`.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_elements"]
pub enum StartWeather {
    #[default]
    Auto,
    Clear,
    PartlyCloudy,
    Overcast,
    Fog,
    Drizzle,
    Rain,
    Thunderstorm,
    Snow,
    Blizzard,
    DustStorm,
}

impl StartWeather {
    pub const ALL: [Self; 11] = [
        Self::Auto,
        Self::Clear,
        Self::PartlyCloudy,
        Self::Overcast,
        Self::Fog,
        Self::Drizzle,
        Self::Rain,
        Self::Thunderstorm,
        Self::Snow,
        Self::Blizzard,
        Self::DustStorm,
    ];

    /// Nome do preset no `bevy_elements` (`None` para `Auto`).
    pub fn preset_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Auto => return None,
            Self::Clear => "Clear",
            Self::PartlyCloudy => "PartlyCloudy",
            Self::Overcast => "Overcast",
            Self::Fog => "Fog",
            Self::Drizzle => "Drizzle",
            Self::Rain => "Rain",
            Self::Thunderstorm => "Thunderstorm",
            Self::Snow => "Snow",
            Self::Blizzard => "Blizzard",
            Self::DustStorm => "DustStorm",
        })
    }

    /// Rótulo curto para o painel.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Clear => "Limpo",
            Self::PartlyCloudy => "Nuvens",
            Self::Overcast => "Encoberto",
            Self::Fog => "Nevoeiro",
            Self::Drizzle => "Chuvisco",
            Self::Rain => "Chuva",
            Self::Thunderstorm => "Trovoada",
            Self::Snow => "Neve",
            Self::Blizzard => "Nevasca",
            Self::DustStorm => "Poeira",
        }
    }
}

/// Espelho do `ElementsQuality` do crate.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_elements"]
pub enum ElementsQualityLevel {
    Low,
    #[default]
    Medium,
    High,
    Ultra,
}

impl ElementsSettings {
    /// Dia do ano (1–366) da data de arranque.
    pub fn day_of_year(&self) -> u32 {
        day_of_year(self.start_year, self.start_month, self.start_day)
    }

    /// Direção para o Sol à hora `hour` (ver [`sun_direction`]).
    pub fn sun_direction(&self, hour: f64) -> Vec3 {
        sun_direction(self.latitude_deg, self.day_of_year(), hour)
    }
}

/// Dia do ano de uma data civil (gregoriano).
pub fn day_of_year(year: i32, month: u8, day: u8) -> u32 {
    const CUMULATIVE: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let month = month.clamp(1, 12) as usize;
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    CUMULATIVE[month - 1] + u32::from(day.max(1)) + u32::from(leap && month > 2)
}

/// Direção unitária para o Sol (Y para cima, norte = −Z, este = +X) em hora
/// solar local, com a declinação de Cooper. Estimativa para preview no
/// editor: o jogo usa as efemérides do `bevy_elements`.
pub fn sun_direction(latitude_deg: f64, day_of_year: u32, hour: f64) -> Vec3 {
    let lat = latitude_deg.to_radians();
    let declination = (23.44_f64).to_radians()
        * ((360.0 / 365.0) * (284.0 + f64::from(day_of_year)))
            .to_radians()
            .sin();
    let hour_angle = (15.0 * (hour - 12.0)).to_radians();
    // Vetor horizontal-local: x = este, y = cima, z = sul (depois → norte = −Z).
    let up = lat.sin() * declination.sin() + lat.cos() * declination.cos() * hour_angle.cos();
    let east = -declination.cos() * hour_angle.sin();
    let north = lat.cos() * declination.sin() - lat.sin() * declination.cos() * hour_angle.cos();
    Vec3::new(east as f32, up as f32, -north as f32).normalize_or(Vec3::Y)
}

/// Regista os tipos para reflexão (jogo e editor).
pub struct ElementsSettingsPlugin;

impl Plugin for ElementsSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ElementsSettings>()
            .register_type::<ClimateProfile>()
            .register_type::<StartWeather>()
            .register_type::<ElementsQualityLevel>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sol_ao_meio_dia_do_solsticio_a_50_norte() {
        let dir = sun_direction(50.0, day_of_year(2026, 6, 21), 12.0);
        let elevation = dir.y.asin().to_degrees();
        assert!((elevation - 63.4).abs() < 1.0, "{elevation}");
        // Hemisfério norte: o Sol do meio-dia está a sul (+Z).
        assert!(dir.z > 0.0, "{dir}");
    }

    #[test]
    fn nasce_a_este_e_e_noite_a_meia_noite() {
        let morning = sun_direction(50.0, 172, 6.0);
        assert!(morning.x > 0.0, "{morning}");
        assert!(sun_direction(50.0, 172, 0.0).y < 0.0);
    }

    #[test]
    fn dia_do_ano() {
        assert_eq!(day_of_year(2026, 1, 1), 1);
        assert_eq!(day_of_year(2026, 6, 21), 172);
        assert_eq!(day_of_year(2024, 3, 1), 61);
    }
}
