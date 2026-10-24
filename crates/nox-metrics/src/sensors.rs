// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Metrics/TemperatureSensorSelector.swift (per-chip CPU core key sets), commit e916961.
//! Which SMC temperature keys mean what, and how CPU/GPU are aggregated.
//!
//! Aggregation (documented in docs/METRICS_SOURCES.md):
//! * CPU = hottest plausible sensor of the chip's known core set (the core
//!   that throttles first); if none answers, hottest `Tp*`/`Te*` sensor.
//! * GPU = hottest plausible `Tg*` sensor.
//! * Battery = hottest `TB<n>T` sensor.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    M1,
    M2,
    M3,
    M4,
    M4Pro,
    M5,
    Other,
}

impl Platform {
    pub fn from_brand(brand: &str) -> Self {
        let b = brand.trim();
        if b == "Apple M4 Pro" {
            return Self::M4Pro;
        }
        let Some(rest) = b.strip_prefix("Apple M") else {
            return Self::Other;
        };
        let mut chars = rest.chars();
        let generation = chars.next().and_then(|c| c.to_digit(10));
        if !matches!(chars.next(), None | Some(' ')) {
            return Self::Other;
        }
        match generation {
            Some(1) => Self::M1,
            Some(2) => Self::M2,
            Some(3) => Self::M3,
            Some(4) => Self::M4,
            Some(5) => Self::M5,
            _ => Self::Other,
        }
    }

    fn core_keys(self) -> &'static [&'static str] {
        match self {
            Self::M1 => &[
                "Tp09", "Tp0T", "Tp01", "Tp05", "Tp0D", "Tp0H", "Tp0L", "Tp0P", "Tp0X", "Tp0b",
            ],
            Self::M2 => &[
                "Tp1h", "Tp1t", "Tp1p", "Tp1l", "Tp01", "Tp05", "Tp09", "Tp0D", "Tp0X", "Tp0b",
                "Tp0f", "Tp0j",
            ],
            Self::M3 => &[
                "Te05", "Te0L", "Te0P", "Te0S", "Tf04", "Tf09", "Tf0A", "Tf0B", "Tf0D", "Tf0E",
                "Tf44", "Tf49", "Tf4A", "Tf4B", "Tf4D", "Tf4E",
            ],
            Self::M4 => &[
                "Te05", "Te0S", "Te09", "Te0H", "Tp01", "Tp05", "Tp09", "Tp0D", "Tp0V", "Tp0Y",
                "Tp0b", "Tp0e",
            ],
            Self::M4Pro => &[
                "Tex0", "Tex1", "Tex2", "Tex3", "Tpx0", "Tpx1", "Tpx2", "Tpx3", "Tpx4", "Tpx5",
                "Tpx8", "Tpx9", "TpxA", "TpxB", "TpxC", "TpxD",
            ],
            Self::M5 => &[
                "Tp00", "Tp04", "Tp08", "Tp0C", "Tp0G", "Tp0K", "Tp0O", "Tp0R", "Tp0U", "Tp0X",
                "Tp0a", "Tp0d", "Tp0g", "Tp0j", "Tp0m", "Tp0p", "Tp0u", "Tp0y",
            ],
            Self::Other => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    CpuCore,
    CpuOther,
    Gpu,
    Battery,
    Ssd,
    Wifi,
}

impl Role {
    pub fn group(self) -> &'static str {
        match self {
            Role::CpuCore | Role::CpuOther => "CPU",
            Role::Gpu => "GPU",
            Role::Battery => "Batterie",
            Role::Ssd => "SSD",
            Role::Wifi => "Wi-Fi",
        }
    }
}

/// Role of an SMC key, or `None` if Nox does not use it (unknown meaning).
pub fn classify(key: &str, platform: Platform) -> Option<Role> {
    let b = key.as_bytes();
    if b.len() != 4 || b[0] != b'T' {
        return None;
    }
    if platform.core_keys().contains(&key) {
        return Some(Role::CpuCore);
    }
    match &key[..2] {
        "Tp" | "Te" => Some(Role::CpuOther),
        "Tf" if platform == Platform::M3 => Some(Role::CpuOther),
        "Tg" => Some(Role::Gpu),
        "TB" if b[2].is_ascii_digit() && b[3] == b'T' => Some(Role::Battery),
        "TH" if b[2] == b'0' => Some(Role::Ssd),
        _ if key == "TW0P" => Some(Role::Wifi),
        _ => None,
    }
}

/// Chip sensors read 0 or garbage when a block is power-gated.
pub fn plausible(role: Role, c: f64) -> bool {
    match role {
        Role::Battery => (-10.0..80.0).contains(&c),
        _ => (10.0..125.0).contains(&c),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub key: String,
    pub role: Role,
    pub celsius: f64,
}

fn max_of(samples: &[Sample], f: impl Fn(Role) -> bool) -> Option<f64> {
    samples
        .iter()
        .filter(|s| f(s.role))
        .map(|s| s.celsius)
        .fold(None, |m, v| Some(m.map_or(v, |m: f64| m.max(v))))
}

pub fn cpu_temp(samples: &[Sample]) -> Option<f64> {
    max_of(samples, |r| r == Role::CpuCore).or_else(|| max_of(samples, |r| r == Role::CpuOther))
}

pub fn gpu_temp(samples: &[Sample]) -> Option<f64> {
    max_of(samples, |r| r == Role::Gpu)
}

pub fn battery_temp(samples: &[Sample]) -> Option<f64> {
    max_of(samples, |r| r == Role::Battery)
}

/// The list shown on the System page: core, GPU, battery, SSD and Wi-Fi
/// sensors (other `Tp`/`Te` keys only feed the CPU fallback).
pub fn shown(samples: &[Sample]) -> Vec<nox_core::model::metrics::Sensor> {
    let mut counters = std::collections::HashMap::<&str, usize>::new();
    samples
        .iter()
        .filter(|s| s.role != Role::CpuOther)
        .map(|s| {
            let group = s.role.group();
            let n = counters.entry(group).or_default();
            *n += 1;
            let label = match s.role {
                Role::CpuCore => format!("CPU {n}"),
                Role::Gpu => format!("GPU {n}"),
                _ => format!("{group} {n}"),
            };
            nox_core::model::metrics::Sensor {
                key: s.key.clone(),
                label,
                group: group.to_owned(),
                celsius: s.celsius,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_from_brand() {
        assert_eq!(Platform::from_brand("Apple M2"), Platform::M2);
        assert_eq!(Platform::from_brand("Apple M2 Max"), Platform::M2);
        assert_eq!(Platform::from_brand("Apple M4 Pro"), Platform::M4Pro);
        assert_eq!(Platform::from_brand("Apple M12"), Platform::Other);
        assert_eq!(
            Platform::from_brand("Intel(R) Core(TM) i7"),
            Platform::Other
        );
    }

    #[test]
    fn classify_m2_keys() {
        let p = Platform::M2;
        assert_eq!(classify("Tp01", p), Some(Role::CpuCore));
        assert_eq!(classify("Tp02", p), Some(Role::CpuOther));
        assert_eq!(classify("Tg0f", p), Some(Role::Gpu));
        assert_eq!(classify("TB1T", p), Some(Role::Battery));
        assert_eq!(classify("TG0B", p), None);
        assert_eq!(classify("TH0T", p), Some(Role::Ssd));
        assert_eq!(classify("TW0P", p), Some(Role::Wifi));
        assert_eq!(classify("Ts0P", p), None);
        assert_eq!(classify("PSTR", p), None);
    }

    fn s(key: &str, role: Role, c: f64) -> Sample {
        Sample {
            key: key.into(),
            role,
            celsius: c,
        }
    }

    #[test]
    fn aggregation() {
        let v = vec![
            s("Tp01", Role::CpuCore, 52.0),
            s("Tp05", Role::CpuCore, 61.5),
            s("Tp02", Role::CpuOther, 75.0),
            s("Tg0f", Role::Gpu, 45.0),
            s("Tg0n", Role::Gpu, 49.0),
            s("TB0T", Role::Battery, 30.3),
        ];
        assert_eq!(cpu_temp(&v), Some(61.5));
        assert_eq!(gpu_temp(&v), Some(49.0));
        assert_eq!(battery_temp(&v), Some(30.3));
        // core set silent → fallback to other CPU sensors
        assert_eq!(cpu_temp(&v[2..]), Some(75.0));
        assert_eq!(cpu_temp(&[]), None);
        let shown = shown(&v);
        assert_eq!(shown.len(), 5);
        assert_eq!(shown[1].label, "CPU 2");
        assert_eq!(shown[3].label, "GPU 2");
        assert_eq!(shown[4].label, "Batterie 1");
    }

    #[test]
    fn plausibility() {
        assert!(!plausible(Role::CpuCore, 0.0));
        assert!(plausible(Role::CpuCore, 48.0));
        assert!(plausible(Role::Battery, 5.0));
        assert!(!plausible(Role::Gpu, 2.8e14));
    }
}
