//! Internal battery from IOPowerSources (what `pmset -g batt` and System
//! Information show) plus `AppleSmartBattery` registry properties.

use crate::value::Value;
use nox_core::model::metrics::BatteryInfo;
use nox_core::{Reading, Unavailable};

/// The `AppleSmartBattery` properties read each sample (one registry call
/// each; the full property table is large and is never copied).
pub const REGISTRY_KEYS: [&str; 10] = [
    "BatteryInstalled",
    "Voltage",
    "Amperage",
    "IsCharging",
    "ExternalConnected",
    "CycleCount",
    "CurrentCapacity",
    "MaxCapacity",
    "BatteryData",
    "Temperature",
];

/// The IOPowerSources description of the internal battery, if any.
pub fn internal_source(descriptions: &[Value]) -> Option<&Value> {
    descriptions
        .iter()
        .find(|d| d.str("Type") == Some("InternalBattery"))
}

fn health_condition_fr(s: &str) -> String {
    match s {
        "Good" => "Normale".into(),
        "Fair" => "Correcte".into(),
        "Poor" => "Mauvaise".into(),
        "Check Battery" => "Vérifier la batterie".into(),
        "Permanent Battery Failure" => "Défaillance permanente".into(),
        other => other.to_owned(),
    }
}

/// Inputs gathered by the collector for one battery sample.
pub struct BatteryInputs<'a> {
    /// Registry properties keyed by name (subset of [`REGISTRY_KEYS`]).
    pub registry: Option<&'a Value>,
    /// IOPowerSources description of the internal battery.
    pub source: Option<&'a Value>,
    /// IOPSGetProvidingPowerSourceType: "AC Power" / "Battery Power" / …
    pub providing: Option<&'a str>,
    /// Hottest plausible battery SMC sensor (TB*T), °C.
    pub smc_temp_c: Option<f64>,
    /// SMC `PDTR` (DC-in total power), W.
    pub smc_adapter_w: Option<f64>,
}

pub fn decode(i: &BatteryInputs) -> Reading<BatteryInfo> {
    let reg = i.registry;
    let src = i.source;
    if reg.and_then(|r| r.bool("BatteryInstalled")) == Some(false)
        || (reg.is_none() && src.is_none())
    {
        return Reading::Unavailable(Unavailable::SourceMissing("Batterie interne".into()));
    }

    let percent = src
        .and_then(|s| Some((s.int("Current Capacity")?, s.int("Max Capacity")?)))
        .or_else(|| reg.and_then(|r| Some((r.int("CurrentCapacity")?, r.int("MaxCapacity")?))))
        .filter(|&(_, max)| max > 0)
        .map(|(cur, max)| (cur as f64 * 100.0 / max as f64).clamp(0.0, 100.0));
    let Some(percent) = percent else {
        return Reading::Unavailable(Unavailable::Error("battery charge not reported".into()));
    };

    let on_ac = match i.providing {
        Some(p) => p == "AC Power",
        None => src
            .and_then(|s| s.str("Power Source State"))
            .map(|s| s == "AC Power")
            .or_else(|| reg.and_then(|r| r.bool("ExternalConnected")))
            .unwrap_or(false),
    };
    let charging = src
        .and_then(|s| s.bool("Is Charging"))
        .or_else(|| reg.and_then(|r| r.bool("IsCharging")))
        .unwrap_or(false);

    let cycle_count = match reg.and_then(|r| r.int("CycleCount")) {
        Some(c) if c >= 0 => Reading::Measured(c as u32),
        _ => Reading::Unavailable(Unavailable::NotSupported),
    };

    Reading::Measured(BatteryInfo {
        percent,
        on_ac,
        charging,
        cycle_count,
        health_percent: health(reg, src),
        condition: condition(src),
        time_remaining_min: time_remaining(src, on_ac, charging),
        battery_watts: battery_watts(reg),
        adapter_watts: adapter_watts(on_ac, i.smc_adapter_w),
        temperature_c: temperature(reg, i.smc_temp_c),
    })
}

/// Primary: IOPS `BatteryPacks[].BatteryHealth."Maximum Capacity Percent"`,
/// the exact "Maximum Capacity" of System Information (Apple's smoothed
/// figure). Fallback: NominalChargeCapacity / DesignCapacity (smoothed full
/// charge capacity; the raw FullChargeCapacity moves with temperature and load)
/// as an estimate.
fn health(reg: Option<&Value>, src: Option<&Value>) -> Reading<f64> {
    let exact = src
        .and_then(|s| s.get("BatteryPacks"))
        .and_then(Value::as_array)
        .and_then(|packs| {
            packs.iter().find_map(|p| {
                p.get("BatteryHealth")
                    .and_then(|h| h.int("Maximum Capacity Percent"))
            })
        })
        .or_else(|| src.and_then(|s| s.int("Maximum Capacity Percent")))
        .filter(|&v| (1..=150).contains(&v));
    if let Some(v) = exact {
        return Reading::Measured(v as f64);
    }
    let data = reg.and_then(|r| r.get("BatteryData"));
    let field = |k: &str| {
        data.and_then(|d| d.int(k))
            .or_else(|| reg.and_then(|r| r.int(k)))
            .filter(|&v| v > 0)
    };
    match (field("NominalChargeCapacity"), field("DesignCapacity")) {
        (Some(n), Some(d)) => Reading::Estimated((n as f64 * 100.0 / d as f64).min(100.0)),
        _ => Reading::Unavailable(Unavailable::NotSupported),
    }
}

/// `BatteryHealthCondition` / `BatteryHealth` when published; otherwise the
/// IOPS "Battery Service State" (0 = no service recommended → "Normale").
fn condition(src: Option<&Value>) -> Reading<String> {
    let Some(src) = src else {
        return Reading::Unavailable(Unavailable::NotSupported);
    };
    if let Some(c) = src
        .str("BatteryHealthCondition")
        .or_else(|| src.str("BatteryHealth"))
    {
        return Reading::Measured(health_condition_fr(c));
    }
    match src.int("Battery Service State") {
        Some(0) => Reading::Measured("Normale".into()),
        Some(n) => Reading::Measured(format!("Service recommandé (état {n})")),
        None => Reading::Unavailable(Unavailable::NotSupported),
    }
}

/// System estimate (IOPS "Time to Empty"/"Time to Full Charge", minutes, the
/// value `pmset -g batt` prints). Negative = still calculating.
fn time_remaining(src: Option<&Value>, on_ac: bool, charging: bool) -> Reading<u32> {
    let key = if charging {
        "Time to Full Charge"
    } else if !on_ac {
        "Time to Empty"
    } else {
        // On AC and not charging (full or on hold): no estimate exists.
        return Reading::Unavailable(Unavailable::NotSupported);
    };
    match src.and_then(|s| s.int(key)) {
        Some(m) if m > 0 && m < 100 * 60 => Reading::Estimated(m as u32),
        Some(_) => Reading::pending(),
        None => Reading::Unavailable(Unavailable::NotSupported),
    }
}

/// Voltage (mV) × Amperage (mA, signed: negative while discharging).
fn battery_watts(reg: Option<&Value>) -> Reading<f64> {
    let v = reg.and_then(|r| r.int("Voltage")).filter(|&v| v > 0);
    let a = reg.and_then(|r| r.int("Amperage"));
    match (v, a) {
        (Some(v), Some(a)) if a.abs() < 100_000 => {
            Reading::Measured(v as f64 / 1000.0 * (a as f64 / 1000.0))
        }
        _ => Reading::Unavailable(Unavailable::NotSupported),
    }
}

fn adapter_watts(on_ac: bool, smc: Option<f64>) -> Reading<f64> {
    if !on_ac {
        return Reading::Unavailable(Unavailable::SourceMissing("Adaptateur secteur".into()));
    }
    match smc {
        Some(w) if (0.0..500.0).contains(&w) => Reading::Measured(w),
        _ => Reading::Unavailable(Unavailable::NotSupported),
    }
}

/// Registry `Temperature` (centi-°C, older macOS) or the SMC battery sensors
/// (macOS 27 no longer publishes `Temperature` in the registry).
fn temperature(reg: Option<&Value>, smc: Option<f64>) -> Reading<f64> {
    if let Some(t) = reg.and_then(|r| r.int("Temperature")) {
        let c = t as f64 / 100.0;
        if (-20.0..90.0).contains(&c) {
            return Reading::Measured(c);
        }
    }
    match smc {
        Some(c) => Reading::Measured(c),
        None => Reading::Unavailable(Unavailable::NotSupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::dict;
    use Value::*;

    /// Registry subset as published by the M2 Air on macOS 27 (no Temperature).
    fn registry() -> Value {
        dict([
            ("BatteryInstalled", Bool(true)),
            ("Voltage", Int(11_514)),
            ("Amperage", Int(-199)),
            ("IsCharging", Bool(false)),
            ("ExternalConnected", Bool(false)),
            ("CycleCount", Int(276)),
            ("CurrentCapacity", Int(47)),
            ("MaxCapacity", Int(100)),
            (
                "BatteryData",
                dict([
                    ("DesignCapacity", Int(4563)),
                    ("NominalChargeCapacity", Int(3932)),
                    ("FullChargeCapacity", Int(3805)),
                ]),
            ),
        ])
    }

    /// IOPS description (`pmset -g ps -xml`) from the same machine.
    fn source() -> Value {
        dict([
            ("Type", Str("InternalBattery".into())),
            ("Current Capacity", Int(47)),
            ("Max Capacity", Int(100)),
            ("Is Charging", Bool(false)),
            ("Power Source State", Str("Battery Power".into())),
            ("Time to Empty", Int(254)),
            ("Time to Full Charge", Int(0)),
            ("Battery Service State", Int(0)),
            (
                "BatteryPacks",
                Array(vec![dict([(
                    "BatteryHealth",
                    dict([("Maximum Capacity Percent", Int(87))]),
                )])]),
            ),
        ])
    }

    fn inputs<'a>(reg: Option<&'a Value>, src: Option<&'a Value>) -> BatteryInputs<'a> {
        BatteryInputs {
            registry: reg,
            source: src,
            providing: Some("Battery Power"),
            smc_temp_c: Some(30.3),
            smc_adapter_w: Some(0.0),
        }
    }

    #[test]
    fn decodes_discharging_m2() {
        let (r, s) = (registry(), source());
        let b = decode(&inputs(Some(&r), Some(&s)));
        let b = b.value().unwrap();
        assert_eq!(b.percent, 47.0);
        assert!(!b.on_ac && !b.charging);
        assert_eq!(b.cycle_count, Reading::Measured(276));
        assert_eq!(b.health_percent, Reading::Measured(87.0));
        assert_eq!(b.condition, Reading::Measured("Normale".into()));
        assert_eq!(b.time_remaining_min, Reading::Estimated(254));
        let w = b.battery_watts.value().copied().unwrap();
        assert!((w - (-2.291_286)).abs() < 1e-6, "{w}");
        assert!(matches!(
            b.adapter_watts,
            Reading::Unavailable(Unavailable::SourceMissing(_))
        ));
        assert_eq!(b.temperature_c, Reading::Measured(30.3));
    }

    #[test]
    fn health_falls_back_to_estimate() {
        let r = registry();
        let b = decode(&inputs(Some(&r), None));
        let b = b.value().unwrap();
        assert!(b.health_percent.is_estimated());
        let h = *b.health_percent.value().unwrap();
        assert!((h - 3932.0 * 100.0 / 4563.0).abs() < 1e-9);
        assert_eq!(b.percent, 47.0);
        assert_eq!(
            b.time_remaining_min,
            Reading::Unavailable(Unavailable::NotSupported)
        );
    }

    #[test]
    fn charging_on_ac() {
        let r = registry();
        let mut s = source();
        if let Dict(m) = &mut s {
            m.insert("Is Charging".into(), Bool(true));
            m.insert("Time to Full Charge".into(), Int(-1));
        }
        let mut i = inputs(Some(&r), Some(&s));
        i.providing = Some("AC Power");
        i.smc_adapter_w = Some(14.2);
        let b = decode(&i);
        let b = b.value().unwrap();
        assert!(b.on_ac && b.charging);
        assert_eq!(b.time_remaining_min, Reading::pending());
        assert_eq!(b.adapter_watts, Reading::Measured(14.2));
    }

    #[test]
    fn no_battery() {
        let r = dict([("BatteryInstalled", Bool(false))]);
        assert!(matches!(
            decode(&inputs(Some(&r), None)),
            Reading::Unavailable(Unavailable::SourceMissing(_))
        ));
        assert!(decode(&inputs(None, None)).value().is_none());
    }

    #[test]
    fn registry_temperature_preferred_when_present() {
        let mut r = registry();
        if let Dict(m) = &mut r {
            m.insert("Temperature".into(), Int(3125));
        }
        let b = decode(&inputs(Some(&r), None));
        assert_eq!(b.value().unwrap().temperature_c, Reading::Measured(31.25));
    }

    #[test]
    fn finds_internal_source() {
        let list = vec![dict([("Type", Str("UPS".into()))]), source()];
        assert_eq!(
            internal_source(&list).and_then(|s| s.int("Time to Empty")),
            Some(254)
        );
    }
}
