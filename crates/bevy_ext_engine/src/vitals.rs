//! Saúde GENÉRICA de entidade (`Health`) e helpers puros de dano/cura.
//!
//! Definição CANÓNICA no `bevy_ext_core::vitals` (TypePaths `viber::vitals`
//! congelados lá) — a IA (`bevy_ext_ai`) e o monólito (`viber::vitals`)
//! re-exportam o MESMO módulo, por isso quem attacha `Health` no runtime
//! (spawner de criaturas, spawn autoral, FSM) espelha um só tipo no fim.

pub use bevy_ext_core::vitals::*;
