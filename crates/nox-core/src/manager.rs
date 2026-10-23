//! Owns modules on a dedicated thread; starts/stops them from settings and
//! routes commands. Disabled modules are stopped (no background collection).

use crate::command::Command;
use crate::hub::Hub;
use crate::module::{Module, ModuleContext, ModuleId};
use crate::settings::Settings;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread::JoinHandle;

enum Msg {
    Command(Command),
    Settings(Arc<Settings>),
    Shutdown,
}

pub struct ModuleManager {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

/// Snapshot of which modules are running (for diagnostics / tests).
pub type RunningSet = Arc<std::sync::Mutex<Vec<ModuleId>>>;

impl ModuleManager {
    pub fn spawn(
        hub: Hub,
        modules: Vec<Box<dyn Module>>,
        settings: Arc<Settings>,
    ) -> (Self, RunningSet) {
        let (tx, rx) = unbounded();
        let cmd_tx = tx.clone();
        hub.set_command_sink(move |c| {
            let _ = cmd_tx.send(Msg::Command(c));
        });
        let running: RunningSet = Default::default();
        let r2 = running.clone();
        let thread = std::thread::Builder::new()
            .name("nox-modules".into())
            .spawn(move || run(hub, modules, settings, rx, r2))
            .expect("spawn module manager");
        (
            Self {
                tx,
                thread: Some(thread),
            },
            running,
        )
    }

    pub fn apply_settings(&self, s: Arc<Settings>) {
        let _ = self.tx.send(Msg::Settings(s));
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(Msg::Command(c));
    }

    /// Stops every module and joins the manager thread.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ModuleManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    hub: Hub,
    modules: Vec<Box<dyn Module>>,
    mut settings: Arc<Settings>,
    rx: Receiver<Msg>,
    running: RunningSet,
) {
    let mut mods: BTreeMap<ModuleId, (Box<dyn Module>, bool)> =
        modules.into_iter().map(|m| (m.id(), (m, false))).collect();

    let reconcile =
        |mods: &mut BTreeMap<ModuleId, (Box<dyn Module>, bool)>, s: &Arc<Settings>, first: bool| {
            for (id, (m, on)) in mods.iter_mut() {
                let want = s.module_enabled(*id);
                if want && !*on {
                    match m.start(ModuleContext {
                        hub: hub.clone(),
                        settings: s.clone(),
                    }) {
                        Ok(()) => {
                            *on = true;
                            log::info!("module {} started", id.key());
                        }
                        Err(e) => log::warn!("module {} failed to start: {e}", id.key()),
                    }
                } else if !want && *on {
                    m.stop();
                    *on = false;
                    log::info!("module {} stopped", id.key());
                } else if *on && !first {
                    m.settings_changed(s);
                }
            }
            *running.lock().unwrap_or_else(|e| e.into_inner()) = mods
                .iter()
                .filter(|(_, (_, on))| *on)
                .map(|(id, _)| *id)
                .collect();
        };

    reconcile(&mut mods, &settings, true);
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Command(c) => match mods.get_mut(&c.target()) {
                Some((m, true)) => m.command(c),
                _ => log::debug!("command for stopped module ignored: {c:?}"),
            },
            Msg::Settings(s) => {
                settings = s;
                reconcile(&mut mods, &settings, false);
            }
            Msg::Shutdown => break,
        }
    }
    for (id, (m, on)) in mods.iter_mut() {
        if *on {
            m.stop();
            *on = false;
            log::info!("module {} stopped (shutdown)", id.key());
        }
    }
    running.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ModuleError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Probe(Arc<AtomicUsize>, Arc<AtomicUsize>);
    impl Module for Probe {
        fn id(&self) -> ModuleId {
            ModuleId::Timers
        }
        fn start(&mut self, _: ModuleContext) -> Result<(), ModuleError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&mut self) {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn disabling_a_module_stops_it() {
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let mut s = Settings::default();
        let (mut mgr, running) = ModuleManager::spawn(
            Hub::new(),
            vec![Box::new(Probe(starts.clone(), stops.clone()))],
            Arc::new(s.clone()),
        );
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        s.set_module_enabled(ModuleId::Timers, false);
        mgr.apply_settings(Arc::new(s));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(running.lock().unwrap().is_empty());
        mgr.shutdown();
        assert_eq!(stops.load(Ordering::SeqCst), 1, "no double stop");
    }
}
