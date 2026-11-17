//! Cronómetro por sistema, sempre ligado e legível pelo bridge.
//!
//! Cada sistema instrumentado faz `let _probe = Probe::new("nome")` no topo do
//! corpo; o `Drop` acumula a duração. O acumulador é barato (um `Mutex` por
//! chamada, ~50 ns) e está SEMPRE ligado — `VAEL_PERF_DIAG=1` só decide se a
//! média periódica sai para o log.
//!
//! Porquê sempre ligado: medir uma sessão a quente (o jogo do utilizador, uma
//! cena específica, depois de 10 min de jogo) não pode exigir reiniciar com
//! uma env. O `vael-tools debug perf` lê [`snapshot`] pelo bridge e devolve a
//! tabela por sistema — é a atribuição que a tab rica não tem, porque os shims
//! `timed` das crates são pass-through.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: OnceLock<bool> = OnceLock::new();

/// `VAEL_PERF_DIAG=1` faz a média sair para o log a cada 120 chamadas. A
/// medição em si está sempre ligada (ver o módulo).
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var("VAEL_PERF_DIAG").is_ok_and(|v| v == "1"))
}

static ACC: Mutex<Option<HashMap<&'static str, (f64, u64)>>> = Mutex::new(None);

/// Mede o tempo até ao `drop` (uma chamada de sistema, um bloco).
pub struct Probe(&'static str, Instant);

impl Probe {
    pub fn new(name: &'static str) -> Self {
        Probe(name, Instant::now())
    }
}

/// Acumula uma amostra e, com `VAEL_PERF_DIAG=1`, imprime a média a cada 120.
pub fn record(name: &'static str, ms: f64) {
    let mut guard = ACC.lock().unwrap_or_else(|poison| poison.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let e = map.entry(name).or_insert((0.0, 0));
    e.0 += ms;
    e.1 += 1;
    if enabled() && e.1 >= 120 {
        eprintln!("PROBE {name}: {:.2} ms/chamada", e.0 / e.1 as f64);
        *e = (0.0, 0);
    }
}

/// A tabela acumulada: `(sistema, média ms/chamada, chamadas)`, da mais cara
/// para a mais barata.
pub fn snapshot() -> Vec<(&'static str, f64, u64)> {
    let guard = ACC.lock().unwrap_or_else(|poison| poison.into_inner());
    let Some(map) = guard.as_ref() else {
        return Vec::new();
    };
    let mut rows: Vec<(&'static str, f64, u64)> = map
        .iter()
        .map(|(name, (total, calls))| {
            let calls = *calls;
            (*name, if calls == 0 { 0.0 } else { total / calls as f64 }, calls)
        })
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows
}

/// Zera os acumuladores (o CLI pode pedir uma janela limpa).
pub fn reset() {
    let mut guard = ACC.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(map) = guard.as_mut() {
        map.clear();
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        record(self.0, self.1.elapsed().as_secs_f64() * 1000.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Um teste só: o acumulador é global.
    #[test]
    fn probes_accumulate_and_snapshot_in_order() {
        reset();
        {
            let _slow = Probe::new("test.slow");
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        {
            let _fast = Probe::new("test.fast");
        }
        let rows = snapshot();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].0, "test.slow", "ordem: mais caro primeiro");
        assert!(rows[0].1 >= 2.0, "a dormir 3 ms: {:?}", rows[0]);
        assert_eq!(rows[1].0, "test.fast");
        assert_eq!(rows[0].2, 1, "uma chamada cada");
        reset();
        assert!(snapshot().is_empty());
    }
}
