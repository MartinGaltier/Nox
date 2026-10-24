//! Power: SMC `PSTR` for the whole system, IOReport "Energy Model" counters
//! for CPU / GPU / ANE — only while those counters actually advance.
//!
//! On the M2 Air under macOS 27 the millijoule channels ("CPU Energy", "ANE",
//! "DRAM", …) stay frozen at their boot-time totals while "GPU Energy" (nJ)
//! keeps counting, so CPU and ANE are reported `NotSupported` there. A channel
//! is trusted once it has been seen advancing; ANE (legitimately 0 when idle)
//! is trusted only when "CPU Energy", which is never idle, advances too.

use crate::sys::ioreport::EnergyDelta;
use nox_core::{Reading, Unavailable};

/// The only "Energy Model" channels Nox subscribes to.
pub const CHANNELS: [&str; 3] = ["CPU Energy", "GPU Energy", "ANE"];

/// Samples without any movement after which a counter is declared frozen.
const FROZEN_AFTER: u32 = 2;

pub fn unit_scale(unit: &str) -> Option<f64> {
    match unit.trim() {
        "J" => Some(1.0),
        "mJ" => Some(1e-3),
        "uJ" | "µJ" => Some(1e-6),
        "nJ" => Some(1e-9),
        _ => None,
    }
}

#[derive(Default)]
struct Channel {
    moved: bool,
    still: u32,
}

#[derive(Default)]
pub struct EnergyTracker {
    cpu: Channel,
    gpu: Channel,
    ane: Channel,
}

pub struct BlockPower {
    pub cpu: Reading<f64>,
    pub gpu: Reading<f64>,
    pub ane: Reading<f64>,
}

impl BlockPower {
    pub fn unavailable(u: Unavailable) -> Self {
        Self {
            cpu: Reading::Unavailable(u.clone()),
            gpu: Reading::Unavailable(u.clone()),
            ane: Reading::Unavailable(u),
        }
    }
}

fn joules(deltas: &[EnergyDelta], channel: &str) -> Option<f64> {
    deltas
        .iter()
        .filter(|d| d.group == "Energy Model" && d.channel == channel)
        .map(|d| unit_scale(&d.unit).map(|s| d.value as f64 * s))
        .sum()
}

impl EnergyTracker {
    /// `deltas` empty = baseline sample (first call).
    pub fn interpret(&mut self, deltas: &[EnergyDelta], dt: f64) -> BlockPower {
        if deltas.is_empty() || dt <= 0.0 {
            return BlockPower::unavailable(Unavailable::Pending);
        }
        let cpu_j = joules(deltas, "CPU Energy");
        let gpu_j = joules(deltas, "GPU Energy");
        let ane_j = joules(deltas, "ANE");

        let cpu = Self::track(&mut self.cpu, cpu_j, dt, false);
        let gpu = Self::track(&mut self.gpu, gpu_j, dt, true);
        let ane = if self.cpu.moved {
            match ane_j {
                Some(j) if j >= 0.0 => {
                    self.ane.moved |= j > 0.0;
                    Reading::Measured(j / dt)
                }
                _ => Reading::Unavailable(Unavailable::NotSupported),
            }
        } else if self.cpu.still >= FROZEN_AFTER {
            Reading::Unavailable(Unavailable::NotSupported)
        } else {
            Reading::pending()
        };
        BlockPower { cpu, gpu, ane }
    }

    fn track(ch: &mut Channel, j: Option<f64>, dt: f64, zero_ok: bool) -> Reading<f64> {
        let Some(j) = j else {
            return Reading::Unavailable(Unavailable::NotSupported);
        };
        if j > 0.0 {
            ch.moved = true;
            ch.still = 0;
            return Reading::Measured(j / dt);
        }
        ch.still = ch.still.saturating_add(1);
        if ch.moved && zero_ok {
            Reading::Measured(0.0)
        } else if ch.moved {
            Reading::Unavailable(Unavailable::Error("compteur d'énergie arrêté".into()))
        } else if ch.still >= FROZEN_AFTER {
            Reading::Unavailable(Unavailable::NotSupported)
        } else {
            Reading::pending()
        }
    }
}

/// SMC `PSTR` (system total power, W) when plausible; otherwise, on battery,
/// the battery discharge power is the whole system's draw (estimate).
pub fn system_watts(pstr: Option<f64>, battery_w: Option<f64>, on_ac: bool) -> Reading<f64> {
    match pstr {
        Some(w) if w > 0.0 && w < 1000.0 => Reading::Measured(w),
        _ => match battery_w {
            Some(b) if !on_ac && b < 0.0 => Reading::Estimated(-b),
            _ => Reading::Unavailable(Unavailable::NotSupported),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(channel: &str, unit: &str, value: i64) -> EnergyDelta {
        EnergyDelta {
            group: "Energy Model".into(),
            channel: channel.into(),
            unit: unit.into(),
            value,
        }
    }

    #[test]
    fn frozen_cpu_counters_become_not_supported() {
        let mut t = EnergyTracker::default();
        let frozen = vec![
            d("CPU Energy", "mJ", 0),
            d("ANE", "mJ", 0),
            d("GPU Energy", "nJ", 13_800_000),
        ];
        assert!(t.interpret(&[], 1.0).cpu.value().is_none());
        let p = t.interpret(&frozen, 1.0);
        assert_eq!(p.cpu, Reading::pending());
        assert!((p.gpu.value().unwrap() - 0.0138).abs() < 1e-12);
        let p = t.interpret(&frozen, 1.0);
        assert_eq!(p.cpu, Reading::Unavailable(Unavailable::NotSupported));
        assert_eq!(p.ane, Reading::Unavailable(Unavailable::NotSupported));
    }

    #[test]
    fn live_counters_are_measured() {
        let mut t = EnergyTracker::default();
        let live = vec![
            d("CPU Energy", "mJ", 1500),
            d("ANE", "mJ", 0),
            d("GPU Energy", "nJ", 0),
        ];
        let p = t.interpret(&live, 2.0);
        assert_eq!(p.cpu, Reading::Measured(0.75));
        assert_eq!(p.ane, Reading::Measured(0.0));
        assert_eq!(p.gpu, Reading::pending());
    }

    #[test]
    fn units() {
        assert_eq!(unit_scale("uJ"), Some(1e-6));
        assert_eq!(unit_scale("events"), None);
    }

    #[test]
    fn system_power_fallback() {
        assert_eq!(
            system_watts(Some(4.0), Some(-2.0), false),
            Reading::Measured(4.0)
        );
        assert_eq!(
            system_watts(None, Some(-2.5), false),
            Reading::Estimated(2.5)
        );
        assert!(system_watts(Some(0.0), Some(1.0), true).value().is_none());
    }
}
