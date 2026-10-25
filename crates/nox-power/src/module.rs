//! The `Power` module: one worker thread owns the [`Engine`]; a second,
//! idle CFRunLoop thread forwards IOPowerSources change notifications (no
//! polling, no external process). Rule install/uninstall run on a third,
//! short-lived worker so the admin prompt never blocks the engine.

use crate::assertions::IoPmAssertions;
use crate::engine::Engine;
use crate::lid::{LidClient, LidControl};
use crate::probe;
use crate::rule;
use crate::sys::{PowerSourceWatch, watch_power_sources};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use nox_core::alert::{Alert, AlertKind};
use nox_core::model::now;
use nox_core::model::power::{PowerCommand, PowerSettings, StopReason};
use nox_core::{Command, Hub, Module, ModuleContext, ModuleError, ModuleId, Settings, Worker};
use std::sync::Arc;
use std::time::Duration;

/// Longest sleep while a session runs: wall-clock expiry stays accurate even
/// if the monotonic clock paused (system sleep forced by the user).
const MAX_WAIT: Duration = Duration::from_secs(15);

pub(crate) enum Msg {
    Command(PowerCommand),
    Settings(PowerSettings),
    PowerChanged,
    JobDone {
        install: bool,
        result: Result<(), String>,
    },
    Shutdown,
}

#[derive(Default)]
pub struct PowerModule {
    tx: Option<Sender<Msg>>,
    worker: Option<Worker>,
    watch: Option<PowerSourceWatch>,
    hub: Option<Hub>,
}

impl PowerModule {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Module for PowerModule {
    fn id(&self) -> ModuleId {
        ModuleId::Power
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        if self.worker.is_some() {
            return Ok(());
        }
        let (tx, rx) = unbounded();
        let tx_watch = tx.clone();
        let watch = match watch_power_sources(move || {
            let _ = tx_watch.send(Msg::PowerChanged);
        }) {
            Ok(w) => Some(w),
            Err(e) => {
                // Guards still run on each tick from a fresh read.
                log::warn!("power source notifications unavailable: {e}");
                None
            }
        };
        let hub = ctx.hub.clone();
        let settings = ctx.settings.power.clone();
        let tx_jobs = tx.clone();
        let worker = Worker::spawn("power", move |_token| {
            let engine = Engine::new(IoPmAssertions, LidClient::system(), settings);
            run(engine, hub, rx, tx_jobs);
        });
        self.tx = Some(tx);
        self.worker = Some(worker);
        self.watch = watch;
        self.hub = Some(ctx.hub);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Msg::Shutdown);
        }
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        if let Some(mut w) = self.watch.take() {
            w.stop();
        }
        if let Some(h) = self.hub.take() {
            h.update(|s| s.power = None);
        }
    }

    fn command(&mut self, cmd: Command) {
        if let (Command::Power(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.power.clone()));
        }
    }
}

fn publish<A: crate::assertions::Assertions, L: LidControl>(hub: &Hub, e: &mut Engine<A, L>) {
    if e.take_dirty() {
        let s = e.state().clone();
        hub.update(|st| st.power = Some(s));
    }
    for a in e.take_alerts() {
        hub.push_alert(a);
    }
}

pub(crate) fn run<A: crate::assertions::Assertions, L: LidControl>(
    mut engine: Engine<A, L>,
    hub: Hub,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
) {
    engine.on_power(probe::power_source(), now());
    engine.refresh();
    engine.set_legacy_helper(rule::legacy_helper_present());
    if let Some(n) = crate::recovery::take_notice() {
        hub.push_alert(
            Alert::new(
                AlertKind::KeepAwake,
                "Maintien éveillé",
                "exclamationmark.triangle",
            )
            .subtitle(n.clone())
            .ttl(10.0),
        );
        engine.set_notice(Some(n));
    }
    let mut job: Option<Worker> = None;

    loop {
        if engine.lid_held() {
            engine.on_sensors(probe::power_source(), probe::thermal_pressure(), now());
        }
        let next = engine.tick(now());
        publish(&hub, &mut engine);
        let msg = match next {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(secs) => rx.recv_timeout(Duration::from_secs_f64(secs).min(MAX_WAIT)),
        };
        let msg = match msg {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => Msg::Shutdown,
        };
        match msg {
            Msg::Command(c) => match c {
                PowerCommand::Start { mode, duration } => engine.start(mode, duration, now()),
                PowerCommand::Stop => engine.stop(StopReason::User, now()),
                PowerCommand::Toggle => engine.toggle(now()),
                PowerCommand::Refresh => engine.refresh(),
                PowerCommand::InstallHelper | PowerCommand::UninstallHelper if job.is_some() => {}
                PowerCommand::InstallHelper => match std::env::current_exe() {
                    Err(e) => engine.set_error(Some(e.to_string())),
                    Ok(exe) => {
                        engine.set_rule_busy(true);
                        let tx = tx.clone();
                        let remove_legacy = engine.state().legacy_helper;
                        job = Some(Worker::spawn("power-install", move |token| {
                            let result = rule::install(remove_legacy, &exe, &token);
                            let _ = tx.send(Msg::JobDone {
                                install: true,
                                result,
                            });
                        }));
                    }
                },
                PowerCommand::UninstallHelper => {
                    if engine
                        .state()
                        .active
                        .as_ref()
                        .is_some_and(|s| s.mode.needs_helper())
                    {
                        engine.stop(StopReason::User, now());
                    }
                    engine.set_rule_busy(true);
                    let tx = tx.clone();
                    job = Some(Worker::spawn("power-uninstall", move |token| {
                        let result = rule::uninstall(&token);
                        let _ = tx.send(Msg::JobDone {
                            install: false,
                            result,
                        });
                    }));
                }
            },
            Msg::Settings(s) => engine.on_settings(s, now()),
            Msg::PowerChanged => engine.on_power(probe::power_source(), now()),
            Msg::JobDone { install, result } => {
                if let Some(mut w) = job.take() {
                    w.stop();
                }
                engine.set_rule_busy(false);
                match result {
                    Ok(()) => {
                        engine.set_error(None);
                        hub.push_alert(Alert::new(
                            AlertKind::KeepAwake,
                            if install {
                                "Autorisation capot fermé installée"
                            } else {
                                "Autorisation capot fermé retirée"
                            },
                            "checkmark.shield",
                        ));
                    }
                    Err(e) => engine.set_error(Some(e)),
                }
                engine.refresh();
                engine.set_legacy_helper(rule::legacy_helper_present());
            }
            Msg::Shutdown => {
                if let Some(mut w) = job.take() {
                    w.stop();
                }
                engine.shutdown(now());
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::model::power::{KeepAwakeDuration, KeepAwakeMode};

    fn own_assertions() -> Vec<String> {
        let out = std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "assertions"])
            .output()
            .unwrap();
        let me = format!("pid {}(", std::process::id());
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains(&me))
            .map(str::to_owned)
            .collect()
    }

    fn wait(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Full module on this Mac: start → command → hub state + real assertion
    /// → stop → nothing left (no thread, no assertion, hub slot cleared).
    #[test]
    #[ignore = "creates a real IOPM assertion; run manually with --ignored"]
    fn module_lifecycle_on_this_mac() {
        let hub = Hub::new();
        let mut m = PowerModule::new();
        m.start(ModuleContext {
            hub: hub.clone(),
            settings: Arc::new(Settings::default()),
        })
        .unwrap();
        assert!(wait(|| hub.read(|s| s.power.is_some())));
        eprintln!("initial state: {:?}", hub.read(|s| s.power.clone()));
        m.command(Command::Power(PowerCommand::Start {
            mode: KeepAwakeMode::SystemAndDisplay,
            duration: KeepAwakeDuration::Minutes(15),
        }));
        assert!(wait(
            || hub.read(|s| s.power.as_ref().is_some_and(|p| p.active.is_some()))
        ));
        let lines = own_assertions();
        eprintln!("assertions: {lines:#?}");
        assert!(lines.iter().any(|l| l.contains("Nox: keep Mac awake")));
        assert!(lines.iter().any(|l| l.contains("Nox: keep display awake")));
        assert!(
            hub.drain_alerts()
                .iter()
                .any(|a| a.kind == AlertKind::KeepAwake)
        );
        m.stop();
        assert!(hub.read(|s| s.power.is_none()));
        assert!(own_assertions().is_empty(), "released on stop");
    }
}
