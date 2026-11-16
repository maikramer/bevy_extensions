//! Cronómetro RAII para diagnóstico de performance.
//!
//! Desligado por omissão: só mede com `VAEL_PERF_DIAG=1` (a mesma variável do
//! `VaelPerfDiagPlugin` do jogo). Sem a env o [`Probe::new`] não lê o relógio e
//! o [`record`] sai antes do `Mutex` — os sistemas quentes podem ficar
//! instrumentados permanentemente sem custo quando ninguém está a medir.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: OnceLock<bool> = OnceLock::new();

/// `VAEL_PERF_DIAG=1` liga os cronómetros deste módulo.
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var("VAEL_PERF_DIAG").is_ok_and(|v| v == "1"))
}

static ACC: Mutex<Option<HashMap<&'static str, (f64, u32)>>> = Mutex::new(None);

/// Mede o tempo até ao `drop` (uma chamada de sistema, um bloco).
pub struct Probe(&'static str, Option<Instant>);

impl Probe {
    pub fn new(name: &'static str) -> Self {
        Probe(name, enabled().then(Instant::now))
    }
}

/// Acumula uma amostra e imprime a média por chamada a cada 120.
pub fn record(name: &'static str, ms: f64) {
    if !enabled() {
        return;
    }
    let mut guard = ACC.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let e = map.entry(name).or_insert((0.0, 0));
    e.0 += ms;
    e.1 += 1;
    if e.1 >= 120 {
        eprintln!("PROBE {name}: {:.2} ms/chamada", e.0 / e.1 as f64);
        *e = (0.0, 0);
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            record(self.0, start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
