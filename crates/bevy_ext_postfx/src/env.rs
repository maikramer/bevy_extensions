//! Contratos de fronteira: estado do mundo que a lente lê.
//!
//! TODO(viber-exit): estes tipos vieram do monólito (`viber::worldsys`,
//! `viber::ambient`, `viber::scene_loader`) porque o runtime de pós-processo
//! os consome por frame. Quando os crates donos da cena/atmosfera aterrarem
//! nesta família, os tipos mudam para lá e o monólito passa a re-exportar
//! (o padrão já usado pelo `bevy_ext_core`); até lá vivem aqui, com o
//! TypePath histórico congelado para BRP/`.bsn`/saves.
//!
//! Uma App integrada só deve ter UM dono por tipo: quem monta o jogo
//! (plugin do Vael/fachada do Viber) publica estes recursos — não os do
//! monólito — para os drives da lente verem o mesmo estado.

use bevy::prelude::*;

// ── worldsys: atmosfera, sol, biomas, weather, tags ─────────────────────

/// Cor linear do monólito (`type Rgb = [f32; 3]` no `worldsys`).
pub type Rgb = [f32; 3];

/// Alvos de pós-processamento em vigor (base do mundo × bioma atual) do céu.
///
/// Estado publicado pelo drive da atmosfera a cada frame; os drives da lente
/// ([`crate::runtime`]) leem `night`/`day`/`golden`/`exposure_scale`/
/// `bloom_boost`. A paleta (`zenith`/`horizon`/`fog`/`sun_tint`) é do domo —
/// transportada integralmente para o contrato não partir os publicadores.
// `Copy` como no `worldsys` do monólito (o dono histórico): os campos são
// todos Copy (Vec3/f32/Rgb) e o domo/IBL copiam o estado por aí.
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

/// Paleta do dia pleno (linear) — a âncora do [`AtmosphereState::default`].
///
/// TODO(viber-exit): os valores vêm do `worldsys` do monólito; se o crate
/// dono do céu os redefinir, o default daqui segue-o.
pub const ZENITH_DAY: Rgb = [0.085, 0.255, 0.62];
/// Horizonte do dia pleno (linear).
pub const HORIZON_DAY: Rgb = [0.60, 0.755, 0.90];
/// Inscattering do sol alto (linear).
pub const SUN_TINT_HIGH: Rgb = [1.0, 0.96, 0.88];

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

/// Fração da iluminância do sol a que a lua desce (a luz direcional de
/// noite é a lua a [`MOONLIGHT_RATIO`] do sol autorado).
pub const MOONLIGHT_RATIO: f32 = 0.06;
/// Cor linear da luz da lua (azul frio) — o valor que o drive do sol
/// escreve na direcional à noite; a key da lua ([`crate::runtime`]) empurra
/// por cima para [`crate::runtime::MOON_KEY_COLOR`].
pub const MOON_COLOR: [f32; 3] = [0.40, 0.54, 0.86];

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

/// Iluminância/cor que o mundo autorou para uma `DirectionalLight`, capturada
/// antes de o drive do sol começar a escrevê-las. As keys da lua/dourada
/// compõem POR CIMA por frame sem nunca alterar a base (sem acumulação).
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::worldsys"]
pub struct SunLightBase {
    pub illuminance: f32,
    pub color: Color,
}

/// `<BiomeRegion>` polygon + fog/tint data (fog rendering follow-up).
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct BiomeRegionData {
    pub id: String,
    /// `display-name`: nome de exposição da zona no HUD (`zone.name`).
    /// Vazio = a engine usa a sua tabela de fallback (e, em último caso,
    /// deriva um título do próprio id).
    pub display_name: String,
    pub polygon: Vec<[f32; 2]>,
    pub fog_density: f32,
    pub tint: Option<[f32; 3]>,
    /// `pp-exposure`: linear exposure multiplier inside the region
    /// (`None` = the world base). Read by the postfx runtime.
    pub pp_exposure: Option<f32>,
    /// `pp-bloom-strength`: bloom intensity inside the region.
    pub pp_bloom_strength: Option<f32>,
    /// `rain-scale`: fator da chuva do `<Weather>` com o herói dentro da
    /// região (0 = seca, 1 = a do mundo; `None` = 1). Escala o ALVO do
    /// ciclo, portanto a chuva abranda/volta com a mesma rampa de sempre ao
    /// atravessar a fronteira — sem degrau.
    pub rain_scale: Option<f32>,
}

/// Todas as `<BiomeRegion>` do mundo (loop 9: fog/tint por bioma).
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct BiomeRegions {
    pub list: Vec<BiomeRegionData>,
}

/// Teste ponto-em-polígono (raio para −Z do plano XZ), o mesmo algoritmo do
/// `ambient.rs` do monólito. Usado pela exposição/bloom do bioma e pelo
/// bónus de pântano do volume de névoa.
pub fn point_in_polygon(x: f32, z: f32, polygon: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = polygon.len().saturating_sub(1);
    for i in 0..polygon.len() {
        let (xi, zi) = (polygon[i][0], polygon[i][1]);
        let (xj, zj) = (polygon[j][0], polygon[j][1]);
        let crosses = (zi > z) != (zj > z);
        if crosses {
            let intersect_x = (xj - xi) * (z - zi) / (zj - zi + f32::EPSILON) + xi;
            if x < intersect_x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Estado do `<Weather>` corrente — o vento é o que rola a textura do
/// volume de névoa (`drive_fog_texture` no [`crate::runtime`]).
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

impl Default for WeatherState {
    fn default() -> Self {
        Self {
            wind: [0.7, 0.25],
            wind_strength: 1.0,
            clouds: 0.3,
            rain: 0.0,
            cycle: false,
        }
    }
}

/// Elemento de configuração de engine genérico, mantido como dados crus
/// (`<PostFxDebugToggle>`, `<AdaptiveQuality>`, …). O parser do mundo
/// lowercased os nomes dos attrs.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct EngineConfigData {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
}

impl EngineConfigData {
    /// Raw value of `key` (attr names are already lowercased by the recipe).
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// `key` parsed as a finite `f32`; `None` when absent or unparsable.
    pub fn f32_attr(&self, key: &str) -> Option<f32> {
        self.attr(key)
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
    }
}

/// Every `EngineConfig` element of the world, in document order.
///
/// One resource for all of them: a world declares `<NavMesh>`, `<SpawnGate>`,
/// `<PostFxDebugToggle>`… side by side (and `<ProjectileTemplate>` many
/// times), so a resource per element would keep only the last one.
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::worldsys"]
pub struct EngineConfigs {
    pub list: Vec<EngineConfigData>,
}

impl EngineConfigs {
    /// First element with this (lowercase) tag.
    pub fn first(&self, tag: &str) -> Option<&EngineConfigData> {
        self.list.iter().find(|c| c.tag == tag)
    }

    /// Every element with this (lowercase) tag, in document order.
    pub fn all<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a EngineConfigData> + 'a {
        self.list.iter().filter(move |c| c.tag == tag)
    }
}

// ── ambient: marcadores de luz ──────────────────────────────────────────

/// PointLight com sombra AUTORADA no mundo (a família "tocha"): o gate
/// colorimétrico do pool nocturno separa-a das luzes quentes sem sombra
/// (fill ×8) — ver [`crate::runtime::TORCH_POOL_NIGHT_GAIN`]. O orçamento
/// de sombras pode desligar `shadow_maps_enabled`, mas o marcador fica.
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::ambient"]
pub struct AuthoredShadowLight;

/// Lanterna do viajante: a intensidade é conduzida pelo próprio driver
/// (`night` × máximo) e nasce a 0 — o pool nocturno NÃO a toca (capturaria
/// base 0 e escreveria 0 para sempre).
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::ambient"]
pub struct TravellerLantern;

// ── scene_loader: persistência entre cenas ──────────────────────────────

/// Entidade que sobrevive a trocas de cena (autoloads, câmara de editor,
/// sonda IBL…). O volume de névoa cinemático usa-a para o sweep do
/// `unload_scene` não o levar numa troca de cena.
///
/// TODO(viber-exit): muda para o crate de cena quando aterrar.
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::scene_loader"]
pub struct Persistent;

#[cfg(test)]
mod tests {
    use super::*;

    /// O polígono fechado por si mesmo contém o centro e nada lá fora.
    #[test]
    fn test_point_in_polygon_hit_and_miss() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(point_in_polygon(5.0, 5.0, &square));
        assert!(!point_in_polygon(15.0, 5.0, &square));
        assert!(!point_in_polygon(-1.0, -1.0, &square));
        // Polígono vazio e degenerado: nunca contém.
        assert!(!point_in_polygon(5.0, 5.0, &[]));
        let degenerate = [[1.0, 1.0], [2.0, 2.0]];
        assert!(!point_in_polygon(1.5, 1.5, &degenerate));
    }
}
