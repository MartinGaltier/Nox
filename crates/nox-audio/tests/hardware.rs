//! Real-hardware checks. `cargo test -p nox-audio --release -- --ignored --nocapture`.
//!
//! None of these creates a process tap except `live_tap_on_playing_app`,
//! which additionally requires `NOX_ALLOW_TAP=1`: a tap shows the system
//! "System Audio Recording" prompt and is a user-side validation.

use std::time::Duration;

#[test]
#[ignore = "reads this Mac's audio clients (no tap, no permission)"]
fn lists_audio_clients_and_owners() {
    assert!(nox_audio::supported());
    let out = nox_audio::default_output().expect("default output");
    println!("default output: {out:?}");
    let clients = nox_audio::audio_clients();
    println!("{} audio clients", clients.len());
    assert!(!clients.is_empty());
    // Every helper whose bundle id ends in ".helper" and has an owner must be
    // owned by a different, regular app process.
    for c in clients.iter().filter(|c| c.owner_pid.is_some()) {
        println!("{c:?}");
    }
}

#[test]
#[ignore = "creates a private aggregate on the default output, no tap"]
fn aggregate_and_ioproc_without_tap() {
    match nox_audio::diag::aggregate_without_tap(Duration::from_secs(20)) {
        Ok((cycles, cpu)) => {
            println!(
                "20 s steady: {cycles} IO cycles, process CPU {:.1} ms ({:.3} % of one core)",
                cpu.as_secs_f64() * 1e3,
                cpu.as_secs_f64() * 100.0 / 20.0
            );
            assert!(cycles > 100, "IOProc ran");
        }
        Err(e) => println!("skipped: {e}"),
    }
}

#[test]
#[ignore = "pure analysis cost"]
fn analysis_cost_per_update() {
    let d = nox_audio::diag::analysis_cost(3000);
    println!(
        "analysis: {:.1} µs per update, {:.3} % of one core at 30 Hz",
        d.as_secs_f64() * 1e6,
        d.as_secs_f64() * 30.0 * 100.0
    );
    assert!(d < Duration::from_millis(1));
}

#[test]
#[ignore = "CREATES A TAP (system prompt): user validation with NOX_ALLOW_TAP=1 and NOX_TAP_PID=<player pid>"]
fn live_tap_on_playing_app() {
    if std::env::var("NOX_ALLOW_TAP").as_deref() != Ok("1") {
        println!("NOX_ALLOW_TAP not set: no tap created");
        return;
    }
    let pid: i32 = std::env::var("NOX_TAP_PID")
        .expect("NOX_TAP_PID")
        .parse()
        .expect("pid");
    let (tx, rx) = std::sync::mpsc::channel();
    let cpu0 = nox_audio::diag::cpu_time();
    let mut m = nox_audio::LevelMeter::start(pid, Default::default(), move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    let mut levels = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(nox_audio::MeterEvent::Levels(l)) => {
                levels += 1;
                if levels % 30 == 0 {
                    println!("{l:.2?}");
                }
            }
            Ok(other) => {
                println!("{other:?}");
                break;
            }
            Err(_) => {}
        }
    }
    m.stop();
    let cpu = nox_audio::diag::cpu_time().saturating_sub(cpu0);
    println!(
        "{levels} level updates; process CPU {:.1} ms over ~15 s",
        cpu.as_secs_f64() * 1e3
    );
    assert!(
        levels > 0,
        "levels received (permission granted and player sounding?)"
    );
}

#[test]
#[ignore = "runs the mixer module with every app at 100 % (no tap is created)"]
fn mixer_lists_apps_without_tapping() {
    use nox_core::{Hub, ModuleContext, Settings};
    let hub = Hub::new();
    let mut m = nox_audio::mixer_module();
    m.start(ModuleContext {
        hub: hub.clone(),
        settings: std::sync::Arc::new(Settings::default()),
    })
    .unwrap();
    let t = std::time::Instant::now();
    let mut apps = vec![];
    while t.elapsed() < Duration::from_secs(5) {
        apps = hub.read(|s| s.mixer.as_ref().map(|m| m.apps.clone()).unwrap_or_default());
        if !apps.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for a in &apps {
        println!(
            "{} ({:?}) pid {} playing={} vol={} engaged={}",
            a.name, a.bundle_id, a.pid, a.playing, a.volume, a.engaged
        );
    }
    assert!(!apps.is_empty());
    assert!(
        apps.iter().all(|a| !a.engaged && a.volume == 1.0),
        "no tap at 100 %"
    );
    m.stop();
    assert!(hub.read(|s| s.mixer.is_none()));
}
