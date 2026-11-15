//! Contrato de troca de cena — o schedule que (re)carrega uma cena.
//!
//! Extraído do `scene_loader.rs` do monólito; o monólito reexporta para
//! `viber::scene_loader::SceneLoad` continuar a resolver. O ScheduleLabel É o
//! contrato: sistemas do monólito (spawn do mundo) e deste crate (relva,
//! cinemáticas) entram no MESMO schedule — um tipo só, definido aqui.

use bevy::ecs::schedule::ScheduleLabel;

/// O schedule que (re)carrega uma cena: os mesmos sistemas do `Startup` que
/// montam o mundo.
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneLoad;

