//! Starts the four activities modules through the real ModuleManager for a
//! few seconds, prints a summary (aggregates only), then shuts down and
//! checks that no `nox-*` activities thread is left.
//! Watch stays off unless enabled (default), so no capture happens.

use nox_core::model::timers::TimerCommand;
use nox_core::{Command, Hub, Settings, manager::ModuleManager};
use std::sync::Arc;
use std::time::Duration;

fn threads() -> usize {
    // Mach thread count of this process.
    let out = std::process::Command::new("/bin/ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .count()
        .saturating_sub(1)
}

fn main() {
    let dir = std::env::temp_dir().join(format!("nox-smoke-{}", std::process::id()));
    // SAFETY: set before any other thread is spawned; points timers state at
    // a throwaway folder.
    unsafe { std::env::set_var("NOX_DATA_DIR", &dir) };
    let before = threads();
    let hub = Hub::new();
    let mut s = Settings::default();
    s.timers.sound = false;
    let (mut mgr, running) =
        ModuleManager::spawn(hub.clone(), nox_activities::modules(), Arc::new(s));
    std::thread::sleep(Duration::from_millis(300));
    hub.send(Command::Timers(TimerCommand::Start {
        label: "smoke".into(),
        secs: 1.0,
    }));
    std::thread::sleep(Duration::from_secs(3));
    println!("running modules: {:?}", running.lock().unwrap());
    let st = hub.snapshot();
    println!(
        "agents providers: {:?}",
        st.agents.as_ref().map(|a| a
            .providers
            .iter()
            .map(|p| (p.provider.label(), p.detected, p.sessions.len()))
            .collect::<Vec<_>>())
    );
    println!(
        "timers: {:?}",
        st.timers
            .as_ref()
            .map(|t| t.timers.iter().map(|x| x.run).collect::<Vec<_>>())
    );
    println!(
        "companion: {:?}",
        st.companion.as_ref().map(|c| (c.mood, c.cause))
    );
    println!("watch slot: {:?}", st.watch.as_ref().map(|w| w.phase));
    let alerts = hub.drain_alerts();
    println!(
        "alerts: {:?}",
        alerts.iter().map(|a| a.kind).collect::<Vec<_>>()
    );
    let during = threads();
    mgr.shutdown();
    std::thread::sleep(Duration::from_millis(200));
    let after = threads();
    let st = hub.snapshot();
    println!("threads: before {before}, running {during}, after shutdown {after}");
    println!(
        "slots cleared: agents {} timers {} companion {}",
        st.agents.is_none(),
        st.timers.is_none(),
        st.companion.is_none()
    );
    let _ = std::fs::remove_dir_all(dir);
}
