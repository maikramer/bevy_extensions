//! Instrumentação de profiling do terreno — cópia local do contrato
//! `viber::profiler::{Group, timed}`.
//!
//! TODO(viber-exit): o `timed` real do monólito alimenta as tabelas do
//! profiler (`TimingStats` por [`Group`]); aqui é um passe-travessia — a API
//! e os pontos de instrumentação mantêm-se, os números não chegam às tabs
//! até o Viber pontuar (ou adotar esta crate e re-registar os sistemas
//! com o `timed` dele por cima).

use bevy::ecs::system::{IntoSystem, SystemInput};

/// Grupos de instrumentação do monólito. Só o subconjunto que o terreno usa
/// sobrevive aqui; os restantes são relevantes para o host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Sistemas do terreno (bootstrap, passe de LOD, edições vivas, tint).
    Terrain,
}

/// Embrulha um sistema com a instrumentação do grupo. Passe-travessia: o
/// sistema entra e sai igual (ver TODO no módulo).
pub fn timed<I, O, M, S>(group: Group, system: S) -> impl IntoSystem<I, O, M>
where
    I: SystemInput,
    S: IntoSystem<I, O, M>,
{
    let _ = group;
    system
}
