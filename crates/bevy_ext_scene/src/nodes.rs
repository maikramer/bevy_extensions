//! Componentes tipados para o resto do DSL dos nós do mundo: LOD dos modelos,
//! estações de flora, scripts e as suas `prop-*`, partículas, exclusões de
//! spawn e portais de cena.
//!
//! Mesmo contrato do resto do crate: [`AttrSpec`] com ida e volta exata; o
//! que não volta igual fica no `WorldNode`.

use bevy::prelude::*;

use crate::{AttrSpec, component_pairs, num};

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

// ------------------------------------------------------------------- LOD

/// Escada de LOD de um `GltfScene` (`lod1-url`, `lod2-url`,
/// `lod-threshold-near/mid`, em metros).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct ModelLod {
    pub lod1: Option<String>,
    pub lod2: Option<String>,
    /// Distância a partir da qual entra o LOD1.
    pub near: Option<f32>,
    /// Distância a partir da qual entra o LOD2.
    pub mid: Option<f32>,
}

impl AttrSpec for ModelLod {
    const KEYS: &'static [&'static str] =
        &["lod1-url", "lod2-url", "lod-threshold-near", "lod-threshold-mid"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            lod1: get("lod1-url"),
            lod2: get("lod2-url"),
            near: opt_num(get, "lod-threshold-near")?,
            mid: opt_num(get, "lod-threshold-mid")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(url) = &self.lod1 {
            out.push(("lod1-url", url.clone()));
        }
        if let Some(url) = &self.lod2 {
            out.push(("lod2-url", url.clone()));
        }
        if let Some(near) = self.near {
            out.push(("lod-threshold-near", num(near)));
        }
        if let Some(mid) = self.mid {
            out.push(("lod-threshold-mid", num(mid)));
        }
        out
    }
}

// ----------------------------------------------------------------- flora

/// Estação de árvore procedural (`prop-flora`): a espécie do `bevy_flora`.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct FloraStation {
    pub species: String,
}

impl AttrSpec for FloraStation {
    const KEYS: &'static [&'static str] = &["prop-flora"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            species: get("prop-flora")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        vec![("prop-flora", self.species.clone())]
    }
}

// ---------------------------------------------------------------- script

/// Script Luau do nó (`script`, relativo a `scripts/`).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct ScriptRef {
    pub path: String,
}

impl AttrSpec for ScriptRef {
    const KEYS: &'static [&'static str] = &["script"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            path: get("script")?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        vec![("script", self.path.clone())]
    }
}

/// Uma `prop-<chave>` autoral (a chave sem o prefixo).
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct AuthoredProp {
    pub key: String,
    pub value: String,
}

/// As `prop-*` de um nó (lidas pelos scripts), menos `prop-flora`, que é
/// [`FloraStation`]. Não é um [`AttrSpec`]: as chaves são abertas.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct AuthoredProps {
    pub props: Vec<AuthoredProp>,
}

impl AuthoredProps {
    /// Prefixo das props no DSL.
    pub const PREFIX: &'static str = "prop-";

    /// As `prop-*` de `attrs` (ordem preservada), menos as reservadas.
    pub fn from_attrs(attrs: &[(String, String)]) -> Option<(Self, Vec<String>)> {
        let mut props = Vec::new();
        let mut keys = Vec::new();
        for (key, value) in attrs {
            let Some(name) = key.strip_prefix(Self::PREFIX) else {
                continue;
            };
            if name == "flora" || name.is_empty() {
                continue;
            }
            props.push(AuthoredProp {
                key: name.to_string(),
                value: value.clone(),
            });
            keys.push(key.clone());
        }
        (!props.is_empty()).then_some((Self { props }, keys))
    }

    pub fn to_attrs(&self) -> Vec<(String, String)> {
        self.props
            .iter()
            .map(|prop| (format!("{}{}", Self::PREFIX, prop.key), prop.value.clone()))
            .collect()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|prop| prop.key == key)
            .map(|prop| prop.value.as_str())
    }
}

// ------------------------------------------------------------- particles

/// O emissor de um `ParticleSystem` (`particle-emitter="preset: fire; …"`).
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct ParticleEmitter {
    pub preset: String,
    pub emission_rate: f32,
    pub shape_radius: Option<f32>,
    pub start_life: Vec2,
    pub start_speed: Vec2,
    pub start_size: Vec2,
    pub start_color: Option<String>,
    pub looping: Option<bool>,
    pub world_space: Option<bool>,
}

const EMITTER_KEYS: [&str; 12] = [
    "preset",
    "emission-rate",
    "shape-radius",
    "start-life-min",
    "start-life-max",
    "start-speed-min",
    "start-speed-max",
    "start-size-min",
    "start-size-max",
    "start-color",
    "looping",
    "world-space",
];

fn flag(value: &str) -> Option<bool> {
    match value {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

impl ParticleEmitter {
    fn parse(text: &str) -> Option<Self> {
        let pairs = component_pairs(text);
        if pairs
            .iter()
            .any(|(key, _)| !EMITTER_KEYS.contains(&key.as_str()))
        {
            return None;
        }
        let field = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        let n = |key: &str| field(key).as_deref().and_then(parse_num);
        let opt = |key: &str| match field(key) {
            None => Some(None),
            Some(v) => parse_num(&v).map(Some),
        };
        let opt_flag = |key: &str| match field(key) {
            None => Some(None),
            Some(v) => flag(&v).map(Some),
        };
        Some(Self {
            preset: field("preset")?,
            emission_rate: n("emission-rate")?,
            shape_radius: opt("shape-radius")?,
            start_life: Vec2::new(n("start-life-min")?, n("start-life-max")?),
            start_speed: Vec2::new(n("start-speed-min")?, n("start-speed-max")?),
            start_size: Vec2::new(n("start-size-min")?, n("start-size-max")?),
            start_color: field("start-color"),
            looping: opt_flag("looping")?,
            world_space: opt_flag("world-space")?,
        })
    }

    fn write(&self) -> String {
        let mut pairs: Vec<(&str, String)> = vec![
            ("preset", self.preset.clone()),
            ("emission-rate", num(self.emission_rate)),
        ];
        if let Some(radius) = self.shape_radius {
            pairs.push(("shape-radius", num(radius)));
        }
        pairs.extend([
            ("start-life-min", num(self.start_life.x)),
            ("start-life-max", num(self.start_life.y)),
            ("start-speed-min", num(self.start_speed.x)),
            ("start-speed-max", num(self.start_speed.y)),
            ("start-size-min", num(self.start_size.x)),
            ("start-size-max", num(self.start_size.y)),
        ]);
        if let Some(color) = &self.start_color {
            pairs.push(("start-color", color.clone()));
        }
        let bit = |b: bool| if b { "1" } else { "0" }.to_string();
        if let Some(looping) = self.looping {
            pairs.push(("looping", bit(looping)));
        }
        if let Some(world) = self.world_space {
            pairs.push(("world-space", bit(world)));
        }
        pairs
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Sistema de partículas: o preset do nó e o emissor detalhado.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct ParticleSpec {
    pub preset: Option<String>,
    pub emitter: Option<ParticleEmitter>,
}

impl AttrSpec for ParticleSpec {
    const KEYS: &'static [&'static str] = &["preset", "particle-emitter"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            preset: get("preset"),
            emitter: match get("particle-emitter") {
                None => None,
                Some(text) => Some(ParticleEmitter::parse(&text)?),
            },
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(preset) = &self.preset {
            out.push(("preset", preset.clone()));
        }
        if let Some(emitter) = &self.emitter {
            out.push(("particle-emitter", emitter.write()));
        }
        out
    }
}

// ------------------------------------------------------------- exclusion

/// Zona sem spawns (`SpawnExclusion at="x z" radius="r"`).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct SpawnExclusionSpec {
    /// Centro em XZ (mundo).
    pub at: Vec2,
    pub radius: f32,
}

impl AttrSpec for SpawnExclusionSpec {
    const KEYS: &'static [&'static str] = &["at", "radius"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let [x, z] = floats::<2>(&get("at")?)?;
        Some(Self {
            at: Vec2::new(x, z),
            radius: parse_num(&get("radius")?)?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("at", format!("{} {}", num(self.at.x), num(self.at.y))),
            ("radius", num(self.radius)),
        ]
    }
}

// ---------------------------------------------------------------- portal

/// Porta para outra cena (`ScenePortal`).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct PortalSpec {
    /// Cena de destino (`main`, `interiors/forge`…).
    pub target: String,
    /// `SceneEntry` de chegada (`@back` volta à de onde se veio).
    pub entry: Option<String>,
    pub prompt: Option<String>,
    pub key: Option<String>,
    pub sound: Option<String>,
    pub toast: Option<String>,
    /// Raio de interação.
    pub radius: Option<f32>,
    /// Raio que atravessa sem tecla.
    pub auto_radius: Option<f32>,
    pub arrive_offset: Option<String>,
}

impl AttrSpec for PortalSpec {
    const KEYS: &'static [&'static str] = &[
        "target",
        "entry",
        "prompt",
        "key",
        "sound",
        "toast",
        "radius",
        "auto-radius",
        "arrive-offset",
    ];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            target: get("target")?,
            entry: get("entry"),
            prompt: get("prompt"),
            key: get("key"),
            sound: get("sound"),
            toast: get("toast"),
            radius: opt_num(get, "radius")?,
            auto_radius: opt_num(get, "auto-radius")?,
            arrive_offset: get("arrive-offset"),
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("target", self.target.clone())];
        let mut text = |key: &'static str, value: &Option<String>| {
            if let Some(value) = value {
                out.push((key, value.clone()));
            }
        };
        text("entry", &self.entry);
        text("prompt", &self.prompt);
        text("key", &self.key);
        text("sound", &self.sound);
        text("toast", &self.toast);
        text("arrive-offset", &self.arrive_offset);
        if let Some(radius) = self.radius {
            out.push(("radius", num(radius)));
        }
        if let Some(radius) = self.auto_radius {
            out.push(("auto-radius", num(radius)));
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

    #[test]
    fn lod_e_flora() {
        let (lod, keys) = lift::<ModelLod>(&attrs(&[
            ("url", "/a_lod0.glb"),
            ("lod1-url", "/a_lod1.glb"),
            ("lod2-url", "/a_lod2.glb"),
            ("lod-threshold-near", "28"),
            ("lod-threshold-mid", "70"),
        ]))
        .unwrap();
        assert_eq!(keys.len(), 4);
        assert_eq!(lod.near, Some(28.0));
        assert!(lift::<FloraStation>(&attrs(&[("prop-flora", "pine_dark")])).is_some());
    }

    #[test]
    fn particulas_do_mundo() {
        for text in [
            "preset: fire; emission-rate: 18; shape-radius: 0.06; start-life-min: 0.3; start-life-max: 0.8; start-speed-min: 0.9; start-speed-max: 1.7; start-size-min: 0.14; start-size-max: 0.3; looping: 1; world-space: 1",
            "preset: smoke; emission-rate: 9; start-life-min: 5.5; start-life-max: 8.5; start-speed-min: 0.2; start-speed-max: 0.45; start-size-min: 0.84; start-size-max: 1.86; start-color: #cfe8de; looping: 1; world-space: 1",
        ] {
            let (spec, _) = lift::<ParticleSpec>(&attrs(&[("preset", "campfire"), ("particle-emitter", text)]))
                .expect(text);
            assert_eq!(spec.to_attrs()[1].1, text);
        }
        // `8.0` voltaria como `8` dentro de um texto compacto (`preset: …;
        // start-life-max: 8`), e a comparação por valor só cobre listas de
        // números: fica no DSL.
        assert!(lift::<ParticleSpec>(&attrs(&[(
            "particle-emitter",
            "preset: smoke; emission-rate: 9; start-life-min: 5.5; start-life-max: 8.0; start-speed-min: 0.2; start-speed-max: 0.45; start-size-min: 0.84; start-size-max: 1.86"
        )]))
        .is_none());
    }

    #[test]
    fn exclusao_portal_e_props() {
        let (ex, _) = lift::<SpawnExclusionSpec>(&attrs(&[("at", "140 360"), ("radius", "18")])).unwrap();
        assert_eq!(ex.at, Vec2::new(140.0, 360.0));
        let (portal, _) = lift::<PortalSpec>(&attrs(&[
            ("target", "interiors/chapel"),
            ("entry", "from-town"),
            ("prompt", "Entrar"),
            ("sound", "door_open"),
            ("toast", "Entras na capela."),
            ("auto-radius", "1.6"),
        ]))
        .unwrap();
        assert_eq!(portal.auto_radius, Some(1.6));
        let (props, keys) = AuthoredProps::from_attrs(&attrs(&[
            ("prop-flora", "oak"),
            ("prop-xp", "5"),
            ("prop-prompt", "Abrir"),
        ]))
        .unwrap();
        assert_eq!(keys, ["prop-xp", "prop-prompt"]);
        assert_eq!(props.get("xp"), Some("5"));
    }
}
