//! Instrumentação de profiling — o núcleo [`timed`] movido do monólito
//! (`viber::profiler::timed`); o monólito reexporta o módulo e as suas tabs
//! (frames/world/physics/audio) continuam lá, a ler estes anéis globais.
//!
//! `timed(...)` embrulha qualquer `System` e mede o `run_unsafe` dele: mesma
//! assinatura, mesmo acesso, +2 leituras de relógio. Os registos vivem em
//! globais (sistemas correm em paralelo) numa janela deslizante por sistema;
//! o congelamento ([`timed::set_frozen`]) pára a aquisição mas preserva o
//! anel. Escopos dinâmicos (por script Luau) usam [`timed::record_script`].

pub mod timed;
pub mod trace;

pub use timed::{Group, TimingStats, timed};
