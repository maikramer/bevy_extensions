//! Consultas de SUPERFÍCIE do mundo para os scripts (`viber.ground_below`,
//! `viber.road`, `viber.road_at`, `viber.on_road`, `viber.in_water`) —
//! exit4/F4.
//!
//! SEAM: o `bevy_ext_terrain_runtime` (estradas/água/leitor voxel) arrasta
//! render + avian + o terreno voxel inteiro — o `bevy_ext_luau` não o dep.
//! O trait [`SurfaceQueries`] é implementado pela INTEGRAÇÃO sobre os seus
//! tipos (o monólito hoje, o jogo depois) e publicado no
//! [`ScriptHostState::surfaces`] como `Arc<dyn SurfaceQueries>`; as closures
//! Lua leem o trait sem saber de terreno. [`RoadSample`] é o espelho PLANO
//! do `terrain_runtime::roads::RoadSample` (o `viber.road` devolve estes
//! campos; a integração converte — 10 números).

use std::sync::Arc;

/// Projeção na estrada mais próxima (`viber.road`/`viber.road_at`): o MESMO
/// corpo do `RoadSample` do terreno, plano (dados, sem lógica).
#[derive(Debug, Clone, Copy)]
pub struct RoadSample {
    /// Arco (m) do ponto na estrada.
    pub s: f32,
    /// Comprimento total da estrada (m).
    pub length: f32,
    /// Meia-largura da fita (m).
    pub half_width: f32,
    /// Desvio lateral assinado do centro (m).
    pub lateral: f32,
    /// O ponto está num vão SUSPENSO (o `y` é o tabuleiro, não o terreno).
    pub deck: bool,
    /// Cota do tabuleiro no vão.
    pub deck_y: f32,
    /// Heading da estrada no ponto (rad).
    pub heading: f32,
    /// Distância ao eixo (m).
    pub center_dist: f32,
    /// Centro do segmento (x, z) — a amostra do `road_at`.
    pub cx: f32,
    pub cz: f32,
}

/// Consultas de superfície publicadas pela integração (o mundo carvado não
/// muda depois do bootstrap — o Arc é construído uma vez).
pub trait SurfaceQueries: Send + Sync {
    /// A superfície sólida mais alta em ou abaixo de `y` nesta coluna
    /// (`viber.ground_below`): topo do mundo, piso de gruta, chão do vão.
    /// `None` = não há terreno sólido abaixo.
    fn ground_below(&self, x: f32, y: f32, z: f32) -> Option<f32>;
    /// Cota do topo renderizado na coluna (`viber.road` sem vão).
    fn ground_top(&self, x: f32, z: f32) -> Option<f32>;
    /// O ponto está na FITA de uma estrada (`viber.on_road`).
    fn on_road(&self, x: f32, z: f32) -> bool;
    /// O ponto está num corpo de água (`viber.in_water`).
    fn in_water(&self, x: f32, z: f32) -> bool;
    /// Projeção na estrada MAIS PRÓXIMA, mesmo fora da fita (`viber.road`).
    fn nearest_road(&self, x: f32, z: f32) -> Option<RoadSample>;
    /// Ponto na estrada mais longa a `s` metros de arco (`viber.road_at`).
    fn road_at(&self, s: f32) -> Option<RoadSample>;
}

/// O trait como app state publicado ([`ScriptHostState::surfaces`]).
pub type SurfaceQueriesHandle = Arc<dyn SurfaceQueries>;
