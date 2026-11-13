//! Instrumentação de profiling — cópia local do contrato
//! `viber::profiler::{Group, timed}` (o mesmo seam que
//! `bevy_ext_terrain_runtime::profiling` usa).
//!
//! O `timed` REAL (que alimenta os anéis de `TimingStats` por sistema, lidos
//! pelas tabs do profiler) vive em `bevy_ext_engine::profiler::timed`; o
//! monólito reexporta-o e embrulha lá os sistemas dele. Este crate não pode
//! dep o engine (o engine dep este para `PendingCollider`/`PhysicsSpec`),
//! por isso aqui é um passe-travessia: a API e os pontos de instrumentação
//! mantêm-se; quando o Viber adoptar os plugins DESTE crate, re-registar os
//! sistemas com o `timed` do engine por cima devolve os números às tabs.

use bevy::ecs::system::{IntoSystem, SystemInput};

/// Grupos de instrumentação da engine (mesma variante set do
/// `bevy_ext_engine::profiler::timed::Group`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Ai,
    Camera,
    Combat,
    Engine,
    /// FX físicos — knockback e queda de destrutíveis (`physics_fx`).
    Fx,
    Hud,
    Player,
    /// Streaming de colliders de terreno (`physics`).
    Physics,
    Render,
    Scripts,
    Terrain,
}

/// Embrulha um sistema com a instrumentação do grupo. Passe-travessia: o
/// sistema entra e sai igual (ver o doc do módulo).
pub fn timed<I, O, M, S>(group: Group, system: S) -> impl IntoSystem<I, O, M>
where
    I: SystemInput,
    S: IntoSystem<I, O, M>,
{
    let _ = group;
    system
}
