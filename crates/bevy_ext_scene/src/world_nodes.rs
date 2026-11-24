//! Componentes tipados da câmara, do áudio, dos biomas e das cercas.
//!
//! Mesmo contrato do resto do crate: [`AttrSpec`] com ida e volta exata; o
//! que não volta igual fica no `WorldNode`. A migração é por tag
//! (`lift_specs`): `at`, `size`, `id` e `radius` também existem noutras tags
//! e um lift global estragava o mundo. As chaves `fence-*` só existem na
//! cerca, por isso a [`FenceSpec`] levanta em qualquer tag.

use bevy::prelude::*;

use crate::{AttrSpec, num};

fn parse_num(value: &str) -> Option<f32> {
    value.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn opt_num(get: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<Option<f32>> {
    match get(key) {
        None => Some(None),
        Some(value) => parse_num(&value).map(Some),
    }
}

fn floats<const N: usize>(text: &str) -> Option<[f32; N]> {
    let values: Vec<f32> = text
        .split_whitespace()
        .map(parse_num)
        .collect::<Option<_>>()?;
    values.try_into().ok()
}

fn vec2_attr(value: Vec2) -> String {
    format!("{} {}", num(value.x), num(value.y))
}

// ----------------------------------------------------------------- camera

/// Câmara de jogo (`<OrbitCamera>`/`<ThirdPersonCamera>`): o follow, a pose
/// e a lente. O comportamento continua no runtime; o inspector passa a ter
/// campos em vez de texto livre.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct CameraSpec {
    /// Distância ao alvo (m).
    pub distance: Option<f32>,
    /// Altura do olho sobre o alvo (m).
    pub height: Option<f32>,
    /// Inclinação inicial (graus).
    pub pitch: Option<f32>,
    /// FOV vertical (graus).
    pub fov: Option<f32>,
    /// Atraso do follow (s).
    pub follow_lag: Option<f32>,
    /// Atraso do yaw (s).
    pub turn_lag: Option<f32>,
    /// Distância mínima ao terreno (m).
    pub min_terrain_distance: Option<f32>,
    /// `name` do alvo: follow rígido em vez de terceira pessoa.
    pub target: Option<String>,
    /// Sensibilidade do rato (rad/px; hoje só o campo).
    pub mouse_sensitivity: Option<f32>,
}

impl AttrSpec for CameraSpec {
    const KEYS: &'static [&'static str] = &[
        "distance",
        "height",
        "pitch",
        "fov",
        "follow-lag",
        "turn-lag",
        "min-terrain-distance",
        "target",
        "mouse-sensitivity",
    ];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            distance: opt_num(get, "distance")?,
            height: opt_num(get, "height")?,
            pitch: opt_num(get, "pitch")?,
            fov: opt_num(get, "fov")?,
            follow_lag: opt_num(get, "follow-lag")?,
            turn_lag: opt_num(get, "turn-lag")?,
            min_terrain_distance: opt_num(get, "min-terrain-distance")?,
            target: get("target"),
            mouse_sensitivity: opt_num(get, "mouse-sensitivity")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (key, value) in [
            ("distance", self.distance),
            ("height", self.height),
            ("pitch", self.pitch),
            ("fov", self.fov),
            ("follow-lag", self.follow_lag),
            ("turn-lag", self.turn_lag),
            ("min-terrain-distance", self.min_terrain_distance),
            ("mouse-sensitivity", self.mouse_sensitivity),
        ] {
            if let Some(value) = value {
                out.push((key, num(value)));
            }
        }
        if let Some(target) = &self.target {
            out.push(("target", target.clone()));
        }
        out
    }
}

// --------------------------------------------------------- cena de interior

/// Sala de interior (`<InteriorScene>`): a área e a câmara fixa por ecrã.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct InteriorSceneSpec {
    /// Centro da sala em XZ (mundo).
    pub at: Vec2,
    /// Tamanho da sala (m).
    pub size: Vec2,
    /// Distância da câmara ao chão (m).
    pub camera_distance: Option<f32>,
    /// Inclinação da câmara (graus).
    pub camera_pitch: Option<f32>,
    /// Yaw da câmara (graus).
    pub camera_yaw: Option<f32>,
}

impl AttrSpec for InteriorSceneSpec {
    const KEYS: &'static [&'static str] = &[
        "at",
        "size",
        "camera-distance",
        "camera-pitch",
        "camera-yaw",
    ];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let [ax, az] = floats::<2>(&get("at")?)?;
        let [sx, sz] = floats::<2>(&get("size")?)?;
        Some(Self {
            at: Vec2::new(ax, az),
            size: Vec2::new(sx, sz),
            camera_distance: opt_num(get, "camera-distance")?,
            camera_pitch: opt_num(get, "camera-pitch")?,
            camera_yaw: opt_num(get, "camera-yaw")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("at", vec2_attr(self.at)), ("size", vec2_attr(self.size))];
        for (key, value) in [
            ("camera-distance", self.camera_distance),
            ("camera-pitch", self.camera_pitch),
            ("camera-yaw", self.camera_yaw),
        ] {
            if let Some(value) = value {
                out.push((key, num(value)));
            }
        }
        out
    }
}

// ------------------------------------------------------------------ áudio

/// Volumes das buses (`<AudioMixer>`).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct AudioMixerSpec {
    pub master: Option<f32>,
    pub music: Option<f32>,
    pub sfx: Option<f32>,
    pub voice: Option<f32>,
    pub ambience: Option<f32>,
}

impl AttrSpec for AudioMixerSpec {
    const KEYS: &'static [&'static str] = &["master", "music", "sfx", "voice", "ambience"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            master: opt_num(get, "master")?,
            music: opt_num(get, "music")?,
            sfx: opt_num(get, "sfx")?,
            voice: opt_num(get, "voice")?,
            ambience: opt_num(get, "ambience")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (key, value) in [
            ("master", self.master),
            ("music", self.music),
            ("sfx", self.sfx),
            ("voice", self.voice),
            ("ambience", self.ambience),
        ] {
            if let Some(value) = value {
                out.push((key, num(value)));
            }
        }
        out
    }
}

/// Uma camada de música (`<MusicLayer>`): o `sound` do banco e o volume base.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct MusicLayerSpec {
    /// Nome da camada (`explore`, `battle`, …).
    pub layer: String,
    /// Id do som no banco.
    pub sound: Option<String>,
    pub base_volume: Option<f32>,
}

impl AttrSpec for MusicLayerSpec {
    const KEYS: &'static [&'static str] = &["layer", "sound", "base-volume"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            layer: get("layer")?,
            sound: get("sound"),
            base_volume: opt_num(get, "base-volume")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("layer", self.layer.clone())];
        if let Some(sound) = &self.sound {
            out.push(("sound", sound.clone()));
        }
        if let Some(volume) = self.base_volume {
            out.push(("base-volume", num(volume)));
        }
        out
    }
}

/// Uma região de bioma (`<BiomeRegion>`): o polígono e o que ele muda no
/// ambiente. O polígono fica texto cru (`[x,z;x,z;…]`) para a ida e volta ser
/// exata — quem precisa de pontos parseia-o (a extensão do Jackdaw fá-lo).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct BiomeRegionSpec {
    pub id: String,
    pub display_name: Option<String>,
    pub polygon: Option<String>,
    /// Cor de tinta em `#rrggbb` (texto cru, não normalizado).
    pub tint: Option<String>,
    pub fog_density: Option<f32>,
    pub pp_exposure: Option<f32>,
    pub pp_bloom_strength: Option<f32>,
    pub rain_scale: Option<f32>,
}

impl AttrSpec for BiomeRegionSpec {
    const KEYS: &'static [&'static str] = &[
        "id",
        "display-name",
        "polygon",
        "tint",
        "fog-density",
        "pp-exposure",
        "pp-bloom-strength",
        "rain-scale",
    ];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            id: get("id")?,
            display_name: get("display-name"),
            polygon: get("polygon"),
            tint: get("tint"),
            fog_density: opt_num(get, "fog-density")?,
            pp_exposure: opt_num(get, "pp-exposure")?,
            pp_bloom_strength: opt_num(get, "pp-bloom-strength")?,
            rain_scale: opt_num(get, "rain-scale")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("id", self.id.clone())];
        for (key, value) in [
            ("display-name", &self.display_name),
            ("polygon", &self.polygon),
            ("tint", &self.tint),
        ] {
            if let Some(value) = value {
                out.push((key, value.clone()));
            }
        }
        for (key, value) in [
            ("fog-density", self.fog_density),
            ("pp-exposure", self.pp_exposure),
            ("pp-bloom-strength", self.pp_bloom_strength),
            ("rain-scale", self.rain_scale),
        ] {
            if let Some(value) = value {
                out.push((key, num(value)));
            }
        }
        out
    }
}

// ------------------------------------------------------------------- cerca

/// Cerca procedural (`fence-*` num `<Entity>`): a linha e as regras que
/// geram a geometria (`bevy_ext_fence`).
///
/// `points` é a polilinha em texto cru (`"x z; x z; …"`, ou `x y z`), local à
/// entidade — a ida e volta é exata e quem precisa de pontos parseia-o. O
/// resto são sobreposições da predefinição (`FenceStyle` de
/// `world/settings/fences.bsn`): `None` é "o do estilo".
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct FenceSpec {
    /// Chave da predefinição (vazio = a predefinição por omissão).
    pub style: Option<String>,
    /// Polilinha `"x z; x z; …"` (texto cru, local à entidade).
    pub points: String,
    /// Fecha o perímetro (do último ponto ao primeiro).
    pub closed: Option<bool>,
    /// Tipo de peças: `rail`, `plank`, `picket`, `palisade`, `wattle`,
    /// `lattice`, `crossbuck`, `rope` ou `gate`.
    pub kind: Option<String>,
    /// Altura ao topo do poste (m).
    pub height: Option<f32>,
    /// Travessas horizontais por vão.
    pub rails: Option<u32>,
    /// Tábuas verticais por vão.
    pub boards: Option<u32>,
    /// Comprimento da tábua, mínimo e máximo (m): cada tábua sorteia dentro.
    pub board_length: Option<Vec2>,
    /// Distância máxima entre postes (m).
    pub spacing: Option<f32>,
    /// Conjunto de texturas (`oak_hewn`, `pine_dark`, …).
    pub material: Option<String>,
    /// Semente da variação (por omissão deriva do nome).
    pub seed: Option<u32>,
    /// Colisor: `segment`, `posts` ou `none`.
    pub collider: Option<String>,
    /// Cota dos postes: `terrain` (amostra o chão) ou `points` (a autorada).
    pub follow: Option<String>,
    /// Abertura da folha do portão em graus (0 = fechado); só os portões.
    pub open: Option<f32>,
}

fn opt_uint(get: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<Option<u32>> {
    match get(key) {
        None => Some(None),
        Some(value) => value.trim().parse::<u32>().ok().map(Some),
    }
}

fn opt_flag(get: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<Option<bool>> {
    match get(key).as_deref().map(str::trim) {
        None => Some(None),
        Some("1") => Some(Some(true)),
        Some("0") => Some(Some(false)),
        Some(_) => None,
    }
}

impl AttrSpec for FenceSpec {
    const KEYS: &'static [&'static str] = &[
        "fence-style",
        "fence-points",
        "fence-closed",
        "fence-kind",
        "fence-height",
        "fence-rails",
        "fence-boards",
        "fence-board-length",
        "fence-spacing",
        "fence-material",
        "fence-seed",
        "fence-collider",
        "fence-follow",
        "fence-open",
    ];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            style: get("fence-style"),
            // Sem linha não há cerca: os `fence-*` ficam no `WorldNode`.
            points: get("fence-points")?,
            closed: opt_flag(get, "fence-closed")?,
            kind: get("fence-kind"),
            height: opt_num(get, "fence-height")?,
            rails: opt_uint(get, "fence-rails")?,
            boards: opt_uint(get, "fence-boards")?,
            board_length: match get("fence-board-length") {
                None => None,
                Some(value) => {
                    let [min, max] = floats::<2>(&value)?;
                    Some(Vec2::new(min, max))
                }
            },
            spacing: opt_num(get, "fence-spacing")?,
            material: get("fence-material"),
            seed: opt_uint(get, "fence-seed")?,
            collider: get("fence-collider"),
            follow: get("fence-follow"),
            open: opt_num(get, "fence-open")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (key, value) in [
            ("fence-style", &self.style),
            ("fence-kind", &self.kind),
            ("fence-material", &self.material),
            ("fence-collider", &self.collider),
            ("fence-follow", &self.follow),
        ] {
            if let Some(value) = value {
                out.push((key, value.clone()));
            }
        }
        out.push(("fence-points", self.points.clone()));
        if let Some(closed) = self.closed {
            out.push(("fence-closed", if closed { "1" } else { "0" }.to_string()));
        }
        for (key, value) in [
            ("fence-height", self.height),
            ("fence-spacing", self.spacing),
            ("fence-open", self.open),
        ] {
            if let Some(value) = value {
                out.push((key, num(value)));
            }
        }
        for (key, value) in [
            ("fence-rails", self.rails),
            ("fence-boards", self.boards),
            ("fence-seed", self.seed),
        ] {
            if let Some(value) = value {
                out.push((key, value.to_string()));
            }
        }
        if let Some(length) = self.board_length {
            out.push(("fence-board-length", vec2_attr(length)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lift;

    fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// `to_attrs` devolve chaves `&'static str`; o teste compara com o texto.
    fn written(spec: &impl AttrSpec) -> Vec<(String, String)> {
        spec.to_attrs()
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    #[test]
    fn camara_do_jogo_vao_e_volta() {
        let input = attrs(&[
            ("distance", "3.3"),
            ("height", "1.55"),
            ("pitch", "18.3"),
            ("fov", "64"),
            ("follow-lag", "0.18"),
            ("turn-lag", "0.38"),
            ("min-terrain-distance", "1"),
        ]);
        let (spec, keys) = lift::<CameraSpec>(&input).unwrap();
        assert_eq!(keys.len(), 7);
        assert_eq!(spec.distance, Some(3.3));
        assert_eq!(written(&spec), input);
    }

    #[test]
    fn camara_com_alvo_e_sensibilidade() {
        // Ordem canónica do `to_attrs`: os números primeiro, o `target` no fim.
        let input = attrs(&[("distance", "4"), ("mouse-sensitivity", "0.005"), ("target", "player")]);
        let (spec, _) = lift::<CameraSpec>(&input).unwrap();
        assert_eq!(spec.target.as_deref(), Some("player"));
        assert_eq!(written(&spec), input);
    }

    #[test]
    fn sala_de_interior_vao_e_volta() {
        let input = attrs(&[
            ("at", "0 0"),
            ("size", "60 55"),
            ("camera-distance", "16"),
            ("camera-pitch", "58"),
            ("camera-yaw", "0"),
        ]);
        let (spec, keys) = lift::<InteriorSceneSpec>(&input).unwrap();
        assert_eq!(keys.len(), 5);
        assert_eq!(spec.at, Vec2::ZERO);
        assert_eq!(spec.size, Vec2::new(60.0, 55.0));
        assert_eq!(written(&spec), input);
    }

    #[test]
    fn interior_com_room_size_fica_parcial() {
        // `room-size` não é absorvido: o par at/size volta igual e o resto
        // fica no `WorldNode`.
        let input = attrs(&[("at", "4 8"), ("size", "20 20"), ("room-size", "16 12")]);
        let (spec, keys) = lift::<InteriorSceneSpec>(&input).unwrap();
        assert_eq!(keys, ["at", "size"]);
        assert_eq!(spec.size, Vec2::new(20.0, 20.0));
    }

    #[test]
    fn mixer_e_camadas_de_musica() {
        let mixer = attrs(&[
            ("master", "1"),
            ("music", "0.7"),
            ("sfx", "0.8"),
            ("voice", "0.9"),
            ("ambience", "0.7"),
        ]);
        let (spec, keys) = lift::<AudioMixerSpec>(&mixer).unwrap();
        assert_eq!(keys.len(), 5);
        assert_eq!(spec.music, Some(0.7));
        assert_eq!(written(&spec), mixer);

        let layer = attrs(&[
            ("layer", "explore"),
            ("sound", "bgm-explore"),
            ("base-volume", "0.18"),
        ]);
        let (spec, _) = lift::<MusicLayerSpec>(&layer).unwrap();
        assert_eq!(spec.layer, "explore");
        assert_eq!(written(&spec), layer);
    }

    #[test]
    fn bioma_do_mundo_vao_e_volta() {
        let input = attrs(&[
            ("id", "dark-forest"),
            ("display-name", "Floresta Sombria"),
            ("polygon", "[-56,56;56,56;4040,4040;-4040,4040]"),
            ("tint", "#1a3320"),
            ("fog-density", "0.0008"),
            ("pp-exposure", "0.78"),
        ]);
        let (spec, keys) = lift::<BiomeRegionSpec>(&input).unwrap();
        assert_eq!(keys.len(), 6);
        assert_eq!(spec.polygon.as_deref(), Some("[-56,56;56,56;4040,4040;-4040,4040]"));
        assert_eq!(written(&spec), input);
    }

    #[test]
    fn cerca_vao_e_volta() {
        // A ordem canónica do `to_attrs` não é a do documento; o `lift`
        // compara os pares ordenados, por isso a volta é exata na mesma.
        let input = attrs(&[
            ("name", "cerca-oeste"),
            ("fence-points", "-24.6 0 0; 5.9 0 0"),
            ("fence-style", "pasture_rail"),
            ("fence-height", "1.2"),
            ("fence-boards", "9"),
            ("fence-board-length", "0.9 1.1"),
            ("fence-closed", "1"),
            ("fence-seed", "7"),
            ("fence-open", "95"),
        ]);
        let (spec, keys) = lift::<FenceSpec>(&input).unwrap();
        assert_eq!(keys.len(), 8, "o `name` não é da cerca");
        assert_eq!(spec.style.as_deref(), Some("pasture_rail"));
        assert_eq!(spec.height, Some(1.2));
        assert_eq!(spec.boards, Some(9));
        assert_eq!(spec.board_length, Some(Vec2::new(0.9, 1.1)));
        assert_eq!(spec.closed, Some(true));
        assert_eq!(spec.seed, Some(7));
        assert_eq!(spec.open, Some(95.0));
        assert_eq!(spec.rails, None);
        let back = written(&spec);
        for (key, value) in &input[1..] {
            assert!(back.contains(&(key.to_string(), value.to_string())), "falta {key}");
        }
        assert_eq!(back.len(), 8);
    }

    #[test]
    fn cerca_parcial_so_com_a_linha() {
        let (spec, keys) = lift::<FenceSpec>(&attrs(&[("fence-points", "0 0 3 0")])).unwrap();
        assert_eq!(keys, ["fence-points"]);
        assert_eq!(spec.style, None);
        assert_eq!(written(&spec), attrs(&[("fence-points", "0 0 3 0")]));
    }

    #[test]
    fn cerca_sem_linha_nao_levanta() {
        // Um `fence-*` órfão fica no `WorldNode`: não há cerca sem linha.
        assert!(lift::<FenceSpec>(&attrs(&[("fence-style", "wattle")])).is_none());
        // Um valor que o tipo não sabe escrever igual também não levanta.
        assert!(lift::<FenceSpec>(&attrs(&[("fence-points", "0 0"), ("fence-closed", "true")])).is_none());
    }
}
