//! Cercas procedurais para Bevy.
//!
//! Uma cerca é um `<Entity>` com [`bevy_ext_scene::FenceSpec`]: a linha
//! (`fence-points`) e, por chave, uma predefinição ([`FenceStyle`]) com as
//! regras — tipo, altura, postes, travessas, tábuas (quantas e com que
//! comprimento variável), paliçada, varas entretecidas, treliça, cruzetas,
//! cordas ou portão. O [`FencePlugin`] lê as cercas do mundo e gera a
//! geometria em quatro passos, todos testáveis sem GPU:
//!
//! 1. [`layout`] — a linha vira postes e peças (puro, determinístico);
//! 2. [`mesh`] — as peças viram malhas em facetas planas, com UV em metros e
//!    desgaste na base;
//! 3. [`material`] — um `StandardMaterial` por conjunto de texturas do pool
//!    (`/assets/textures/<conjunto>/{albedo,normal,ao,roughness}.ktx2`);
//! 4. [`plugin`] — nasce em `Added<FenceSpec>`, refaz em `Changed`, e com a
//!    feature `physics` põe na entidade um colisor de caixas (um por vão).
//!
//! O chão vem do host pelo trait [`FenceGround`] (o jogo amostra o terreno; o
//! editor pode não ter nenhum). As definições (`FenceStyle`/`FenceSettings`)
//! são componentes `Reflect` editáveis no Jackdaw; o jogo recebe-os como
//! recursos da cena e copia-os para [`FenceStyleSet`] e [`FenceSettingsValue`].
//!
//! ```
//! use bevy_ext_fence::{FenceFrame, Rng, layout, mesh, specs};
//! use bevy_ext_scene::FenceSpec;
//!
//! let spec = FenceSpec { points: "0 0 6 0".into(), ..Default::default() };
//! let style = specs::resolve(&spec, &specs::pasture_rail());
//! let fence = layout::layout(&spec, &style, &FenceFrame::default(), &|_, _| Some(0.0), 7).unwrap();
//! assert_eq!(fence.spans.len(), 2, "6 m são dois vãos de 3 m");
//! assert_eq!(mesh::build(&fence, &style).len(), 1, "uma malha: só há madeira");
//! let _: Rng = Rng::new(7);
//! ```

pub mod layout;
pub mod material;
pub mod mesh;
pub mod plugin;
pub mod points;
pub mod specs;

/// O contrato de uma cerca autorada, re-exportado para o host só precisar deste crate.
pub use bevy_ext_scene::FenceSpec;

pub use layout::{FenceFrame, FenceLayout, Ground, LayoutError, Post, Rng, Span};
pub use plugin::{
    ColliderMode, FenceBuilt, FenceGround, FenceGroundProvider, FencePart, FencePlugin, FenceSettingsValue,
    FenceStyleSet, collider_pieces, frame_of, seed_key,
};
#[cfg(feature = "physics")]
pub use plugin::{FenceBody, FenceCollider};
pub use specs::{
    BoardTop, Brace, FenceKind, FenceSettings, FenceSettingsPlugin, FenceStyle, SlopeFollow, Timber,
    TimberShape, find, presets, resolve,
};
