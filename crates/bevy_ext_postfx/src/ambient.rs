//! Névoa por bioma e o estado `CurrentBiome` (extraído do `ambient.rs` do
//! monólito, exit5/G-2): a lente já lê `env::AtmosphereState`/
//! `env::BiomeRegions` por frame — a névoa é o terceiro consumidor da mesma
//! paleta, por isso mora no crate.
//!
//! Ficaram no monólito: o emissor visual de chuva, a lanterna e o orçamento
//! de luzes (`bevy_ext_engine::ambient`) e a fatia de áudio
//! (`bevy_ext_audio::{sfx, ambience}`).
//!
//! TypePath histórico preservado: `#[type_path = "viber::ambient"]` no
//! `CurrentBiome`.

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy_ext_core::{Player, ScriptToast};

// ── Perspetiva aérea: névoa de distância sempre ligada ─────────────────
//
// A `DistanceFog` só existia DENTRO de uma `<BiomeRegion>` e usava o tint do
// bioma como cor — ou seja: no vale (a vila, metade do jogo) não havia névoa
// nenhuma, e na floresta a névoa era verde-escura, o que ESCURECE o longe em
// vez de o afastar. Resultado: tudo lia à mesma distância, como maquete.
//
// Agora a névoa está sempre lá e a cor vem de [`AtmosphereState`] — a mesma
// paleta do céu. O que está longe funde-se com o horizonte, exatamente o que
// separa camadas de serra nas referências. O bioma deixa de ser a fonte da
// cor e passa a ser um MODULADOR: densidade (o pântano fecha, o deserto abre)
// e um empurrão leve de tinta (só 25% — mais do que isso e volta o
// verde-escuro).
//
// `directional_light_color` liga o inscattering do sol: a metade do mundo
// virada ao sol fica dourada ao pôr-do-sol, que é a razão pela qual as
// paredes deixam de ser cinzentas às 19:00.

/// Distância (m) a que a névoa atinge ~63% com densidade base. Abaixo disto
/// a imagem fica intacta — a legibilidade de jogo não paga a atmosfera.
pub const FOG_REFERENCE_M: f32 = 340.0;
/// Densidade base (falloff quadrático): imperceptível a 100 m, véu claro a
/// 200 m, camadas separadas a 400 m, horizonte fundido a 800 m.
pub const FOG_BASE_DENSITY: f32 = 1.0 / FOG_REFERENCE_M;
/// Peso do tint do bioma na cor da névoa.
const BIOME_TINT_WEIGHT: f32 = 0.25;

/// Bioma atual do herói (id da região, ou `None` = vale central).
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]

#[type_path = "viber::ambient"]
pub struct CurrentBiome {
    pub id: Option<String>,
}

/// Converte a `fog-density` exponencial do XML num multiplicador sobre a
/// densidade base.
///
/// Os mundos autoraram 0.0014–0.0046 para `FogFalloff::Exponential`; o
/// falloff quadrático usa outra escala, por isso o valor do XML entra como
/// razão face à mediana autoral (0.0028) e é limitado — um bioma nunca pode
/// cegar o jogador nem apagar a atmosfera por completo.
pub fn fog_density_multiplier(xml_density: f32) -> f32 {
    if !xml_density.is_finite() || xml_density <= 0.0 {
        return 1.0;
    }
    (xml_density / 0.0028).clamp(0.55, 1.9)
}

/// Mistura linear entre duas cores RGB.
fn mix_rgb(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[allow(clippy::too_many_arguments)]

pub fn biome_fog_system(
    mut throttle: Local<f32>,
    // REAL: no modo Edit o `Time` virtual está em pausa (`pause_virtual_time`)
    // e o throttle nunca baixava — a névoa congelava no 1.º frame do editor,
    // mesmo depois de `set_clock`/mudar de bioma.
    time: Res<Time<bevy::time::Real>>,
    atmosphere: Res<crate::env::AtmosphereState>,
    weather: Option<Res<crate::env::WeatherState>>,
    players: Query<&GlobalTransform, With<Player>>,
    biomes: Option<Res<crate::env::BiomeRegions>>,
    scene: Option<Res<bevy_ext_core::InteriorSceneConfig>>,
    mut current: ResMut<CurrentBiome>,
    mut cameras: Query<Entity, With<Camera3d>>,
    mut commands: Commands,
    mut toasts: MessageWriter<ScriptToast>,
    // Estado suavizado da transição de bioma (mult, tint, peso do tint).
    mut biome_blend: Local<Option<(f32, [f32; 3], f32)>>,
) {
    // 4×/s: a paleta muda devagar (o dia inteiro dura 20 min reais) e a
    // névoa é um componente inserido, não um uniform barato.
    *throttle -= time.delta_secs();
    if *throttle > 0.0 {
        return;
    }
    *throttle = 0.25;

    // Dentro da bolsa de interior não há bioma: a cena está declaradamente
    // FORA do mundo (a 2 km do mapa) e a única razão para a `BiomeRegion` do
    // vale se aplicar lá era a posição no plano. Sem isto a sala herda a
    // névoa, a tinta e a exposição de um bioma onde não está — o exterior
    // pinta-se por cima do interior.
    let inside_scene =
        scene
            .as_deref()
            .zip(players.iter().next())
            .is_some_and(|(scene, player)| {
                let pos = player.translation();
                scene.contains(pos.x, pos.z)
            });
    let region = if inside_scene {
        None
    } else {
        match (biomes, players.iter().next()) {
            (Some(biomes), Some(player)) => {
                let pos = player.translation();
                biomes
                    .list
                    .iter()
                    .find(|b| point_in_polygon(pos.x, pos.z, &b.polygon))
                    .cloned()
            }
            _ => None,
        }
    };

    // Toast só na TRANSIÇÃO (o sistema agora corre sempre, não só ao entrar).
    // Dentro de uma cena de interior `region` é forçada a None (a bolsa fica
    // fora do mapa): isso não é "voltar ao vale" — entrar em casa não pode
    // anunciar que se voltou à região central (nem re-anunciar a região ao sair).
    let next_id = region.as_ref().map(|r| r.id.clone());
    if !inside_scene && next_id != current.id {
        match &next_id {
            Some(id) => {
                toasts.write(ScriptToast(format!("Entraste em: {id}")));
            }
            None => {
                if current.id.is_some() {
                    toasts.write(ScriptToast("De volta ao vale.".into()));
                }
            }
        }
        current.id = next_id;
    }

    let (density_mult, tint) = match &region {
        Some(r) => (fog_density_multiplier(r.fog_density), r.tint),
        None => (1.0, None),
    };

    // ── Transição SUAVE de bioma ──────────────────────────────────────────
    // A paleta por região aplicava-se de GOLPE ao cruzar o polígono — a
    // "linha diagonal" que cortava o ecrã de um lado ao outro. O alvo agora
    // persegue-se por lerp (~0.9 s a 4 Hz): o toast anuncia, o olho não vê
    // o corte. Primeira chamada: snap ao alvo.
    let target_mult = density_mult;
    let target_tint = tint.unwrap_or([1.0, 1.0, 1.0]);
    let target_weight = if tint.is_some() { 1.0 } else { 0.0 };
    let blend = biome_blend.get_or_insert((target_mult, target_tint, target_weight));
    const BLEND_K: f32 = 0.35;
    blend.0 += (target_mult - blend.0) * BLEND_K;
    for (b, t) in blend.1.iter_mut().zip(target_tint) {
        *b += (t - *b) * BLEND_K;
    }
    blend.2 += (target_weight - blend.2) * BLEND_K;
    let (smooth_mult, smooth_tint, smooth_weight) = *blend;

    // Cor: horizonte da hora + um toque do bioma, na MESMA escala do domo
    // (paleta raw — o r7 multiplicava por SKY_RADIANCE 400 e o fog ficava
    // branco estourado, r8 limitava a luminância). O cap 0.8×horizonte
    // mantém-se: silhuetas de serra lêem-se contra um fundo mais claro.
    let mut color = [atmosphere.fog[0], atmosphere.fog[1], atmosphere.fog[2]];
    if smooth_weight > 0.01 {
        color = mix_rgb(color, smooth_tint, BIOME_TINT_WEIGHT * smooth_weight);
    }
    let horizon_sky = [
        atmosphere.horizon[0],
        atmosphere.horizon[1],
        atmosphere.horizon[2],
    ];
    let lum = |c: [f32; 3]| 0.25 * c[0] + 0.5 * c[1] + 0.25 * c[2];
    let cap = lum(horizon_sky) * 0.8;
    let fog_lum = lum(color);
    if fog_lum > cap && fog_lum > 0.0 {
        let k = cap / fog_lum;
        color = [color[0] * k, color[1] * k, color[2] * k];
    }
    // À noite a névoa não pode "acender" o mundo: é o azul profundo que
    // engole o longe e deixa as fogueiras a valer ouro. Mas a névoa também é
    // VISIBILIDADE: abaixo de ~20 % do alcance ela deixa de separar camadas
    // e passa a ser um véu que apaga o que está longe, incluindo as luzes.
    // O +8 % antigo adensava-a exactamente na noite; agora o braço nocturno
    // ABRE 20 % (a base e o multiplicador do bioma continuam a mandar) e a
    // chuva soma só os 4 % de sempre.
    let rain = weather.map(|w| w.rain.clamp(0.0, 1.0)).unwrap_or(0.0);
    let density =
        FOG_BASE_DENSITY * smooth_mult * (1.0 - 0.20 * atmosphere.night) * (1.0 + 0.04 * rain);

    // Inscattering direcional — a escala NÃO é arbitrária, e errá-la lava o
    // frame inteiro de branco (r1/r2).
    //
    // O `pbr_functions.wgsl` calcula
    //   `scattering = pow(dot(view, sun), exponent) * light.color * view.exposure`
    // e `light.color` de uma direcional é `cor × ILLUMINANCE` — 10 000 lux no
    // default do Bevy. Com `view.exposure ≈ 1/(2^EV100 × 1.2)` (≈ 1e-3 a
    // EV100 9.7) o pico de `scattering` anda por ~10 unidades. Depois
    //   `fog_color = base_color + scattering × directional_light_color`
    // — ou seja, um `directional_light_color` de 0.5 somava **5.0** a uma cor
    // de névoa que vive em 0..1. Era isso que punha o mundo inteiro branco
    // no meio-dia.
    //
    // Aqui pede-se o EFEITO (quanto o glow soma à cor da névoa) e converte-se
    // para o valor do componente, compensando a exposição da hora.
    const SCATTER_AT_PEAK: f32 = 10.0;
    let inscatter = atmosphere.sun_tint;
    let target_add = (0.10 + 0.34 * atmosphere.golden) * atmosphere.day;
    let inscatter_strength =
        (target_add / (SCATTER_AT_PEAK * atmosphere.exposure_scale.max(0.1))).clamp(0.0, 0.08);

    let fog = DistanceFog {
        color: Color::LinearRgba(bevy::color::LinearRgba::rgb(color[0], color[1], color[2])),
        directional_light_color: Color::LinearRgba(bevy::color::LinearRgba::rgb(
            inscatter[0] * inscatter_strength,
            inscatter[1] * inscatter_strength,
            inscatter[2] * inscatter_strength,
        )),
        // Lobe de ~45°: largo o bastante para a metade do mundo virada ao
        // sol aquecer na hora dourada, estreito o bastante para não ser um
        // véu global (que é indistinguível de "subir a exposição").
        directional_light_exponent: 14.0,
        // Quadrático: a imagem perto fica intacta e o longe funde-se rápido —
        // é o perfil que dá "camadas" em vez de um véu uniforme.
        falloff: FogFalloff::ExponentialSquared { density },
    };
    for camera in &mut cameras {
        commands.entity(camera).insert(fog.clone());
    }
}

pub use crate::env::point_in_polygon;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_in_polygon_square() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(point_in_polygon(5.0, 5.0, &square));
        assert!(!point_in_polygon(15.0, 5.0, &square));
        assert!(!point_in_polygon(-1.0, -1.0, &square));
    }


    #[test]
    fn test_point_in_polygon_wedge() {
        // cunha do bioma norte (estilo environment.xml)
        let wedge = [
            [-56.0, 56.0],
            [56.0, 56.0],
            [4040.0, 4040.0],
            [-4040.0, 4040.0],
        ];
        assert!(point_in_polygon(0.0, 300.0, &wedge), "norte profundo");
        assert!(!point_in_polygon(0.0, 0.0, &wedge), "praça fora");
    }


    #[test]
    fn test_biome_fog_thickens_with_rain() {
        fn fog_density(rain: f32, night: f32) -> f32 {
            let mut app = bevy::app::App::new();
            app.add_plugins(bevy::MinimalPlugins);
            app.add_message::<ScriptToast>();
            app.insert_resource(CurrentBiome::default());
            app.insert_resource(crate::env::AtmosphereState {
                night,
                ..Default::default()
            });
            app.insert_resource(crate::env::WeatherState {
                wind: [0.0, 0.0],
                wind_strength: 0.0,
                clouds: 0.0,
                rain,
                cycle: false,
            });
            app.add_systems(bevy::app::Update, biome_fog_system);
            app.world_mut()
                .spawn((Player::default(), GlobalTransform::from_xyz(0.0, 0.0, 0.0)));
            app.world_mut().spawn((Camera3d::default(),));
            app.update();
            let camera = app
                .world_mut()
                .query_filtered::<&DistanceFog, With<Camera3d>>()
                .single(app.world())
                .expect("a névoa foi inserida na câmara")
                .clone();
            match camera.falloff {
                FogFalloff::ExponentialSquared { density } => density,
                other => panic!("falloff inesperado: {other:?}"),
            }
        }
        // A névoa é VISIBILIDADE: noite e chuva dão só um toque (eram +35 % e
        // +60 %, depois +12 % na chuva — ainda fechava o horizonte).
        let dry = fog_density(0.0, 0.0);
        let wet = fog_density(1.0, 0.0);
        assert!(
            (wet / dry - 1.04).abs() < 1e-4,
            "chuva cheia engrossa a névoa ×1.04 (subtil): {wet} vs {dry}"
        );
        let night = fog_density(0.0, 1.0);
        assert!(
            (night / dry - 0.80).abs() < 1e-4,
            "noite ABRE a névoa ×0.80 (visibilidade): {night} vs {dry}"
        );
    }
}
