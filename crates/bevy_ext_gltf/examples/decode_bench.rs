//! Decode benchmark over a real asset pool.
//!
//! ```text
//! cargo run -p bevy_ext_gltf --release --example decode_bench -- \
//!     --root /home/maikeu/GitClones/Vael/assets/public [--limit 200] [--json]
//! ```
//!
//! Reads every `.glb` under `--root` (or the `--limit` largest ones) and times
//! [`decode_glb`] on each. Reports per-file input/output bytes and milliseconds,
//! then totals, p50/p95 and the process high-water RSS (`VmHWM`).
//!
//! With `--features legacy` the same bytes also run through the pre-rewrite
//! decoder ([`bevy_ext_gltf::legacy`]), which is what makes the A/B honest: the
//! two implementations see the same bytes in the same process, and the bench
//! asserts the outputs are equal before believing the timings.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let mut root: Option<PathBuf> = None;
    let mut limit: Option<usize> = None;
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--limit" => limit = args.next().and_then(|v| v.parse().ok()),
            "--json" => json = true,
            "-h" | "--help" => {
                println!("usage: decode_bench --root <dir> [--limit N] [--json]");
                return;
            }
            other => {
                eprintln!("unknown argument `{other}`");
                std::process::exit(2);
            }
        }
    }
    let Some(root) = root else {
        eprintln!("usage: decode_bench --root <dir> [--limit N] [--json]");
        std::process::exit(2);
    };

    let mut files = Vec::new();
    walk(&root, &mut files);
    // Largest first: the decode cost is what we are measuring, and the pool has
    // a long tail of tiny props that would drown the average.
    files.sort_by_key(|path| std::cmp::Reverse(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)));
    if let Some(limit) = limit {
        files.truncate(limit);
    }
    if files.is_empty() {
        eprintln!("no .glb under {}", root.display());
        std::process::exit(1);
    }

    let mut samples: Vec<Sample> = Vec::with_capacity(files.len());
    #[cfg(feature = "legacy")]
    let mut mismatches = 0usize;
    #[cfg(not(feature = "legacy"))]
    let mismatches = 0usize;
    for path in &files {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("{}: {error}", path.display());
                continue;
            }
        };
        let start = Instant::now();
        let decoded = match bevy_ext_gltf::decode_glb(&bytes) {
            Ok(decoded) => decoded,
            Err(error) => {
                eprintln!("{}: decode failed: {error:#}", path.display());
                continue;
            }
        };
        let elapsed = start.elapsed();

        #[cfg(feature = "legacy")]
        let legacy = {
            let start = Instant::now();
            let legacy = bevy_ext_gltf::legacy::decode_glb(&bytes)
                .unwrap_or_else(|e| panic!("legacy decode of {}: {e:#}", path.display()));
            let legacy_elapsed = start.elapsed();
            if legacy.as_slice() != decoded.as_ref() {
                mismatches += 1;
                eprintln!("{}: legacy and current outputs differ", path.display());
            }
            Some(legacy_elapsed.as_nanos() as u64)
        };
        #[cfg(not(feature = "legacy"))]
        let legacy = None;

        samples.push(Sample {
            path: path.clone(),
            source: bytes.len(),
            decoded: decoded.len(),
            nanos: elapsed.as_nanos() as u64,
            legacy_nanos: legacy,
        });
    }

    let report = Report::new(&samples, mismatches);
    if json {
        println!("{}", report.to_json());
    } else {
        print!("{}", report.to_text());
    }
    if report.mismatches > 0 {
        std::process::exit(1);
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("glb"))
        {
            out.push(path);
        }
    }
}

struct Sample {
    path: PathBuf,
    source: usize,
    decoded: usize,
    nanos: u64,
    legacy_nanos: Option<u64>,
}

struct Report {
    samples: Vec<Row>,
    mismatches: usize,
    total_source: usize,
    total_decoded: usize,
    total_nanos: u64,
    peak_rss_kb: Option<u64>,
}

struct Row {
    path: PathBuf,
    source: usize,
    decoded: usize,
    nanos: u64,
    legacy: Option<u64>,
}

impl Report {
    fn new(samples: &[Sample], mismatches: usize) -> Self {
        Self {
            samples: samples
                .iter()
                .map(|s| Row {
                    path: s.path.clone(),
                    source: s.source,
                    decoded: s.decoded,
                    nanos: s.nanos,
                    legacy: s.legacy_nanos,
                })
                .collect(),
            mismatches,
            total_source: samples.iter().map(|s| s.source).sum(),
            total_decoded: samples.iter().map(|s| s.decoded).sum(),
            total_nanos: samples.iter().map(|s| s.nanos).sum(),
            peak_rss_kb: peak_rss_kb(),
        }
    }

    fn percentiles(&self) -> (f64, f64) {
        let mut ms: Vec<f64> = self.samples.iter().map(|row| row.nanos as f64 / 1e6).collect();
        if ms.is_empty() {
            return (0.0, 0.0);
        }
        ms.sort_by(f64::total_cmp);
        let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
        (at(0.5), at(0.95))
    }

    fn to_text(&self) -> String {
        let mut out = String::new();
        let width = self
            .samples
            .iter()
            .map(|row| row.path.as_os_str().len())
            .max()
            .unwrap_or(0)
            .min(64);
        for row in &self.samples {
            let mb_s = if row.nanos > 0 {
                row.source as f64 / 1e6 / (row.nanos as f64 / 1e9)
            } else {
                0.0
            };
            let name: String = row
                .path
                .to_string_lossy()
                .chars()
                .rev()
                .take(width)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            out.push_str(&format!(
                "{name:>width$} {source:>9} B -> {decoded:>9} B  {:>8.2} ms  {mb_s:>7.1} MB/s",
                row.nanos as f64 / 1e6,
                source = row.source,
                decoded = row.decoded,
            ));
            if let Some(legacy) = row.legacy {
                out.push_str(&format!(
                    "   legacy {:>8.2} ms  ({:+.0}%)",
                    legacy as f64 / 1e6,
                    (row.nanos as f64 / legacy as f64 - 1.0) * 100.0
                ));
            }
            out.push('\n');
        }
        let (p50, p95) = self.percentiles();
        let total_ms = self.total_nanos as f64 / 1e6;
        out.push_str(&format!(
            "\nfiles {}  source {:.1} MB -> decoded {:.1} MB ({:+.1}%)\n",
            self.samples.len(),
            self.total_source as f64 / 1e6,
            self.total_decoded as f64 / 1e6,
            (self.total_decoded as f64 / self.total_source as f64 - 1.0) * 100.0,
        ));
        out.push_str(&format!(
            "total {total_ms:.1} ms  p50 {p50:.2} ms  p95 {p95:.2} ms  {:.1} MB/s\n",
            self.total_source as f64 / 1e6 / (total_ms / 1000.0)
        ));
        if let Some(rss) = self.peak_rss_kb {
            out.push_str(&format!("peak RSS {:.1} MB\n", rss as f64 / 1024.0));
        }
        if self.mismatches > 0 {
            out.push_str(&format!("MISMATCHES: {}\n", self.mismatches));
        }
        if let Some(legacy_total) = self.samples.iter().filter_map(|row| row.legacy).reduce(|a, b| a + b)
        {
            let current = self.total_nanos as f64 / 1e6;
            let legacy = legacy_total as f64 / 1e6;
            out.push_str(&format!(
                "legacy total {legacy:.1} ms vs current {current:.1} ms  (×{:.2})\n",
                legacy / current
            ));
        }
        out
    }

    fn to_json(&self) -> String {
        let (p50, p95) = self.percentiles();
        let files: Vec<String> = self
            .samples
            .iter()
            .map(|row| {
                format!(
                    "{{\"path\":\"{}\",\"source\":{},\"decoded\":{},\"ms\":{:.3},\"legacy_ms\":{}}}",
                    row.path.to_string_lossy().replace('\\', "/"),
                    row.source,
                    row.decoded,
                    row.nanos as f64 / 1e6,
                    row.legacy.map_or("null".into(), |l| format!("{:.3}", l as f64 / 1e6))
                )
            })
            .collect();
        format!(
            "{{\"files\":{},\"source_bytes\":{},\"decoded_bytes\":{},\"total_ms\":{:.3},\
             \"p50_ms\":{p50:.3},\"p95_ms\":{p95:.3},\"peak_rss_kb\":{},\"mismatches\":{},\
             \"items\":[{}]}}",
            self.samples.len(),
            self.total_source,
            self.total_decoded,
            self.total_nanos as f64 / 1e6,
            self.peak_rss_kb.map_or("null".into(), |v| v.to_string()),
            self.mismatches,
            files.join(",")
        )
    }
}

/// High-water RSS of this process, in kB (`VmHWM` on Linux).
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}
