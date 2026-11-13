//! Shims de instrumentação — o profiling por anéis da engine (avg/min/max/
//! p95 por sistema numa janela de 120 frames) ainda não saiu do monólito.
//!
//! [`timed`] mantém a FORMA da chamada (`timed(Group::Ai, sistema)`) e
//! devolve o sistema sem embrulhar: sem medição, sem mudança de
//! comportamento — os anéis do profiler são observação, não lógica.
//!
//! TODO(viber-exit): quando o profiler sair do monólito (crate próprio),
//! trocar este módulo pelo `timed` real (que mede o `run_unsafe` e grava
//! no anel do grupo) — a troca é de um import, as chamadas ficam iguais.

use bevy::ecs::system::{IntoSystem, SystemInput};

/// Grupos do profiler da engine; este crate só usa [`Group::Ai`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Sistemas de IA (sentidos, FSM, integrador de locomoção).
    Ai,
}

/// Embrulha um sistema com medição — PLACEHOLDER sem medição (ver docs do
/// módulo). Mesma assinatura do `timed` da engine para a troca ser um
/// import: aceita qualquer `IntoSystem` (fn items de sistemas incluídos) e
/// devolve o sistema usável em tuples, `.chain()`, `.run_if()` e
/// `.after()`/`.before()` como o sistema original.
pub fn timed<I: SystemInput, O, M, S: IntoSystem<I, O, M>>(_group: Group, system: S) -> S {
    system
}
