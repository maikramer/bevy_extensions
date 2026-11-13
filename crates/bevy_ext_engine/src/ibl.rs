//! Constantes de IBL — extraído do `ibl.rs` do monólito; o monólito
//! reexporta para `viber::ibl::IBL_INTENSITY` continuar a resolver.

/// Ganho global do IBL: 1.0 mantém o cubemap pintado como o autorado.
///
/// O knob histórico do port (o `IBL scale` do runtime TS era sempre 1); se
/// um dia se expuser ao QA, é aqui e no monólito (reexport).
pub const IBL_INTENSITY: f32 = 1.0;
