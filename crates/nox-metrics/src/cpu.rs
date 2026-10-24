//! CPU usage from `host_processor_info` tick deltas, with P/E core kinds.

use crate::sys::mach::CpuTicks;
use nox_core::model::metrics::{CoreKind, CoreUsage};

/// Usage of every CPU between two tick snapshots, 0..100, plus the
/// tick-weighted total. `None` if the CPU count changed or no tick elapsed.
pub fn usage(prev: &[CpuTicks], cur: &[CpuTicks]) -> Option<(f64, Vec<f64>)> {
    if prev.len() != cur.len() || cur.is_empty() {
        return None;
    }
    let mut busy_all = 0u64;
    let mut total_all = 0u64;
    let mut per = Vec::with_capacity(cur.len());
    for (p, c) in prev.iter().zip(cur) {
        // Ticks are 32-bit counters: wrapping_sub survives a wrap-around.
        let d: [u64; 4] = std::array::from_fn(|i| u64::from(c[i].wrapping_sub(p[i])));
        let busy = d[0] + d[1] + d[3];
        let total = busy + d[2];
        busy_all += busy;
        total_all += total;
        per.push(if total == 0 {
            0.0
        } else {
            busy as f64 * 100.0 / total as f64
        });
    }
    (total_all > 0).then(|| (busy_all as f64 * 100.0 / total_all as f64, per))
}

/// Kind of each logical CPU, by `logical-cpu-id`.
///
/// Primary source: the `cluster-type` ("E"/"P") property of the registry's
/// cpu nodes. Fallback: `hw.perflevel1.logicalcpu` efficiency cores first, as
/// on every Apple-silicon Mac so far (cpu0..3 = E on M1/M2).
pub fn core_kinds(
    registry: &[(u32, CoreKind)],
    n_cpus: usize,
    efficiency_count: Option<usize>,
) -> Vec<CoreKind> {
    if registry.len() == n_cpus {
        let mut v = vec![CoreKind::Performance; n_cpus];
        let mut seen = vec![false; n_cpus];
        for &(id, kind) in registry {
            if let Some(slot) = v.get_mut(id as usize) {
                *slot = kind;
                seen[id as usize] = true;
            }
        }
        if seen.iter().all(|&s| s) {
            return v;
        }
    }
    let e = efficiency_count.unwrap_or(0).min(n_cpus);
    (0..n_cpus)
        .map(|i| {
            if i < e {
                CoreKind::Efficiency
            } else {
                CoreKind::Performance
            }
        })
        .collect()
}

pub fn cores(per: &[f64], kinds: &[CoreKind]) -> Vec<CoreUsage> {
    per.iter()
        .enumerate()
        .map(|(index, &usage)| CoreUsage {
            index,
            kind: kinds.get(index).copied().unwrap_or(CoreKind::Performance),
            usage,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_from_deltas() {
        let prev = vec![[100, 50, 850, 0], [0, 0, 0, 0]];
        let cur = vec![[150, 100, 900, 0], [10, 0, 90, 0]];
        let (total, per) = usage(&prev, &cur).unwrap();
        assert!((per[0] - 100.0 * 100.0 / 150.0).abs() < 1e-9);
        assert_eq!(per[1], 10.0);
        assert!((total - (110.0 / 250.0 * 100.0)).abs() < 1e-9);
    }

    #[test]
    fn usage_survives_counter_wrap() {
        let prev = vec![[u32::MAX - 9, 0, 0, 0]];
        let cur = vec![[10, 0, 20, 0]];
        let (total, _) = usage(&prev, &cur).unwrap();
        assert!((total - 50.0).abs() < 1e-9);
    }

    #[test]
    fn usage_rejects_cpu_count_change_and_zero_time() {
        assert!(usage(&[[0; 4]], &[[0; 4], [0; 4]]).is_none());
        assert!(usage(&[[5; 4]], &[[5; 4]]).is_none());
    }

    #[test]
    fn kinds_from_registry_then_fallback() {
        use CoreKind::*;
        let reg = [
            (1, Efficiency),
            (0, Efficiency),
            (2, Performance),
            (3, Performance),
        ];
        assert_eq!(
            core_kinds(&reg, 4, None),
            vec![Efficiency, Efficiency, Performance, Performance]
        );
        assert_eq!(
            core_kinds(&[], 4, Some(1)),
            vec![Efficiency, Performance, Performance, Performance]
        );
        // incomplete registry → fallback
        assert_eq!(core_kinds(&reg[..2], 4, Some(2))[2], Performance);
    }
}
