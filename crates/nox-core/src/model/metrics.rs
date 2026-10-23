//! System monitor snapshot. Every value carries its provenance (`Reading`).
//! Sources are documented in docs/METRICS_SOURCES.md.

use super::Timestamp;
use crate::reading::Reading;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreKind {
    Performance,
    Efficiency,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoreUsage {
    pub index: usize,
    pub kind: CoreKind,
    /// 0..100
    pub usage: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sensor {
    pub key: String,
    pub label: String,
    pub group: String,
    pub celsius: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryPressure {
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total: u64,
    pub used: u64,
    pub app: u64,
    pub wired: u64,
    pub compressed: u64,
    pub cached: u64,
    pub pressure: Reading<MemoryPressure>,
    pub swap_used: u64,
    pub swap_total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatteryInfo {
    pub percent: f64,
    pub on_ac: bool,
    pub charging: bool,
    pub cycle_count: Reading<u32>,
    /// Full-charge capacity / design capacity, percent.
    pub health_percent: Reading<f64>,
    pub condition: Reading<String>,
    /// Minutes to empty (on battery) or to full (charging).
    pub time_remaining_min: Reading<u32>,
    /// Positive = into battery, negative = out of battery.
    pub battery_watts: Reading<f64>,
    pub adapter_watts: Reading<f64>,
    pub temperature_c: Reading<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerInfo {
    pub system_watts: Reading<f64>,
    pub cpu_watts: Reading<f64>,
    pub gpu_watts: Reading<f64>,
    pub ane_watts: Reading<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub rx_total: u64,
    pub tx_total: u64,
    pub primary_interface: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskInfo {
    pub mount: String,
    pub name: String,
    pub total: u64,
    pub available: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub taken_at: Timestamp,
    pub cpu_total: Reading<f64>,
    pub cpu_cores: Reading<Vec<CoreUsage>>,
    pub gpu_usage: Reading<f64>,
    pub cpu_temp: Reading<f64>,
    pub gpu_temp: Reading<f64>,
    pub sensors: Reading<Vec<Sensor>>,
    pub memory: Reading<MemoryInfo>,
    pub battery: Reading<BatteryInfo>,
    pub power: Reading<PowerInfo>,
    pub network: Reading<NetworkInfo>,
    pub disks: Reading<Vec<DiskInfo>>,
    pub fans_rpm: Reading<Vec<f64>>,
}

impl Default for MetricsSnapshot {
    fn default() -> Self {
        Self {
            taken_at: 0.0,
            cpu_total: Reading::pending(),
            cpu_cores: Reading::pending(),
            gpu_usage: Reading::pending(),
            cpu_temp: Reading::pending(),
            gpu_temp: Reading::pending(),
            sensors: Reading::pending(),
            memory: Reading::pending(),
            battery: Reading::pending(),
            power: Reading::pending(),
            network: Reading::pending(),
            disks: Reading::pending(),
            fans_rpm: Reading::pending(),
        }
    }
}

/// Which series the bounded history keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Series {
    Cpu,
    Gpu,
    CpuTemp,
    Memory,
    NetRx,
    NetTx,
    SystemPower,
}

/// Bounded history: `None` entries mean "no sample" (gap), never zero.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsHistory {
    pub capacity: usize,
    pub series: Vec<(Series, std::collections::VecDeque<Option<f64>>)>,
}

impl MetricsHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            series: vec![],
        }
    }
    pub fn push(&mut self, s: Series, v: Option<f64>) {
        let cap = self.capacity.max(1);
        let q = match self.series.iter_mut().position(|(k, _)| *k == s) {
            Some(i) => &mut self.series[i].1,
            None => {
                self.series.push((s, Default::default()));
                &mut self.series.last_mut().expect("just pushed").1
            }
        };
        q.push_back(v);
        while q.len() > cap {
            q.pop_front();
        }
    }
    pub fn get(&self, s: Series) -> Option<&std::collections::VecDeque<Option<f64>>> {
        self.series.iter().find(|(k, _)| *k == s).map(|(_, q)| q)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsState {
    pub snapshot: MetricsSnapshot,
    pub history: MetricsHistory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsSettings {
    /// Sampling period while the monitor or island system page is visible.
    pub interval_ms: u64,
    /// Slower period when nothing displays metrics (battery alerts still work).
    pub background_interval_ms: u64,
    pub history_len: usize,
    /// Internal battery low alert (percent, 0 = off).
    pub low_battery_alert: u8,
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            interval_ms: 2000,
            background_interval_ms: 10_000,
            history_len: 120,
            low_battery_alert: 15,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded_and_keeps_gaps() {
        let mut h = MetricsHistory::new(3);
        for v in [Some(1.0), None, Some(3.0), Some(4.0)] {
            h.push(Series::Cpu, v);
        }
        let q: Vec<_> = h.get(Series::Cpu).unwrap().iter().copied().collect();
        assert_eq!(q, vec![None, Some(3.0), Some(4.0)]);
    }
}
