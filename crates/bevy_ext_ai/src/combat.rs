//! Marcador de hit-react (flinch ao levar dano).
//!
//! Movido ao `bevy_ext_core` (exit4/G1): as vias de dano inserem, o
//! hit-react system do host consome e os sentidos o observam para
//! sintetizar audição — UM tipo por contrato. O caminho de reflexão
//! histórico (`viber::animation::HitReact`) fica congelado no core.

pub use bevy_ext_core::HitReact;
