//! Contrato de chão para o integrador de locomoção.
//!
//! O serviço de IA não conhece o terreno: amostra o chão por este contrato —
//! o padrão dos serviços que não podem depender do crate de terreno. Quem
//! monta a App embrulha a
//! sua fonte de alturas num [`Ground`]; sem o recurso o integrador deixa o
//! Y como está e não clampeia ao disco do mundo — o comportamento headless
//! dos testes.
//!
//! TODO(viber-exit): quando o runtime de terreno sair do monólito
//! (bevy_ext_terrain), implementar [`GroundQuery`] para ele e apagar este
//! desvio — `TerrainRuntime::spec.world_size`, `in_field` e `surface_below`
//! são exatamente os três métodos.

use std::sync::Arc;

use bevy::prelude::*;

/// Fonte de alturas do mundo para a IA.
pub trait GroundQuery: Send + Sync + 'static {
    /// Raio do disco do mundo para o clamp da borda; `None` = sem limite.
    fn world_radius(&self) -> Option<f32>;

    /// `(x, z)` está dentro do heightfield? Fora dele os samplers saturam
    /// para a borda — não é o chão da criatura.
    fn in_field(&self, x: f32, z: f32) -> bool;

    /// Piso sólido mais alto na coluna `(x, z)` EM OU ABAIXO de `from_y`.
    ///
    /// `None` = não há piso sólido na coluna (o destino está enterrado em
    /// rocha que desce até ao fundo): o passo é recusado e a criatura
    /// encosta, em vez de ser teletransportada para o topo do mundo.
    fn surface_below(&self, x: f32, z: f32, from_y: f32) -> Option<f32>;
}

/// Recurso-ponte para a fonte de chão da App. O objeto de traço vive num
/// [`Arc`] porque o trait-object não tem `TypeId` próprio para o ECS.
#[derive(Resource, Clone, Default)]
pub struct Ground(Option<Arc<dyn GroundQuery>>);

impl Ground {
    /// Embrulha uma fonte de chão (o runtime de terreno do jogo).
    pub fn new(source: impl GroundQuery) -> Self {
        Self(Some(Arc::new(source)))
    }

    /// A fonte de alturas, quando a App ligou uma.
    pub fn get(&self) -> Option<&dyn GroundQuery> {
        self.0.as_deref()
    }
}
