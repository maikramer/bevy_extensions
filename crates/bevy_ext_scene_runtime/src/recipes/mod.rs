//! Ponte ao IR do parse do mundo — **transitória** (TODO(viber-exit)).
//!
//! O spawner ([`spawn`]) foi extraído do monólito; o IR que ele consome
//! (`EntitySpec`/`EntityKind`/`ParsedWorld` e o `parse_world`) ainda vive no
//! `viber::recipes` (7k linhas de parser DSL). Os re-exports em baixo são
//! exatamente o que o spawner usa; quando o parser sair do `viber` para um
//! crate próprio (ou para o `bevy_ext_world`), trocar a origem aqui — os
//! corpos do spawner não mudam.
//!
//! `Shape`/`MaterialSpec`/`PrimitivePart` NÃO vivem aqui: o contrato de
//! primitivas é o [`bevy_ext_scene::primitives`] e o `viber` já o
//! re-exporta; o render procedural é o `bevy_ext_gradmesh` (lado do jogo).

pub use viber::recipes::{
    EntityKind, EntitySpec, MaterialSpec, ParsedWorld, ParticleSpec, Shape, StaticSpawnerSpec,
    TemplateLod, TransformSpec, UiStyleSource,
    DEFAULT_CLIFF_MARGIN, DEFAULT_LOD_MID, DEFAULT_LOD_NEAR, DEFAULT_NEAR_WATER_RADIUS,
    parse_world, sun_shadow_cascades, sun_shadow_distance,
};
// O `viber::recipes` importa `XmlNode` a privado (`use`, não `pub use`);
// o caminho público é o re-export do módulo xml.
pub use viber::xml::XmlNode;

pub use viber::recipes::transform;

pub mod spawn;
