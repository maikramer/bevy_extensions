//! Contratos do terreno e do layout dos campos.
//!
//! O crate não conhece o terreno do jogo: o jogo fornece um [`GrassTerrain`]
//! (altura, regras de sítio e bioma) como `Arc` num recurso [`GrassProvider`],
//! e um [`GrassFieldLayout`] que decide os retângulos onde a relva cresce. O
//! `bevy_grass` só vê heightmaps e máscaras de densidade; este módulo é a
//! fronteira entre os dois mundos.

use std::sync::Arc;

use bevy::math::{UVec2, Vec2};
use bevy::prelude::Resource;

/// Identificador de bioma do jogo (0..N). O crate não impõe uma enum: o
/// layout e os `GrassStyleSpec` usam o mesmo id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GrassBiomeId(pub u16);

/// O que o terreno diz de um ponto (XZ do mundo).
#[derive(Clone, Copy, Debug)]
pub struct GrassSite {
    /// Água, estrada, pad, teto fino, falésia, exclusão ou fora do mundo.
    pub blocked: bool,
    /// Cobertura base `0..1` (clareiras, adensamentos, noise do jogo).
    pub coverage: f32,
    pub biome: GrassBiomeId,
}

impl GrassSite {
    /// Nada cresce aqui.
    pub const BLOCKED: Self = Self {
        blocked: true,
        coverage: 0.0,
        biome: GrassBiomeId(0),
    };

    /// Relva com `coverage` (0..1) neste bioma.
    #[must_use]
    pub fn grass(coverage: f32, biome: GrassBiomeId) -> Self {
        Self {
            blocked: false,
            coverage: coverage.clamp(0.0, 1.0),
            biome,
        }
    }
}

/// O terreno do jogo, visto pelo bake. Os métodos são chamados no
/// `AsyncComputeTaskPool` (milhões de vezes num mapa de 4 km), por isso têm de
/// ser baratos e thread-safe.
pub trait GrassTerrain: Send + Sync + 'static {
    /// Lado do mundo (metros); o bake cobre `±world_size/2`.
    fn world_size(&self) -> f32;
    /// Sobe quando as alturas mudam (edições vivas do terreno).
    fn revision(&self) -> u64;
    /// Altura do topo do terreno (metros).
    fn height(&self, x: f32, z: f32) -> f32;
    /// Regras de sítio e bioma neste ponto.
    fn site(&self, x: f32, z: f32, ground: f32) -> GrassSite;
    /// Nº de biomas com região autoral. `0` = mundo sem `<BiomeRegion>`.
    fn biome_count(&self) -> u16 {
        0
    }
}

/// Um retângulo de relva rodado em Y, centrado em `center` (mundo, XZ).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldPlan {
    pub biome: GrassBiomeId,
    pub center: Vec2,
    pub yaw: f32,
    pub size: Vec2,
}

impl FieldPlan {
    /// Resolução das imagens (heightmap e máscara) a `cell` metros por texel.
    #[must_use]
    pub fn texels(&self, cell: f32) -> UVec2 {
        let tx = (self.size.x / cell.max(1e-3)).ceil().max(1.0) as u32;
        let tz = (self.size.y / cell.max(1e-3)).ceil().max(1.0) as u32;
        UVec2::new(tx.clamp(1, 4096), tz.clamp(1, 4096))
    }

    /// Tamanho de um texel (X, Z).
    #[must_use]
    pub fn cell(&self, texels: UVec2) -> Vec2 {
        self.size / texels.as_vec2()
    }

    /// Centro local (relativo ao centro do campo) do texel `(ix, iz)`.
    #[must_use]
    pub fn local_point(&self, texels: UVec2, ix: u32, iz: u32) -> Vec2 {
        (Vec2::new(ix as f32, iz as f32) + 0.5) * self.cell(texels) - self.size * 0.5
    }

    /// Local → mundo (rotação Y do `Transform` do campo).
    #[must_use]
    pub fn world_point(&self, local: Vec2) -> Vec2 {
        let (sin, cos) = self.yaw.sin_cos();
        Vec2::new(
            self.center.x + local.x * cos + local.y * sin,
            self.center.y - local.x * sin + local.y * cos,
        )
    }

    /// Mundo → local (inversa da rotação).
    #[must_use]
    pub fn local_of(&self, world: Vec2) -> Vec2 {
        let (sin, cos) = self.yaw.sin_cos();
        let d = world - self.center;
        Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
    }

    /// O ponto do mundo cai no retângulo?
    #[must_use]
    pub fn contains(&self, world: Vec2) -> bool {
        let local = self.local_of(world);
        local.x.abs() <= self.size.x * 0.5 && local.y.abs() <= self.size.y * 0.5
    }
}

/// Decide os campos a partir do terreno (ex.: um por bioma, um único campo,
/// malha de células…). Corre na main thread ao pedir um bake.
pub trait GrassFieldLayout: Send + Sync + 'static {
    fn fields(&self, terrain: &dyn GrassTerrain) -> Vec<FieldPlan>;
}

/// O layout mais simples: um campo do bioma 0 a cobrir o mundo inteiro.
#[derive(Debug, Default, Clone, Copy)]
pub struct SingleFieldLayout;

impl GrassFieldLayout for SingleFieldLayout {
    fn fields(&self, terrain: &dyn GrassTerrain) -> Vec<FieldPlan> {
        vec![FieldPlan {
            biome: GrassBiomeId(0),
            center: Vec2::ZERO,
            yaw: 0.0,
            size: Vec2::splat(terrain.world_size().max(1.0)),
        }]
    }
}

/// O terreno em uso. O jogo substitui este recurso quando muda de cena/mundo;
/// a troca do `Arc` (ou uma `revision()` nova) dispara um bake.
#[derive(Resource, Clone)]
pub struct GrassProvider(pub Arc<dyn GrassTerrain>);

impl std::ops::Deref for GrassProvider {
    type Target = dyn GrassTerrain;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

/// O layout em uso (fixo por configuração do plugin).
#[derive(Resource, Clone)]
pub struct GrassLayout(pub Arc<dyn GrassFieldLayout>);

impl std::ops::Deref for GrassLayout {
    type Target = dyn GrassFieldLayout;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_plan_roundtrips_local_and_world() {
        let plan = FieldPlan {
            biome: GrassBiomeId(2),
            center: Vec2::new(100.0, -40.0),
            yaw: -std::f32::consts::FRAC_PI_4,
            size: Vec2::splat(200.0),
        };
        for (x, z) in [(-99.0, -99.0), (0.0, 0.0), (50.0, -80.0), (99.0, 99.0)] {
            let local = Vec2::new(x, z);
            let back = plan.local_of(plan.world_point(local));
            assert!(
                (back - local).length() < 1e-3,
                "ida-e-volta falhou: {local:?} -> {back:?}"
            );
        }
        let texels = plan.texels(2.0);
        assert_eq!(texels, UVec2::splat(100));
        assert!((plan.cell(texels) - Vec2::splat(2.0)).length() < 1e-5);
    }

    #[test]
    fn single_layout_covers_the_world() {
        struct Flat(f32);
        impl GrassTerrain for Flat {
            fn world_size(&self) -> f32 {
                self.0
            }
            fn revision(&self) -> u64 {
                0
            }
            fn height(&self, _x: f32, _z: f32) -> f32 {
                0.0
            }
            fn site(&self, _x: f32, _z: f32, _g: f32) -> GrassSite {
                GrassSite::grass(1.0, GrassBiomeId(0))
            }
        }
        let plans = SingleFieldLayout.fields(&Flat(512.0));
        assert_eq!(plans.len(), 1);
        assert!(plans[0].contains(Vec2::new(255.0, -255.0)));
        assert!(!plans[0].contains(Vec2::new(300.0, 0.0)));
    }
}
