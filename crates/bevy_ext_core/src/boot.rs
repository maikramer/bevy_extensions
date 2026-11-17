//! Traço do arranque: quanto custa cada fase antes do primeiro frame.
//!
//! O `mundo pronto em X s` do véu começa a contar no primeiro `PostUpdate`, ou
//! seja **depois** do parse do `.bsn` com prefabs, do spawn do mundo, do
//! `bootstrap` do terreno e das pools de flora. Esses segundos existem mas não
//! apareciam em lado nenhum. Aqui cada fase chama [`mark`] e o relatório mostra
//! o tempo desde o arranque do processo e o delta desde a marca anterior:
//!
//! ```text
//! bevy_ext_core::boot::start();          // primeira linha do main
//! let world = load_world(path)?;
//! bevy_ext_core::boot::mark("world.parse");
//! ```
//!
//! O custo é um `Instant::now()` e um `push` por marca (uma dúzia no arranque),
//! por isso está sempre ligado — uma medição que depende de uma env var acaba
//! por não existir. `VAEL_BOOT_TRACE=0` cala o relatório, não a medição.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::Serialize;

/// Uma fase medida desde o arranque do processo.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BootMark {
    /// Nome da fase (`world.parse`, `terrain.bootstrap`, …).
    pub name: &'static str,
    /// Milissegundos desde [`start`].
    pub ms: f32,
    /// Milissegundos desde a marca anterior.
    pub delta: f32,
}

static START: OnceLock<Instant> = OnceLock::new();
static MARKS: Mutex<Vec<BootMark>> = Mutex::new(Vec::new());

/// Arranca o relógio. Idempotente: a primeira chamada manda.
pub fn start() {
    START.get_or_init(Instant::now);
}

/// `true` quando o relatório deve ser impresso (`VAEL_BOOT_TRACE=0` desliga).
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !std::env::var("VAEL_BOOT_TRACE").is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "off"))
    })
}

/// Marca uma fase. Antes de [`start`] o relógio começa aqui.
pub fn mark(name: &'static str) {
    let start = *START.get_or_init(Instant::now);
    let ms = start.elapsed().as_secs_f32() * 1000.0;
    let mut marks = MARKS.lock().unwrap_or_else(|poison| poison.into_inner());
    let previous = marks.last().map_or(0.0, |mark| mark.ms);
    marks.push(BootMark {
        name,
        ms,
        // Uma fase marcada fora de ordem não pode aparecer como negativa: o
        // que interessa é o total desde o arranque.
        delta: (ms - previous).max(0.0),
    });
}

/// As marcas até agora, por ordem de registo.
pub fn marks() -> Vec<BootMark> {
    MARKS.lock().unwrap_or_else(|poison| poison.into_inner()).clone()
}

/// Total desde [`start`] até à última marca.
pub fn elapsed_ms() -> f32 {
    marks().last().map_or(0.0, |mark| mark.ms)
}

/// O relatório numa linha: `boot: world.parse 1418 ms (+1418) · …`.
pub fn report() -> String {
    let marks = marks();
    if marks.is_empty() {
        return "boot: nenhuma marca".to_string();
    }
    let mut out = String::from("boot:");
    for mark in &marks {
        out.push_str(&format!(
            " {} {:.0} ms (+{:.0}) ·",
            mark.name, mark.ms, mark.delta
        ));
    }
    out.push_str(&format!(" total {:.0} ms", elapsed_ms()));
    out
}

/// Esquece as marcas (testes).
pub fn reset() {
    MARKS.lock().unwrap_or_else(|poison| poison.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, because the trace is process-global state: three `#[test]`
    /// functions would reset and push into the same vector from different
    /// threads.
    #[test]
    fn trace_records_marks_in_order() {
        reset();
        assert!(report().contains("nenhuma marca"));

        start();
        mark("a");
        std::thread::sleep(std::time::Duration::from_millis(2));
        mark("b");
        let rows = marks();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[1].name, "b");
        assert!(rows[1].ms >= rows[0].ms, "monotonic");
        assert!(rows[1].delta >= 1.0, "the sleep shows up in the delta");
        assert!(elapsed_ms() >= rows[1].ms);
        let text = report();
        assert!(text.contains("a") && text.contains("b"), "{text}");
        assert!(text.contains("total"), "{text}");

        // A mark pushed out of order cannot report a negative delta.
        {
            let mut rows = MARKS.lock().unwrap();
            rows.push(BootMark {
                name: "future",
                ms: 1e9,
                delta: 1e9,
            });
        }
        mark("now");
        let rows = marks();
        assert!(rows.last().expect("a mark").delta >= 0.0, "{rows:?}");
    }
}
