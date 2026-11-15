//! Ambiente da água: vento, chuva, sol/lua e amplitude das ondas.
//!
//! A crate não fala com o relógio astronómico nem com o clima de um jogo
//! concreto: o host escreve este recurso por frame (o Vael liga-o ao
//! `bevy_elements`/`WeatherState`) e o material + solver leem-no.

use bevy::prelude::*;

/// Estado de ambiente que a água consome num frame.
#[derive(Resource, Debug, Clone)]
pub struct WaterEnvironment {
    /// Direção do vento em XZ (mundo).
    pub wind: [f32; 2],
    /// Intensidade do vento (0..~3) — velocidade e chop das ondas.
    pub wind_strength: f32,
    /// Chuva normalizada (0..1).
    pub rain: f32,
    /// Direção da luz ativa (sol de dia, lua de noite), normalizada.
    pub sun_dir: Vec3,
    /// Fator dia/noite (0 = noite, 1 = dia).
    pub day: f32,
    /// Cor da luz ativa (linear, sRGB 0..1).
    pub sun_tint: [f32; 3],
    /// Cor do zénite do céu.
    pub zenith: [f32; 3],
    /// Cor do horizonte do céu.
    pub horizon: [f32; 3],
    /// Amplitude das ondas autorada no mundo (`<Lake ripple>` máximo).
    pub wave_amp: f32,
    /// Tempo de simulação (s) para as ondas procedurais.
    pub time: f32,
}

impl Default for WaterEnvironment {
    fn default() -> Self {
        Self {
            wind: [0.7, 0.25],
            wind_strength: 1.5,
            rain: 0.0,
            sun_dir: Vec3::new(0.35, 0.72, 0.59).normalize(),
            day: 1.0,
            sun_tint: [1.0, 0.95, 0.86],
            zenith: [0.085, 0.255, 0.62],
            horizon: [0.60, 0.755, 0.90],
            wave_amp: 1.0,
            time: 0.0,
        }
    }
}

impl WaterEnvironment {
    /// `(wind_x, wind_z, wind_strength, wave_amp)` — o packing do uniform.
    pub fn wind_params(&self) -> Vec4 {
        Vec4::new(self.wind[0], self.wind[1], self.wind_strength, self.wave_amp)
    }

    /// `(sun_dir, day)` — o packing do uniform.
    pub fn sun_dir_day(&self) -> Vec4 {
        self.sun_dir.normalize_or(Vec3::Y).extend(self.day)
    }
}
