// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Metrics/MetricFormat.swift (memoryUsed/appMemory), commit e916961.
//! Memory figures matching Activity Monitor's Memory tab.

use nox_core::Reading;
use nox_core::model::metrics::{MemoryInfo, MemoryPressure};

/// Page counts from `host_statistics64(HOST_VM_INFO64)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VmPages {
    pub free: u64,
    pub active: u64,
    pub inactive: u64,
    pub speculative: u64,
    pub wired: u64,
    pub purgeable: u64,
    pub compressor: u64,
    pub external: u64,
    pub internal: u64,
    /// Reserved tagged-memory storage (macOS 26+ rev3 field, else 0).
    pub tag_storage: u64,
}

/// `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warning, 4 critical.
pub fn pressure(level: Option<i32>) -> Reading<MemoryPressure> {
    match level {
        Some(1) => Reading::Measured(MemoryPressure::Normal),
        Some(2) => Reading::Measured(MemoryPressure::Warning),
        Some(4) => Reading::Measured(MemoryPressure::Critical),
        Some(other) => Reading::Unavailable(nox_core::Unavailable::Error(format!(
            "unknown pressure level {other}"
        ))),
        None => Reading::Unavailable(nox_core::Unavailable::Error(
            "kern.memorystatus_vm_pressure_level".into(),
        )),
    }
}

/// App = internal − purgeable; Used = App + Wired + Compressed (+ tag storage);
/// Cached files = external + purgeable. All clamped to `total`.
pub fn info(
    total: u64,
    page_size: u64,
    p: VmPages,
    pressure: Reading<MemoryPressure>,
    swap: (u64, u64),
) -> MemoryInfo {
    let bytes = |pages: u64| pages.saturating_mul(page_size).min(total);
    let tag = if p.tag_storage <= total / page_size.max(1) {
        p.tag_storage
    } else {
        0
    };
    let app = bytes(p.internal.saturating_sub(p.purgeable));
    let wired = bytes(p.wired);
    let compressed = bytes(p.compressor);
    let used = app
        .saturating_add(wired)
        .saturating_add(compressed)
        .saturating_add(bytes(tag))
        .min(total);
    MemoryInfo {
        total,
        used,
        app,
        wired,
        compressed,
        cached: bytes(p.external.saturating_add(p.purgeable)),
        pressure,
        swap_used: swap.0,
        swap_total: swap.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Values captured from `vm_stat` on the M2 (16 KiB pages, 16 GiB).
    fn m2_pages() -> VmPages {
        VmPages {
            free: 103_506,
            active: 156_418,
            inactive: 162_694,
            speculative: 497,
            wired: 119_809,
            purgeable: 1_973,
            compressor: 468_974,
            external: 162_629,
            internal: 156_980,
            tag_storage: 0,
        }
    }

    #[test]
    fn activity_monitor_formula() {
        let ps = 16_384;
        let total = 16 * 1024 * 1024 * 1024;
        let m = info(total, ps, m2_pages(), pressure(Some(2)), (10, 20));
        assert_eq!(m.app, (156_980 - 1_973) * ps);
        assert_eq!(m.wired, 119_809 * ps);
        assert_eq!(m.compressed, 468_974 * ps);
        assert_eq!(m.used, m.app + m.wired + m.compressed);
        assert_eq!(m.cached, (162_629 + 1_973) * ps);
        assert_eq!(m.pressure, Reading::Measured(MemoryPressure::Warning));
        assert_eq!((m.swap_used, m.swap_total), (10, 20));
    }

    #[test]
    fn clamps_and_ignores_absurd_tag_storage() {
        let mut p = m2_pages();
        p.tag_storage = u64::MAX;
        p.compressor = u64::MAX / 2;
        let m = info(1 << 30, 16_384, p, pressure(Some(1)), (0, 0));
        assert_eq!(m.used, 1 << 30);
        assert!(m.compressed <= 1 << 30);
    }

    #[test]
    fn pressure_levels() {
        assert_eq!(
            pressure(Some(4)),
            Reading::Measured(MemoryPressure::Critical)
        );
        assert!(pressure(Some(3)).value().is_none());
        assert!(pressure(None).value().is_none());
    }
}
