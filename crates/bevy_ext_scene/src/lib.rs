//! Componentes de cena tipados que substituem as strings do DSL do Viber.
//!
//! Um prop do Vael era um `WorldNode` com atributos de texto livre
//! (`collider="shape: cylinder; radius: 0.45; half-height: 2.2"`). Estes
//! tipos são `Reflect`: o Jackdaw edita-os campo a campo no inspector, as
//! extensões leem-nos sem parsers próprios e o `.bsn` fica legível.
//!
//! O runtime ainda é o parser do Viber, por isso cada tipo implementa
//! [`AttrSpec`]: o loader do `bevy_ext_world` converte o componente de volta
//! nos atributos que o parser entende. A migração só acontece quando a ida e
//! volta reproduz **exatamente** o texto original ([`lift`]) — um valor que
//! o tipo não sabe escrever igual fica no `WorldNode`, sem perdas.

use bevy::prelude::*;

pub mod nodes;
pub mod primitives;

pub use nodes::{
    AuthoredProp, AuthoredProps, FloraStation, ModelLod, ParticleEmitter, ParticleSpec, PortalSpec,
    ScriptRef, SpawnExclusionSpec,
};

/// Conversão sem perdas entre um componente e atributos do DSL.
pub trait AttrSpec: Sized {
    /// Chaves de atributo que o tipo absorve.
    const KEYS: &'static [&'static str];

    /// Lê os atributos (`get(chave)`); `None` se faltar algo obrigatório ou
    /// um valor não for reconhecido.
    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self>;

    /// Os atributos que o parser do Viber espera, na forma canónica.
    fn to_attrs(&self) -> Vec<(&'static str, String)>;
}

/// Lê `T` de `attrs` só se a volta (`to_attrs`) devolver exatamente os
/// mesmos pares que o tipo absorve. Devolve também as chaves consumidas.
pub fn lift<T: AttrSpec>(attrs: &[(String, String)]) -> Option<(T, Vec<&'static str>)> {
    let present: Vec<(&'static str, &str)> = T::KEYS
        .iter()
        .filter_map(|key| {
            attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| (*key, v.as_str()))
        })
        .collect();
    if present.is_empty() {
        return None;
    }
    let get = |key: &str| {
        present
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    };
    let value = T::from_attrs(&get)?;
    let mut back = value.to_attrs();
    let mut want: Vec<(&'static str, String)> =
        present.iter().map(|(k, v)| (*k, v.to_string())).collect();
    back.sort();
    want.sort();
    (back == want).then(|| (value, present.iter().map(|(k, _)| *k).collect()))
}

/// Pares `chave: valor` de uma string de componente (`a: 1; b: x`).
pub fn component_pairs(value: &str) -> Vec<(String, String)> {
    value
        .split(';')
        .filter(|part| !part.trim().is_empty())
        .filter_map(|part| {
            let (key, value) = part.split_once(':')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn component_string(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Número como o DSL o escreve (`1`, `0.45`).
pub fn num(value: f32) -> String {
    format!("{value}")
}

fn parse_num(value: &str) -> Option<f32> {
    value.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn parse_flag(value: &str) -> Option<bool> {
    match value.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

fn flag(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

// ---------------------------------------------------------------- collider

/// Forma do colisor (`collider="…"`).
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub enum ColliderShape {
    /// Sem colisor.
    #[default]
    None,
    /// Caixa das bounds renderizadas.
    Auto,
    /// Caixa explícita (tamanho total) com deslocamento vertical.
    Box { size: Vec3, offset_y: f32 },
    Sphere { radius: f32 },
    /// Cilindro no eixo Y.
    Cylinder { radius: f32, half_height: f32 },
    /// Cápsula no eixo Y.
    Capsule { radius: f32, half_height: f32 },
    /// Malha exata de um GLB de colisão; `base` assenta-a na base do modelo.
    Mesh { url: String, base: bool },
    /// Casco convexo cozinhado da malha de render (rochas, árvores).
    ConvexHull { url: String },
}

/// O colisor de um prop.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct ColliderSpec {
    pub shape: ColliderShape,
}

impl AttrSpec for ColliderSpec {
    const KEYS: &'static [&'static str] = &["collider"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let value = get("collider")?;
        let shape = match value.trim() {
            "none" => ColliderShape::None,
            "auto" => ColliderShape::Auto,
            text => {
                let pairs = component_pairs(text);
                let field = |key: &str| {
                    pairs
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.as_str())
                };
                match field("shape")? {
                    "box" => {
                        let size: Vec<f32> = field("size")?
                            .split_whitespace()
                            .map(parse_num)
                            .collect::<Option<_>>()?;
                        let [x, y, z] = size[..] else {
                            return None;
                        };
                        ColliderShape::Box {
                            size: Vec3::new(x, y, z),
                            offset_y: field("pos-offset-y").map_or(Some(0.0), parse_num)?,
                        }
                    }
                    "sphere" => ColliderShape::Sphere {
                        radius: parse_num(field("radius")?)?,
                    },
                    "cylinder" => ColliderShape::Cylinder {
                        radius: parse_num(field("radius")?)?,
                        half_height: parse_num(field("half-height")?)?,
                    },
                    "capsule" => ColliderShape::Capsule {
                        radius: parse_num(field("radius")?)?,
                        half_height: parse_num(field("half-height")?)?,
                    },
                    "trimesh" => ColliderShape::Mesh {
                        url: field("mesh-url")?.to_string(),
                        base: match field("mesh-anchor") {
                            None => false,
                            Some("base") => true,
                            Some(_) => return None,
                        },
                    },
                    "precompute" => ColliderShape::ConvexHull {
                        url: field("mesh-url")?.to_string(),
                    },
                    _ => return None,
                }
            }
        };
        Some(Self { shape })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let text = match &self.shape {
            ColliderShape::None => "none".to_string(),
            ColliderShape::Auto => "auto".to_string(),
            ColliderShape::Box { size, offset_y } => {
                let mut pairs = vec![
                    ("shape", "box".to_string()),
                    (
                        "size",
                        format!("{} {} {}", num(size.x), num(size.y), num(size.z)),
                    ),
                ];
                if *offset_y != 0.0 {
                    pairs.push(("pos-offset-y", num(*offset_y)));
                }
                component_string(&pairs)
            }
            ColliderShape::Sphere { radius } => {
                component_string(&[("shape", "sphere".into()), ("radius", num(*radius))])
            }
            ColliderShape::Cylinder {
                radius,
                half_height,
            } => component_string(&[
                ("shape", "cylinder".into()),
                ("radius", num(*radius)),
                ("half-height", num(*half_height)),
            ]),
            ColliderShape::Capsule {
                radius,
                half_height,
            } => component_string(&[
                ("shape", "capsule".into()),
                ("radius", num(*radius)),
                ("half-height", num(*half_height)),
            ]),
            ColliderShape::Mesh { url, base } => {
                let mut pairs = vec![("shape", "trimesh".to_string()), ("mesh-url", url.clone())];
                if *base {
                    pairs.push(("mesh-anchor", "base".into()));
                }
                component_string(&pairs)
            }
            ColliderShape::ConvexHull { url } => component_string(&[
                ("shape", "precompute".into()),
                ("mesh-url", url.clone()),
            ]),
        };
        vec![("collider", text)]
    }
}

impl ColliderSpec {
    /// Meia-extensão aproximada (para gizmos/preview), se a forma a souber.
    pub fn half_extents(&self) -> Option<Vec3> {
        match &self.shape {
            ColliderShape::Box { size, .. } => Some(*size * 0.5),
            ColliderShape::Sphere { radius } => Some(Vec3::splat(*radius)),
            ColliderShape::Cylinder {
                radius,
                half_height,
            } => Some(Vec3::new(*radius, *half_height, *radius)),
            ColliderShape::Capsule {
                radius,
                half_height,
            } => Some(Vec3::new(*radius, half_height + radius, *radius)),
            _ => None,
        }
    }
}

// -------------------------------------------------------------------- body

/// Tipo de corpo rígido.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub enum BodyKind {
    #[default]
    None,
    Fixed,
    Dynamic,
    Kinematic,
}

impl BodyKind {
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fixed => "fixed",
            Self::Dynamic => "dynamic",
            Self::Kinematic => "kinematic",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "none" => Self::None,
            "fixed" => Self::Fixed,
            "dynamic" => Self::Dynamic,
            "kinematic" => Self::Kinematic,
            _ => return None,
        })
    }
}

/// Corpo rígido (`rigidbody="type: …; mass: …; gravity-scale: …"`).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct BodySpec {
    pub kind: BodyKind,
    pub mass: f32,
    pub gravity_scale: f32,
}

impl AttrSpec for BodySpec {
    const KEYS: &'static [&'static str] = &["rigidbody"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let pairs = component_pairs(&get("rigidbody")?);
        let field = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        Some(Self {
            kind: BodyKind::parse(field("type")?)?,
            mass: parse_num(field("mass")?)?,
            gravity_scale: parse_num(field("gravity-scale")?)?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        vec![(
            "rigidbody",
            component_string(&[
                ("type", self.kind.name().into()),
                ("mass", num(self.mass)),
                ("gravity-scale", num(self.gravity_scale)),
            ]),
        )]
    }
}

// ------------------------------------------------------------ destructible

/// Prop que parte com golpes (`destructible="…"`): árvores, rochas, cactos.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct Destructible {
    /// Texto que salta ao partir ("Madeira", "Pedra").
    pub popup_text: String,
    /// Cor do texto (`#rrggbb`).
    pub popup_color: String,
    /// Partículas ao partir.
    pub preset: String,
    pub burst_count: Option<u32>,
    /// Golpes até partir.
    pub hits: u32,
    pub spark_on_hit: bool,
    /// Partículas a cada golpe.
    pub hit_preset: String,
    pub hit_burst_count: u32,
    pub shake_on_hit: Option<bool>,
    pub crack_on_hit: Option<bool>,
    pub crack_style: Option<String>,
    /// `fall` (tomba), `shatter` (estilhaça), `pick` (colhe).
    pub break_style: String,
    /// Altura do corte (fração) para `fall`.
    pub cut_height: Option<f32>,
    /// Prop que fica no lugar (ex.: arbusto sem bagas).
    pub replacement: Option<String>,
    pub replace_scale: Option<f32>,
}

const DESTRUCTIBLE_ORDER: [&str; 15] = [
    "popup-text",
    "popup-color",
    "preset",
    "burst-count",
    "hits",
    "spark-on-hit",
    "hit-preset",
    "hit-burst-count",
    "shake-on-hit",
    "crack-on-hit",
    "crack-style",
    "break-style",
    "cut-height",
    "replacement",
    "replace-scale",
];

impl AttrSpec for Destructible {
    const KEYS: &'static [&'static str] = &["destructible"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let pairs = component_pairs(&get("destructible")?);
        if pairs
            .iter()
            .any(|(key, _)| !DESTRUCTIBLE_ORDER.contains(&key.as_str()))
        {
            return None;
        }
        let field = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        let int = |key: &str| field(key).map(|v| v.parse::<u32>().ok());
        Some(Self {
            popup_text: field("popup-text")?,
            popup_color: field("popup-color")?,
            preset: field("preset")?,
            burst_count: int("burst-count").map_or(Some(None), |v| v.map(Some))?,
            hits: int("hits")??,
            spark_on_hit: parse_flag(&field("spark-on-hit")?)?,
            hit_preset: field("hit-preset")?,
            hit_burst_count: int("hit-burst-count")??,
            shake_on_hit: field("shake-on-hit").map_or(Some(None), |v| parse_flag(&v).map(Some))?,
            crack_on_hit: field("crack-on-hit").map_or(Some(None), |v| parse_flag(&v).map(Some))?,
            crack_style: field("crack-style"),
            break_style: field("break-style")?,
            cut_height: field("cut-height").map_or(Some(None), |v| parse_num(&v).map(Some))?,
            replacement: field("replacement"),
            replace_scale: field("replace-scale")
                .map_or(Some(None), |v| parse_num(&v).map(Some))?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("popup-text", self.popup_text.clone()),
            ("popup-color", self.popup_color.clone()),
            ("preset", self.preset.clone()),
        ];
        if let Some(count) = self.burst_count {
            pairs.push(("burst-count", count.to_string()));
        }
        pairs.push(("hits", self.hits.to_string()));
        pairs.push(("spark-on-hit", flag(self.spark_on_hit)));
        pairs.push(("hit-preset", self.hit_preset.clone()));
        pairs.push(("hit-burst-count", self.hit_burst_count.to_string()));
        if let Some(shake) = self.shake_on_hit {
            pairs.push(("shake-on-hit", flag(shake)));
        }
        if let Some(crack) = self.crack_on_hit {
            pairs.push(("crack-on-hit", flag(crack)));
        }
        if let Some(style) = &self.crack_style {
            pairs.push(("crack-style", style.clone()));
        }
        pairs.push(("break-style", self.break_style.clone()));
        if let Some(cut) = self.cut_height {
            pairs.push(("cut-height", num(cut)));
        }
        if let Some(replacement) = &self.replacement {
            pairs.push(("replacement", replacement.clone()));
        }
        if let Some(scale) = self.replace_scale {
            pairs.push(("replace-scale", num(scale)));
        }
        vec![("destructible", component_string(&pairs))]
    }
}

// ----------------------------------------------------------------- harvest

/// Recurso que o prop dá ao partir (`resource-kind` + `resource-yield`).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_scene"]
pub struct Harvestable {
    /// `wood`, `dark-wood`, `stone`, `berry`…
    pub kind: String,
    /// Unidades por colheita.
    pub amount: u32,
}

impl AttrSpec for Harvestable {
    const KEYS: &'static [&'static str] = &["resource-kind", "resource-yield"];

    fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            kind: get("resource-kind")?,
            amount: get("resource-yield")?.parse().ok()?,
        })
    }

    fn to_attrs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("resource-kind", self.kind.clone()),
            ("resource-yield", self.amount.to_string()),
        ]
    }
}

/// Os componentes tipados de um nó do mundo, juntos (o que o loader leu do
/// documento). O jogo anexa-os à entidade spawnada e lê-os em vez das props.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeSpecs {
    pub collider: Option<ColliderSpec>,
    pub body: Option<BodySpec>,
    pub destructible: Option<Destructible>,
    pub harvest: Option<Harvestable>,
    pub lod: Option<ModelLod>,
    pub flora: Option<FloraStation>,
    pub script: Option<ScriptRef>,
    pub props: Option<AuthoredProps>,
    pub particle: Option<ParticleSpec>,
    pub exclusion: Option<SpawnExclusionSpec>,
    pub portal: Option<PortalSpec>,
}

impl NodeSpecs {
    pub fn is_empty(&self) -> bool {
        self.collider.is_none()
            && self.body.is_none()
            && self.destructible.is_none()
            && self.harvest.is_none()
            && self.lod.is_none()
            && self.flora.is_none()
            && self.script.is_none()
            && self.props.is_none()
            && self.particle.is_none()
            && self.exclusion.is_none()
            && self.portal.is_none()
    }
}

/// Caminhos de tipo dos componentes deste crate (para o loader e o editor).
pub const TYPE_PATHS: [&str; 11] = [
    "bevy_ext_scene::ColliderSpec",
    "bevy_ext_scene::BodySpec",
    "bevy_ext_scene::Destructible",
    "bevy_ext_scene::Harvestable",
    "bevy_ext_scene::ModelLod",
    "bevy_ext_scene::FloraStation",
    "bevy_ext_scene::ScriptRef",
    "bevy_ext_scene::AuthoredProps",
    "bevy_ext_scene::ParticleSpec",
    "bevy_ext_scene::SpawnExclusionSpec",
    "bevy_ext_scene::PortalSpec",
];

/// Regista os tipos para reflexão (jogo, loader e editor).
pub struct SceneSpecPlugin;

impl Plugin for SceneSpecPlugin {
    fn build(&self, app: &mut App) {
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        register_types(&mut registry.write());
    }
}

/// Regista os tipos num registo avulso (loader sem `App`).
pub fn register_types(registry: &mut bevy::reflect::TypeRegistry) {
    registry.register::<ColliderSpec>();
    registry.register::<ColliderShape>();
    registry.register::<BodySpec>();
    registry.register::<BodyKind>();
    registry.register::<Destructible>();
    registry.register::<Harvestable>();
    registry.register::<ModelLod>();
    registry.register::<FloraStation>();
    registry.register::<ScriptRef>();
    registry.register::<AuthoredProp>();
    registry.register::<AuthoredProps>();
    registry.register::<ParticleEmitter>();
    registry.register::<ParticleSpec>();
    registry.register::<SpawnExclusionSpec>();
    registry.register::<PortalSpec>();
    registry.register::<Option<ParticleEmitter>>();
    registry.register::<Vec<AuthoredProp>>();
    registry.register::<Option<u32>>();
    registry.register::<Option<bool>>();
    registry.register::<Option<f32>>();
    registry.register::<Option<String>>();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn colliders_do_mundo_vao_e_voltam() {
        for text in [
            "shape: cylinder; radius: 0.45; half-height: 2.2",
            "shape: precompute; mesh-url: /assets/meshes/props/rock_mossy_lod0.glb",
            "shape: trimesh; mesh-url: /assets/meshes/village/torch_post_collision.glb; mesh-anchor: base",
            "shape: box; size: 0.8 0.8 0.8",
            "shape: box; size: 1 2 1; pos-offset-y: 0.5",
            "none",
            "auto",
        ] {
            let (spec, keys) = lift::<ColliderSpec>(&attrs(&[("collider", text)])).expect(text);
            assert_eq!(keys, ["collider"]);
            assert_eq!(spec.to_attrs()[0].1, text);
        }
    }

    #[test]
    fn forma_nao_canonica_fica_no_dsl() {
        // `0.80` voltaria como `0.8`: não migra.
        assert!(lift::<ColliderSpec>(&attrs(&[("collider", "shape: box; size: 0.80 1 1")])).is_none());
        assert!(lift::<ColliderSpec>(&attrs(&[("collider", "shape: cuboid; size: 1 1 1")])).is_none());
    }

    #[test]
    fn corpo_e_colheita() {
        let text = "type: fixed; mass: 0; gravity-scale: 0";
        let (body, _) = lift::<BodySpec>(&attrs(&[("rigidbody", text)])).unwrap();
        assert_eq!(body.kind, BodyKind::Fixed);
        let (harvest, keys) = lift::<Harvestable>(&attrs(&[
            ("resource-kind", "dark-wood"),
            ("resource-yield", "3"),
            ("other", "x"),
        ]))
        .unwrap();
        assert_eq!((harvest.kind.as_str(), harvest.amount), ("dark-wood", 3));
        assert_eq!(keys.len(), 2);
        assert!(lift::<Harvestable>(&attrs(&[("resource-kind", "wood")])).is_none());
    }

    #[test]
    fn destrutiveis_do_mundo() {
        for text in [
            "popup-text: Madeira Escura; popup-color: #1a3320; preset: leaves; burst-count: 24; hits: 3; spark-on-hit: 1; hit-preset: woodchips; hit-burst-count: 12; shake-on-hit: 1; crack-on-hit: 1; crack-style: vertical; break-style: fall; cut-height: 0.7",
            "popup-text: Pedra; popup-color: #cccccc; preset: dust; burst-count: 22; hits: 3; spark-on-hit: 1; hit-preset: rockshards; hit-burst-count: 10; crack-on-hit: 1; break-style: shatter",
            "popup-text: Frutas; popup-color: #d64040; preset: leaves; burst-count: 12; hits: 1; spark-on-hit: 0; hit-preset: leaves; hit-burst-count: 5; shake-on-hit: 1; break-style: pick; replacement: berry-bush-bare; replace-scale: 1",
        ] {
            let (spec, _) = lift::<Destructible>(&attrs(&[("destructible", text)])).expect(text);
            assert_eq!(spec.to_attrs()[0].1, text);
        }
    }
}
