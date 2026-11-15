//! Contrato do HOST com a água: corpos, leito e foco.
//!
//! A crate não conhece heightfield, carve nem cena. Um jogo implementa
//! [`WaterSource`] no seu recurso de mundo (no Vael é o `TerrainRuntime`) e
//! publica os obstáculos já cortados na lâmina em [`WaterObstacles`]. A
//! coleta desses obstáculos (Rapier, avian, voxel, malha autoral) é do host.

use bevy::prelude::*;

use crate::body::{WaterBody, WaterDomainSample};
use crate::obstacles::{self, WaterObstacle};

/// Mundo de água do host: a lista de corpos (lagos/rios) e o leito por baixo.
///
/// Implementado como `Resource` para os plugins genéricos
/// (`WaterWavePlugin<S>`, `WaterFxPlugin<S>`, `WaterBuoyancyPlugin<S>`)
/// lerem tudo de um recurso só, sem clones por frame.
pub trait WaterSource: Resource {
    /// Corpos registados (lagos e rios), na ordem de consulta.
    fn bodies(&self) -> &[WaterBody];

    /// Cota do leito em `(x, z)`; `None` fora do heightfield/terreno.
    fn bed_y(&self, x: f32, z: f32) -> Option<f32>;

    /// Identidade da lista de corpos — o solver rasters só quando muda.
    /// O default hasha comprimento + âncoras; um host com mundo estável pode
    /// devolver uma constante.
    fn signature(&self) -> u64 {
        crate::waves::bodies_signature(self.bodies())
    }

    /// Amostra hidrodinâmica no ponto, já com a corrente deflectida pelos
    /// obstáculos. `None` em terra/fora da lâmina.
    fn domain_at(&self, p: Vec2, obstacles: &[WaterObstacle]) -> Option<WaterDomainSample> {
        let bed = self.bed_y(p.x, p.y);
        let mut best: Option<WaterDomainSample> = None;
        for body in self.bodies() {
            if let Some(sample) = body.sample_domain(p, bed, 1.5) {
                if best.is_none_or(|b| sample.surface_y > b.surface_y) {
                    best = Some(sample);
                }
            }
        }
        best.map(|mut sample| {
            sample.flow = obstacles::deflect_flow_by_obstacles(p, sample.flow, obstacles);
            sample
        })
    }

    /// Cota da superfície (sem ondas) em `p`; a mais alta quando os corpos se
    /// sobrepõem.
    fn surface_at(&self, p: Vec2) -> Option<f32> {
        self.bodies()
            .iter()
            .filter(|b| b.contains(p))
            .filter_map(|b| b.surface_y_at(p))
            .fold(None, |acc: Option<f32>, y| Some(acc.map_or(y, |a| a.max(y))))
    }
}

/// Foco da janela rolante do solver (128 m). Sem isto o plugin usa a câmara
/// que desenha para a janela; o jogo põe aqui a posição do herói.
#[derive(Resource, Debug, Default, Clone)]
pub struct WaterFocus(pub Option<Vec2>);

impl WaterFocus {
    /// Foca a janela num XZ de mundo.
    pub fn set(&mut self, x: f32, z: f32) {
        self.0 = Some(Vec2::new(x, z));
    }
}

/// Obstáculos do escoamento publicados pelo host neste frame.
///
/// O host incrementa `generation` quando a lista muda (collider acrescentado,
/// movido ou removido); o solver só re-bakeia o `obstacle_tex` quando a
/// geração muda. Os obstáculos devem chegar já cortados no plano da lâmina
/// (ver [`WaterObstacle`]).
#[derive(Resource, Debug, Default, Clone)]
pub struct WaterObstacles {
    pub obstacles: Vec<WaterObstacle>,
    /// Contador de versão — qualquer mudança na lista tem de o incrementar.
    pub generation: u64,
}

impl WaterObstacles {
    /// Substitui a lista e invalida o bake.
    pub fn set(&mut self, obstacles: Vec<WaterObstacle>) {
        self.obstacles = obstacles;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Marca a lista como mudada sem a substituir (ex.: collider movido).
    pub fn touch(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}
