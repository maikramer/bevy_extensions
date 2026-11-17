//! Decode telemetry: what the reader actually spent.
//!
//! Loading is the one part of the frame nobody profiles well — it happens
//! before the profiler has frames to look at. These counters are cheap
//! (atomics, one lock only on the first occurrence of a warning) and answer the
//! questions a perf pass needs: how many files, how much data, how long, and
//! whether anything fell back to raw bytes.
//!
//! ```text
//! BEVY_EXT_GLTF_STATS=1   # log a line whenever the counters move
//! ```

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::app::{App, Plugin, Update};
use bevy::log::info;
use bevy::prelude::{Res, Time};

/// A snapshot of the decoder's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStats {
    /// GLB files that reached the decoder.
    pub files: u64,
    /// Files whose bytes the decoder rewrote (the rest passed through).
    pub rewritten: u64,
    /// Source bytes seen.
    pub bytes_in: u64,
    /// Bytes handed to the loader (rewritten files only, plus pass-throughs).
    pub bytes_out: u64,
    /// Wall time spent inside the decoder.
    pub decode_nanos: u64,
    /// Part of `decode_nanos` spent widening quantized attributes.
    pub dequant_nanos: u64,
    /// Single-channel UASTC textures re-labelled `RGB` (the BC4 workaround).
    pub uastc_relabels: u64,
    /// Files that named an extension this crate does not understand.
    pub unsupported: u64,
}

impl DecodeStats {
    /// `bytes_out / bytes_in`, i.e. how much the rewrite inflates the pool.
    pub fn ratio(&self) -> f32 {
        if self.bytes_in == 0 {
            1.0
        } else {
            self.bytes_out as f32 / self.bytes_in as f32
        }
    }
}

static FILES: AtomicU64 = AtomicU64::new(0);
static REWRITTEN: AtomicU64 = AtomicU64::new(0);
static BYTES_IN: AtomicU64 = AtomicU64::new(0);
static BYTES_OUT: AtomicU64 = AtomicU64::new(0);
static DECODE_NANOS: AtomicU64 = AtomicU64::new(0);
static DEQUANT_NANOS: AtomicU64 = AtomicU64::new(0);
static UASTC_RELABELS: AtomicU64 = AtomicU64::new(0);
static UNSUPPORTED: AtomicU64 = AtomicU64::new(0);

/// Reads the counters. Safe to call from anywhere; costs one relaxed load each.
pub fn decode_stats() -> DecodeStats {
    DecodeStats {
        files: FILES.load(Ordering::Relaxed),
        rewritten: REWRITTEN.load(Ordering::Relaxed),
        bytes_in: BYTES_IN.load(Ordering::Relaxed),
        bytes_out: BYTES_OUT.load(Ordering::Relaxed),
        decode_nanos: DECODE_NANOS.load(Ordering::Relaxed),
        dequant_nanos: DEQUANT_NANOS.load(Ordering::Relaxed),
        uastc_relabels: UASTC_RELABELS.load(Ordering::Relaxed),
        unsupported: UNSUPPORTED.load(Ordering::Relaxed),
    }
}

/// Records one decoded (or passed-through) GLB.
pub(crate) fn record_file(bytes_in: usize, bytes_out: usize, rewritten: bool, nanos: u64) {
    FILES.fetch_add(1, Ordering::Relaxed);
    BYTES_IN.fetch_add(bytes_in as u64, Ordering::Relaxed);
    BYTES_OUT.fetch_add(bytes_out as u64, Ordering::Relaxed);
    DECODE_NANOS.fetch_add(nanos, Ordering::Relaxed);
    if rewritten {
        REWRITTEN.fetch_add(1, Ordering::Relaxed);
    }
}

/// Records time spent widening quantized attributes (part of the decode span).
pub(crate) fn record_dequant(nanos: u64) {
    DEQUANT_NANOS.fetch_add(nanos, Ordering::Relaxed);
}

/// Records how many UASTC `RRR` textures were re-labelled.
pub(crate) fn record_uastc_relabels(count: usize) {
    if count > 0 {
        UASTC_RELABELS.fetch_add(count as u64, Ordering::Relaxed);
    }
}

/// Records a file that names an extension we cannot handle.
pub(crate) fn record_unsupported() {
    UNSUPPORTED.fetch_add(1, Ordering::Relaxed);
}

/// Emits a warning once per `key`.
///
/// Loading is parallel and per-file, so the same complaint about the same
/// missing feature would otherwise arrive once per asset — and there are
/// hundreds.
pub fn warn_once(key: &str, message: impl FnOnce() -> String) {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let fresh = {
        let mut seen = SEEN.lock().unwrap_or_else(|poison| poison.into_inner());
        seen.get_or_insert_with(HashSet::new).insert(key.to_string())
    };
    if fresh {
        bevy::log::warn!("{}", message());
    }
}

/// Logs the decode counters every few seconds when `BEVY_EXT_GLTF_STATS` is set.
pub struct GltfStatsPlugin;

impl Plugin for GltfStatsPlugin {
    fn build(&self, app: &mut App) {
        let enabled = std::env::var("BEVY_EXT_GLTF_STATS")
            .is_ok_and(|value| !matches!(value.as_str(), "" | "0" | "false" | "off"));
        if enabled {
            app.add_systems(Update, log_stats);
        }
    }
}

fn log_stats(time: Res<Time>, mut last: bevy::prelude::Local<(f64, DecodeStats)>) {
    let now = time.elapsed_secs_f64();
    if now - last.0 < 5.0 {
        return;
    }
    last.0 = now;
    let stats = decode_stats();
    if stats == last.1 || stats.files == 0 {
        return;
    }
    last.1 = stats;
    info!(
        "gltf decode: {} ficheiros ({} reescritos), {:.1} MB -> {:.1} MB (×{:.2}), {:.0} ms \
         (dequant {:.0} ms), {} relabels UASTC, {} sem suporte",
        stats.files,
        stats.rewritten,
        stats.bytes_in as f64 / 1e6,
        stats.bytes_out as f64 / 1e6,
        stats.ratio(),
        stats.decode_nanos as f64 / 1e6,
        stats.dequant_nanos as f64 / 1e6,
        stats.uastc_relabels,
        stats.unsupported,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_is_one_without_input() {
        assert_eq!(DecodeStats::default().ratio(), 1.0);
        let stats = DecodeStats {
            bytes_in: 100,
            bytes_out: 150,
            ..Default::default()
        };
        assert_eq!(stats.ratio(), 1.5);
    }

    #[test]
    fn counters_accumulate() {
        let before = decode_stats();
        record_file(10, 20, true, 5);
        record_dequant(3);
        record_uastc_relabels(2);
        record_unsupported();
        let after = decode_stats();
        assert_eq!(after.files, before.files + 1);
        assert_eq!(after.rewritten, before.rewritten + 1);
        assert_eq!(after.bytes_in, before.bytes_in + 10);
        assert_eq!(after.bytes_out, before.bytes_out + 20);
        assert_eq!(after.decode_nanos, before.decode_nanos + 5);
        assert_eq!(after.dequant_nanos, before.dequant_nanos + 3);
        assert_eq!(after.uastc_relabels, before.uastc_relabels + 2);
        assert_eq!(after.unsupported, before.unsupported + 1);
    }
}
