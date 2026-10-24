//! Throughput of the primary interface from 64-bit interface byte counters.

use nox_core::model::metrics::NetworkInfo;
use nox_core::{Reading, Unavailable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfCounters {
    pub name: String,
    pub up: bool,
    pub loopback: bool,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Default)]
pub struct NetTracker {
    prev: Option<(String, u64, u64, f64)>,
}

impl NetTracker {
    /// `now` in seconds (monotonic); `max_gap` beyond which a delta is no
    /// longer a current rate (sleep, long pause) and only re-baselines.
    pub fn sample(
        &mut self,
        primary: Option<&str>,
        counters: &[IfCounters],
        now: f64,
        max_gap: f64,
    ) -> Reading<NetworkInfo> {
        let Some(name) = primary else {
            self.prev = None;
            return Reading::Unavailable(Unavailable::SourceMissing("Connexion réseau".into()));
        };
        let Some(c) = counters.iter().find(|c| c.name == name) else {
            self.prev = None;
            return Reading::Unavailable(Unavailable::Error(format!(
                "interface {name} not listed"
            )));
        };
        let prev = self.prev.replace((name.to_owned(), c.rx, c.tx, now));
        let Some((pname, prx, ptx, pt)) = prev else {
            return Reading::pending();
        };
        let dt = now - pt;
        if pname != name || dt <= 0.0 || dt > max_gap || c.rx < prx || c.tx < ptx {
            return Reading::pending();
        }
        Reading::Measured(NetworkInfo {
            rx_bytes_per_sec: (c.rx - prx) as f64 / dt,
            tx_bytes_per_sec: (c.tx - ptx) as f64 / dt,
            rx_total: c.rx,
            tx_total: c.tx,
            primary_interface: Some(name.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en0(rx: u64, tx: u64) -> Vec<IfCounters> {
        vec![
            IfCounters {
                name: "lo0".into(),
                up: true,
                loopback: true,
                rx: 9,
                tx: 9,
            },
            IfCounters {
                name: "en0".into(),
                up: true,
                loopback: false,
                rx,
                tx,
            },
        ]
    }

    #[test]
    fn rates_after_baseline() {
        let mut t = NetTracker::default();
        assert_eq!(
            t.sample(Some("en0"), &en0(1000, 500), 10.0, 30.0),
            Reading::pending()
        );
        let r = t.sample(Some("en0"), &en0(5000, 1500), 12.0, 30.0);
        let n = r.value().unwrap();
        assert_eq!(n.rx_bytes_per_sec, 2000.0);
        assert_eq!(n.tx_bytes_per_sec, 500.0);
        assert_eq!((n.rx_total, n.tx_total), (5000, 1500));
        assert_eq!(n.primary_interface.as_deref(), Some("en0"));
    }

    #[test]
    fn gap_reset_and_switch_rebaseline() {
        let mut t = NetTracker::default();
        t.sample(Some("en0"), &en0(1000, 500), 0.0, 30.0);
        // long gap (sleep)
        assert_eq!(
            t.sample(Some("en0"), &en0(9000, 900), 100.0, 30.0),
            Reading::pending()
        );
        // counter reset
        assert_eq!(
            t.sample(Some("en0"), &en0(10, 10), 102.0, 30.0),
            Reading::pending()
        );
        assert!(
            t.sample(Some("en0"), &en0(20, 20), 104.0, 30.0)
                .value()
                .is_some()
        );
        // offline
        assert!(matches!(
            t.sample(None, &en0(30, 30), 106.0, 30.0),
            Reading::Unavailable(Unavailable::SourceMissing(_))
        ));
        assert_eq!(
            t.sample(Some("en0"), &en0(40, 40), 108.0, 30.0),
            Reading::pending()
        );
    }
}
