//! System monitor collectors (CPU, GPU, sensors, memory, battery, power, network, disk).
//! Owned by Agent Mesures. Public contract: [`module`].
//!
//! One worker thread ([`module::MetricsModule`]) owns every system handle
//! (SMC connection, IOReport subscription, registry entries, dynamic store),
//! samples at `settings.metrics.interval_ms` while something displays metrics
//! (`Command::MetricsVisible(true)`) and at `background_interval_ms`
//! otherwise, and publishes `IslandState::metrics`. Sources and their
//! verification on the M2: docs/METRICS_SOURCES.md.

mod alerts;
mod battery;
mod collector;
mod cpu;
mod disk;
mod memory;
mod module;
mod network;
mod power;
mod sensors;
mod smc;
mod sys;
mod value;

use nox_core::Module;

/// The metrics module (single shared sampler feeding `IslandState::metrics`).
pub fn module() -> Box<dyn Module> {
    Box::new(module::MetricsModule::default())
}

#[doc(hidden)]
pub mod diag {
    //! Diagnostics for the examples and the `#[ignore]` hardware tests (not
    //! part of the module contract).

    use nox_core::model::metrics::MetricsSnapshot;
    use std::time::{Duration, Instant};

    /// A real snapshot: baseline sample, wait, second sample (deltas valid).
    pub fn snapshot(wait: Duration) -> MetricsSnapshot {
        let mut c = crate::collector::Collector::new();
        c.sample(60.0);
        std::thread::sleep(wait);
        c.sample(60.0)
    }

    /// Samples every `interval` for `duration` on this thread; returns
    /// (samples, collector thread CPU time, wall time, setup CPU time).
    pub fn bench(duration: Duration, interval: Duration) -> (u64, Duration, Duration, Duration) {
        let t0 = crate::sys::mach::thread_cpu_time();
        let mut c = crate::collector::Collector::new();
        let setup = crate::sys::mach::thread_cpu_time().saturating_sub(t0);
        let start = Instant::now();
        let cpu0 = crate::sys::mach::thread_cpu_time();
        let mut n = 0;
        while start.elapsed() < duration {
            std::hint::black_box(c.sample(60.0));
            n += 1;
            std::thread::sleep(interval);
        }
        (
            n,
            crate::sys::mach::thread_cpu_time().saturating_sub(cpu0),
            start.elapsed(),
            setup,
        )
    }

    /// Every SMC key with its type and decoded value (when decodable).
    pub fn smc_keys() -> Result<Vec<(String, String, Option<f64>)>, String> {
        let smc = crate::sys::smc::Smc::open()?;
        let mut out = vec![];
        for i in 0..smc.key_count() {
            let Some(k) = smc.key_at(i) else { continue };
            let Some(info) = smc.key_info(k) else {
                continue;
            };
            let v = smc
                .read(k, info)
                .and_then(|b| crate::smc::decode(&b, &info.kind));
            out.push((
                crate::smc::fourcc_str(k),
                String::from_utf8_lossy(&info.kind).into_owned(),
                v,
            ));
        }
        Ok(out)
    }

    /// IOReport "Energy Model" deltas over `ms`: (group, channel, unit, value).
    pub fn energy(ms: u64) -> Result<Vec<(String, String, String, i64)>, String> {
        let mut s = crate::sys::ioreport::Subscription::new("Energy Model")?;
        s.sample()?;
        std::thread::sleep(Duration::from_millis(ms));
        Ok(s.sample()?
            .into_iter()
            .map(|d| (d.group, d.channel, d.unit, d.value))
            .collect())
    }
}
