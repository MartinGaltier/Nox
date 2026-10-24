//! One line per sample: `cargo run --release -p nox-metrics --example watch [secs] [interval_ms]`.
use std::time::{Duration, Instant};

fn main() {
    let mut a = std::env::args().skip(1);
    let secs: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let ms: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(2000);
    let mut sample = nox_metrics::diag::sampler();
    let start = Instant::now();
    sample();
    while start.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_millis(ms));
        let s = sample();
        let f = |r: &nox_core::Reading<f64>| r.value().map_or("—".into(), |v| format!("{v:.1}"));
        let p = s.power.value();
        println!(
            "t={:5.1}s cpu={}% gpu={}% cpu_t={}°C gpu_t={}°C sys={}W gpu_w={}W cpu_w={:?}",
            start.elapsed().as_secs_f64(),
            f(&s.cpu_total),
            f(&s.gpu_usage),
            f(&s.cpu_temp),
            f(&s.gpu_temp),
            p.map_or("—".into(), |p| f(&p.system_watts)),
            p.map_or("—".into(), |p| p
                .gpu_watts
                .value()
                .map_or("—".into(), |v| format!("{v:.3}"))),
            p.map(|p| p.cpu_watts.clone()),
        );
    }
}
