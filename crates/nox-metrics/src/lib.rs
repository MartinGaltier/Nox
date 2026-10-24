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
        let mut s = crate::sys::ioreport::Subscription::new("Energy Model", &[])?;
        s.sample()?;
        std::thread::sleep(Duration::from_millis(ms));
        Ok(s.sample()?
            .into_iter()
            .map(|d| (d.group, d.channel, d.unit, d.value))
            .collect())
    }
}

#[doc(hidden)]
pub mod profile {
    //! Per-source cost of one sample (thread CPU time, averaged).

    use crate::sys;
    use std::time::Duration;

    fn time(n: u32, mut f: impl FnMut()) -> Duration {
        let t0 = sys::mach::thread_cpu_time();
        for _ in 0..n {
            f();
        }
        sys::mach::thread_cpu_time().saturating_sub(t0) / n
    }

    pub fn parts(n: u32) -> Vec<(&'static str, Duration)> {
        let gpu = sys::iokit::IoObject::all_matching("IOAccelerator");
        let batt = sys::iokit::IoObject::matching("AppleSmartBattery");
        let smc = sys::smc::Smc::open().ok();
        let mut ior =
            sys::ioreport::Subscription::new("Energy Model", &crate::power::CHANNELS).ok();
        let store = sys::net::DynamicStore::new();
        let key = crate::smc::fourcc("Tp01");
        let info = smc.as_ref().and_then(|s| s.key_info(key));
        vec![
            ("cpu_ticks", time(n, || drop(sys::mach::cpu_ticks()))),
            ("vm_stats", time(n, || drop(sys::mach::vm_stats()))),
            (
                "gpu PerformanceStatistics",
                time(n, || {
                    for g in &gpu {
                        drop(g.property("PerformanceStatistics").map(|v| v.to_value()));
                    }
                }),
            ),
            (
                "smc read x1",
                time(n, || {
                    if let (Some(s), Some(i)) = (&smc, info) {
                        drop(s.read(key, i));
                    }
                }),
            ),
            (
                "battery registry (10 keys)",
                time(n, || {
                    if let Some(b) = &batt {
                        for k in crate::battery::REGISTRY_KEYS {
                            drop(b.property(k).map(|v| v.to_value()));
                        }
                    }
                }),
            ),
            (
                "IOPS power sources",
                time(n, || drop(sys::iokit::power_sources())),
            ),
            (
                "ioreport sample",
                time(n, || {
                    if let Some(s) = &mut ior {
                        drop(s.sample());
                    }
                }),
            ),
            (
                "primary interface (configd)",
                time(n, || {
                    drop(store.as_ref().and_then(|s| s.primary_interface()))
                }),
            ),
            (
                "interface counters",
                time(n, || drop(sys::net::interface_counters())),
            ),
            ("mounts", time(n, || drop(sys::disk::mounts()))),
        ]
    }
}
