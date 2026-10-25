//! Opt-in render timing (`NOX_PERF=1`, read once). Each stage's samples are
//! summarised as p50 / p95 / max and logged every few seconds. Disabled, a
//! probe costs one cached flag check.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Seconds between two summaries of the same stage.
const REPORT_EVERY: Duration = Duration::from_secs(5);

struct Stage {
    /// Microseconds.
    samples: Vec<f64>,
    since: Instant,
}

static STAGES: Mutex<BTreeMap<&'static str, Stage>> = Mutex::new(BTreeMap::new());

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NOX_PERF").is_ok_and(|v| v == "1"))
}

/// Times the scope it lives in: `let _p = Probe::new("paint");`.
pub struct Probe(Option<(&'static str, Instant)>);

impl Probe {
    pub fn new(stage: &'static str) -> Self {
        Self(enabled().then(|| (stage, Instant::now())))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some((stage, start)) = self.0.take() {
            record(stage, start.elapsed());
        }
    }
}

fn record(stage: &'static str, d: Duration) {
    let Ok(mut stages) = STAGES.lock() else {
        return;
    };
    let s = stages.entry(stage).or_insert_with(|| Stage {
        samples: Vec::new(),
        since: Instant::now(),
    });
    s.samples.push(d.as_secs_f64() * 1e6);
    if s.since.elapsed() >= REPORT_EVERY {
        let (p50, p95, max) = summary(&mut s.samples);
        log::info!(
            "perf {stage}: n={} p50={p50:.0}µs p95={p95:.0}µs max={max:.0}µs",
            s.samples.len()
        );
        s.samples.clear();
        s.since = Instant::now();
    }
}

/// Takes every sample recorded so far (for benchmarks), in microseconds.
pub fn drain() -> Vec<(&'static str, Vec<f64>)> {
    let Ok(mut stages) = STAGES.lock() else {
        return Vec::new();
    };
    stages
        .iter_mut()
        .map(|(name, s)| (*name, std::mem::take(&mut s.samples)))
        .collect()
}

/// (p50, p95, max) of the samples; sorts them in place.
pub fn summary(samples: &mut [f64]) -> (f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    let at = |q: usize| samples[(n * q / 100).min(n - 1)];
    (at(50), at(95), samples[n - 1])
}

#[cfg(test)]
mod tests {
    use super::summary;

    #[test]
    fn summary_gives_p50_p95_and_max() {
        let mut v: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        assert_eq!(summary(&mut v), (51.0, 96.0, 100.0));
        assert_eq!(summary(&mut []), (0.0, 0.0, 0.0));
    }
}
