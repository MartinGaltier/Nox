//! Real-hardware checks (Apple-silicon Mac). Run: `cargo test -p nox-metrics -- --ignored --nocapture`.

use nox_core::model::metrics::{CoreKind, Series};
use nox_core::{Command, Hub, ModuleContext, Reading, Settings, Unavailable};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn sysctl(name: &str) -> String {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .output()
        .expect("sysctl");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
#[ignore = "reads this Mac's hardware"]
fn snapshot_is_plausible() {
    let s = nox_metrics::diag::snapshot(Duration::from_secs(1));
    println!("{s:#?}");

    let cpu = *s.cpu_total.value().expect("cpu measured");
    assert!((0.0..=100.0).contains(&cpu));
    let cores = s.cpu_cores.value().expect("cores measured");
    let ncpu: usize = sysctl("hw.ncpu").parse().unwrap();
    assert_eq!(cores.len(), ncpu);
    let e: usize = sysctl("hw.perflevel1.logicalcpu").parse().unwrap_or(0);
    assert_eq!(
        cores
            .iter()
            .filter(|c| c.kind == CoreKind::Efficiency)
            .count(),
        e
    );

    let gpu = *s.gpu_usage.value().expect("gpu measured");
    assert!((0.0..=100.0).contains(&gpu));

    let t = *s.cpu_temp.value().expect("cpu temp");
    assert!((10.0..110.0).contains(&t), "cpu temp {t}");
    let t = *s.gpu_temp.value().expect("gpu temp");
    assert!((10.0..110.0).contains(&t), "gpu temp {t}");
    assert!(!s.sensors.value().unwrap().is_empty());

    let m = s.memory.value().expect("memory");
    assert_eq!(m.total.to_string(), sysctl("hw.memsize"));
    assert!(m.used <= m.total && m.app <= m.used);
    assert!(m.pressure.value().is_some());

    if let Some(b) = s.battery.value() {
        assert!((0.0..=100.0).contains(&b.percent));
        assert!(b.cycle_count.value().is_some());
        let h = *b.health_percent.value().expect("health");
        assert!((50.0..=100.0).contains(&h));
        assert!(b.battery_watts.value().is_some());
    }

    let p = s.power.value().expect("power");
    let w = *p.system_watts.value().expect("system power");
    assert!((0.1..200.0).contains(&w), "system {w} W");

    let n = s.network.value().expect("network");
    assert!(n.primary_interface.is_some());

    let disks = s.disks.value().expect("disks");
    assert!(
        disks
            .iter()
            .any(|d| d.mount == "/" && d.total > d.available)
    );

    // M2 MacBook Air: fanless.
    if sysctl("hw.model") == "Mac14,2" {
        assert_eq!(s.fans_rpm, Reading::Unavailable(Unavailable::NotSupported));
    }
}

#[test]
#[ignore = "runs the real worker"]
fn module_lifecycle_publishes_and_cleans_up() {
    let hub = Hub::new();
    let mut settings = Settings::default();
    settings.metrics.interval_ms = 300;
    settings.metrics.background_interval_ms = 60_000;
    let mut m = nox_metrics::module();
    m.start(ModuleContext {
        hub: hub.clone(),
        settings: Arc::new(settings.clone()),
    })
    .unwrap();
    assert!(hub.read(|s| s.metrics.is_some()));

    // Hidden: first sample only (background period is 60 s). Becoming
    // visible must sample again right away, then every 300 ms.
    std::thread::sleep(Duration::from_millis(500));
    let len = |hub: &Hub| {
        hub.read(|s| {
            s.metrics
                .as_ref()
                .and_then(|m| m.history.get(Series::Cpu).map(|q| q.len()))
                .unwrap_or(0)
        })
    };
    assert_eq!(len(&hub), 1);
    m.command(Command::MetricsVisible(true));
    let start = Instant::now();
    while len(&hub) < 4 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(len(&hub) >= 4, "visible interval not applied");
    let cpu = hub.read(|s| s.metrics.as_ref().unwrap().snapshot.cpu_total.clone());
    assert!(cpu.value().is_some(), "{cpu:?}");

    // Settings change applies without restart.
    settings.metrics.history_len = 2;
    m.settings_changed(&Arc::new(settings));
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(len(&hub), 2);

    m.stop();
    assert!(hub.read(|s| s.metrics.is_none()));
    let generation = hub.generation();
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(hub.generation(), generation, "no publication after stop");
    m.stop(); // idempotent
}

#[test]
#[ignore = "60 s benchmark"]
fn collector_cost_60s_at_2s() {
    let (n, cpu, wall, setup) =
        nox_metrics::diag::bench(Duration::from_secs(60), Duration::from_secs(2));
    let pct = cpu.as_secs_f64() * 100.0 / wall.as_secs_f64();
    println!(
        "samples={n} cpu={:.1} ms wall={:.1} s per-sample={:.2} ms avg-load={pct:.3}% setup={:.1} ms",
        cpu.as_secs_f64() * 1e3,
        wall.as_secs_f64(),
        cpu.as_secs_f64() * 1e3 / n as f64,
        setup.as_secs_f64() * 1e3
    );
    assert!(pct < 1.0, "collector too expensive: {pct:.3}%");
}
