//! `ModuleId::Metrics`: one worker thread samples everything at the
//! foreground or background interval and publishes `IslandState::metrics`.

use crate::alerts::BatteryAlerts;
use crate::collector::Collector;
use nox_core::model::metrics::{
    MetricsHistory, MetricsSettings, MetricsSnapshot, MetricsState, Series,
};
use nox_core::{Command, Hub, Module, ModuleContext, ModuleError, ModuleId, Settings, Worker};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MIN_INTERVAL: Duration = Duration::from_millis(250);
const MAX_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Default)]
struct Shared {
    visible: AtomicBool,
    settings: Mutex<MetricsSettings>,
}

impl Shared {
    fn settings(&self) -> MetricsSettings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn interval(&self) -> Duration {
        interval(&self.settings(), self.visible.load(Ordering::SeqCst))
    }
}

pub(crate) fn interval(s: &MetricsSettings, visible: bool) -> Duration {
    let ms = if visible {
        s.interval_ms
    } else {
        s.background_interval_ms
    };
    Duration::from_millis(ms).clamp(MIN_INTERVAL, MAX_INTERVAL)
}

/// Delta readings older than this are stale (the Mac slept, the thread was
/// suspended): they only re-baseline. Uses the longest of the previous and
/// current periods so switching background → foreground keeps the sample.
pub(crate) fn max_gap(prev: Duration, cur: Duration) -> f64 {
    prev.max(cur).as_secs_f64() * 2.5 + 1.0
}

pub(crate) fn push_history(h: &mut MetricsHistory, s: &MetricsSnapshot) {
    let mem = s
        .memory
        .value()
        .filter(|m| m.total > 0)
        .map(|m| m.used as f64 * 100.0 / m.total as f64);
    let net = s.network.value();
    let sys_w = s
        .power
        .value()
        .and_then(|p| p.system_watts.value().copied());
    h.push(Series::Cpu, s.cpu_total.value().copied());
    h.push(Series::Gpu, s.gpu_usage.value().copied());
    h.push(Series::CpuTemp, s.cpu_temp.value().copied());
    h.push(Series::Memory, mem);
    h.push(Series::NetRx, net.map(|n| n.rx_bytes_per_sec));
    h.push(Series::NetTx, net.map(|n| n.tx_bytes_per_sec));
    h.push(Series::SystemPower, sys_w);
}

fn run(token: &nox_core::CancelToken, hub: &Hub, shared: &Shared) {
    let mut collector = Collector::new();
    log::info!("metrics collector ready ({:?})", collector.platform());
    let mut alerts = BatteryAlerts::default();
    let mut history = MetricsHistory::new(shared.settings().history_len);
    let mut prev = shared.interval();
    let cpu_start = crate::sys::mach::thread_cpu_time();
    let wall_start = std::time::Instant::now();
    let mut ticks = 0u64;
    loop {
        let settings = shared.settings();
        let cur = shared.interval();
        let snapshot = collector.sample(max_gap(prev, cur));
        history.capacity = settings.history_len.max(1);
        push_history(&mut history, &snapshot);
        for a in alerts.check(snapshot.battery.value(), settings.low_battery_alert) {
            hub.push_alert(a);
        }
        let state = MetricsState {
            snapshot,
            history: history.clone(),
        };
        hub.update(|s| s.metrics = Some(state));
        ticks += 1;
        if ticks.is_multiple_of(100) {
            let cpu = crate::sys::mach::thread_cpu_time().saturating_sub(cpu_start);
            log::debug!(
                "metrics: {ticks} samples, collector CPU {:.3}% of wall time",
                cpu.as_secs_f64() * 100.0 / wall_start.elapsed().as_secs_f64().max(1e-9)
            );
        }
        prev = cur;
        if token.sleep(cur) {
            break;
        }
    }
}

#[derive(Default)]
pub struct MetricsModule {
    worker: Option<Worker>,
    shared: Arc<Shared>,
    hub: Option<Hub>,
}

impl Module for MetricsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Metrics
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        if self.worker.is_some() {
            return Ok(());
        }
        *self
            .shared
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.settings.metrics.clone();
        let hub = ctx.hub.clone();
        let shared = self.shared.clone();
        hub.update(|s| s.metrics = Some(MetricsState::default()));
        self.worker = Some(Worker::spawn("metrics", move |token| {
            run(&token, &hub, &shared)
        }));
        self.hub = Some(ctx.hub);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        if let Some(hub) = self.hub.take() {
            hub.update(|s| s.metrics = None);
        }
    }

    fn command(&mut self, cmd: Command) {
        if let Command::MetricsVisible(v) = cmd {
            let was = self.shared.visible.swap(v, Ordering::SeqCst);
            // Becoming visible: sample now at the faster rate. Becoming hidden
            // simply lets the current short sleep finish.
            if v && !was
                && let Some(w) = &self.worker
            {
                w.token().nudge();
            }
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        let mut cur = self
            .shared
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *cur == settings.metrics {
            return;
        }
        *cur = settings.metrics.clone();
        drop(cur);
        if let Some(w) = &self.worker {
            w.token().nudge();
        }
    }
}

impl Drop for MetricsModule {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::Reading;

    #[test]
    fn interval_follows_visibility_and_clamps() {
        let s = MetricsSettings::default();
        assert_eq!(interval(&s, true), Duration::from_millis(2000));
        assert_eq!(interval(&s, false), Duration::from_millis(10_000));
        let tiny = MetricsSettings {
            interval_ms: 1,
            ..Default::default()
        };
        assert_eq!(interval(&tiny, true), MIN_INTERVAL);
    }

    #[test]
    fn gap_uses_longest_period() {
        let fg = Duration::from_secs(2);
        let bg = Duration::from_secs(10);
        assert!(max_gap(bg, fg) > 10.0);
        assert!(max_gap(fg, fg) < 10.0);
    }

    #[test]
    fn history_gaps_are_none() {
        let mut h = MetricsHistory::new(4);
        let mut s = MetricsSnapshot {
            cpu_total: Reading::Measured(12.0),
            ..Default::default()
        };
        push_history(&mut h, &s);
        s.cpu_total = Reading::pending();
        push_history(&mut h, &s);
        let q: Vec<_> = h.get(Series::Cpu).unwrap().iter().copied().collect();
        assert_eq!(q, vec![Some(12.0), None]);
        assert_eq!(h.get(Series::Memory).unwrap().len(), 2);
    }
}
