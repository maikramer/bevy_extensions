//! Marcadores de streaming partilhados entre o terreno, o LOD e a cinemática.
//!
//! Vivem no `bevy_ext_core` porque três crates os leem e escrevem e nenhuma
//! delas é dona das outras: a cinemática (`bevy_ext_engine`) liga o boost, o
//! terreno (`bevy_ext_terrain_runtime`) consome-o e o jogo marca quando o
//! mundo ainda não é jogável. Antes havia duas definições separadas com o mesmo
//! `type_path` — o terreno lia um recurso que ninguém inseria, e o ramo de
//! orçamento ×10 nunca corria.

use bevy::prelude::*;

/// Orçamento de streaming multiplicado (terreno, LOD de render, relva).
///
/// Ligado enquanto a cinemática espera pelo cenário, depois de um respawn e —
/// desde que o jogo marque [`LoadingStreaming`] — enquanto o véu está em cima.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::cinematic"]
pub struct StreamingBoost(pub bool);

/// `true` enquanto o mundo ainda não está jogável (o véu em cima).
///
/// A carga do mundo é o momento em que o terreno mais tem para construir: no
/// arranque medido, a segunda vaga de colunas segurava o véu ~5 s ao orçamento
/// normal de 4 caixas/frame. O jogo escreve isto a partir do `WorldReadiness`;
/// o terreno e o LOD leem-no como se fosse um boost.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::cinematic"]
pub struct LoadingStreaming(pub bool);
