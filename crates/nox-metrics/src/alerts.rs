//! Internal-battery alerts (E2): low charge once per crossing (with
//! hysteresis) and power-source changes.

use nox_core::model::metrics::BatteryInfo;
use nox_core::{Alert, AlertKind, AlertPriority};

/// Charge points above the threshold needed to re-arm the low alert.
pub const HYSTERESIS: f64 = 3.0;

pub struct BatteryAlerts {
    armed: bool,
    last_on_ac: Option<bool>,
}

impl Default for BatteryAlerts {
    fn default() -> Self {
        Self {
            armed: true,
            last_on_ac: None,
        }
    }
}

fn battery_symbol(percent: f64) -> &'static str {
    match percent {
        p if p <= 10.0 => "battery.0percent",
        p if p <= 37.0 => "battery.25percent",
        p if p <= 62.0 => "battery.50percent",
        p if p <= 87.0 => "battery.75percent",
        _ => "battery.100percent",
    }
}

impl BatteryAlerts {
    /// `threshold` = `settings.metrics.low_battery_alert` (0 = off).
    pub fn check(&mut self, battery: Option<&BatteryInfo>, threshold: u8) -> Vec<Alert> {
        let Some(b) = battery else {
            return vec![];
        };
        let mut out = vec![];
        let pct = b.percent.round();

        if let Some(prev) = self.last_on_ac.replace(b.on_ac)
            && prev != b.on_ac
        {
            let (title, symbol) = if b.on_ac {
                ("Sur secteur", "powerplug.fill")
            } else {
                ("Sur batterie", battery_symbol(b.percent))
            };
            let sub = if b.on_ac && b.charging {
                format!("{pct:.0} % · en charge")
            } else {
                format!("{pct:.0} %")
            };
            out.push(Alert::new(AlertKind::PowerSource, title, symbol).subtitle(sub));
        }

        let t = f64::from(threshold);
        if threshold == 0 || b.on_ac || b.percent >= t + HYSTERESIS {
            self.armed = true;
        } else if self.armed && b.percent <= t {
            self.armed = false;
            out.push(
                Alert::new(
                    AlertKind::LowBattery,
                    "Batterie faible",
                    battery_symbol(b.percent),
                )
                .subtitle(format!("{pct:.0} % restants"))
                .priority(AlertPriority::High)
                .ttl(6.0),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::Reading;

    fn batt(percent: f64, on_ac: bool) -> BatteryInfo {
        BatteryInfo {
            percent,
            on_ac,
            charging: on_ac,
            cycle_count: Reading::pending(),
            health_percent: Reading::pending(),
            condition: Reading::pending(),
            time_remaining_min: Reading::pending(),
            battery_watts: Reading::pending(),
            adapter_watts: Reading::pending(),
            temperature_c: Reading::pending(),
        }
    }

    fn kinds(v: &[Alert]) -> Vec<AlertKind> {
        v.iter().map(|a| a.kind).collect()
    }

    #[test]
    fn low_battery_once_per_crossing_with_hysteresis() {
        let mut a = BatteryAlerts::default();
        assert!(a.check(Some(&batt(20.0, false)), 15).is_empty());
        let v = a.check(Some(&batt(15.0, false)), 15);
        assert_eq!(kinds(&v), vec![AlertKind::LowBattery]);
        assert_eq!(v[0].priority, AlertPriority::High);
        assert!(a.check(Some(&batt(14.0, false)), 15).is_empty());
        // jitter just above the threshold does not re-arm
        assert!(a.check(Some(&batt(16.0, false)), 15).is_empty());
        assert!(a.check(Some(&batt(15.0, false)), 15).is_empty());
        // clearly above → re-armed
        assert!(a.check(Some(&batt(18.0, false)), 15).is_empty());
        assert_eq!(
            kinds(&a.check(Some(&batt(15.0, false)), 15)),
            vec![AlertKind::LowBattery]
        );
    }

    #[test]
    fn disabled_threshold_and_ac() {
        let mut a = BatteryAlerts::default();
        assert!(a.check(Some(&batt(5.0, false)), 0).is_empty());
        let mut a = BatteryAlerts::default();
        assert!(a.check(Some(&batt(5.0, true)), 15).is_empty());
        // unplugging at 5 % → power-source alert and low alert
        assert_eq!(
            kinds(&a.check(Some(&batt(5.0, false)), 15)),
            vec![AlertKind::PowerSource, AlertKind::LowBattery]
        );
    }

    #[test]
    fn power_source_changes_only() {
        let mut a = BatteryAlerts::default();
        assert!(a.check(Some(&batt(50.0, false)), 15).is_empty());
        let v = a.check(Some(&batt(50.0, true)), 15);
        assert_eq!(kinds(&v), vec![AlertKind::PowerSource]);
        assert_eq!(v[0].title, "Sur secteur");
        assert_eq!(v[0].subtitle.as_deref(), Some("50 % · en charge"));
        assert!(a.check(Some(&batt(51.0, true)), 15).is_empty());
        assert!(a.check(None, 15).is_empty());
    }
}
