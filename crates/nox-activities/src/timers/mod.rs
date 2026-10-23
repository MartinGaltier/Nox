//! Timers module (D1–D3): countdowns, stopwatch, Pomodoro.
//!
//! One worker sleeps until the next deadline. While something counts it
//! publishes at 1 Hz; at rest it sleeps until a command nudges it (no wake-ups).

pub mod engine;

use crate::clock::{SharedClock, SystemClock};
use crate::sound::{self, Tone};
use crate::wake::{Wake, stop_worker};
use engine::{Persisted, TimerEngine, TimerEvent};
use nox_core::model::timers::FocusPhase;
use nox_core::{
    Alert, AlertKind, AlertPriority, Command, Hub, Module, ModuleContext, ModuleError, ModuleId,
    Settings, Worker,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Sleep used when nothing runs: effectively "until nudged".
const IDLE_SLEEP: Duration = Duration::from_secs(24 * 3600);

struct Shared {
    engine: TimerEngine,
    clock: SharedClock,
    hub: Hub,
    path: Option<PathBuf>,
}

impl Shared {
    fn publish(&self) {
        let snap = self.engine.snapshot(&*self.clock);
        self.hub.update(|s| s.timers = Some(snap));
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let res = if self.engine.is_empty() {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            serde_json::to_vec(&self.engine.persist(&*self.clock))
                .map_err(std::io::Error::other)
                .and_then(|b| nox_core::paths::write_atomic(path, &b))
        };
        if let Err(e) = res {
            log::warn!("timers: could not save state: {e}");
        }
    }

    fn announce(&self, events: Vec<TimerEvent>) {
        for ev in events {
            let (alert, tone) = alert_for(&ev);
            self.hub.push_alert(alert);
            if self.engine.sound() {
                sound::play(tone);
            }
        }
    }
}

fn alert_for(ev: &TimerEvent) -> (Alert, Tone) {
    match ev {
        TimerEvent::Done { label, late } => {
            let mut a = Alert::new(AlertKind::TimerDone, "Minuteur terminé", "timer")
                .priority(AlertPriority::High)
                .ttl(8.0);
            a = if *late {
                a.subtitle(format!("{label} (pendant que Nox était fermé)"))
            } else {
                a.subtitle(label.clone())
            };
            (a, Tone::TimerDone)
        }
        TimerEvent::FocusPhase { phase, round } => {
            let (title, symbol) = match phase {
                FocusPhase::Work => (
                    format!("Concentration — séance {round}"),
                    "brain.head.profile",
                ),
                FocusPhase::ShortBreak => ("Pause courte".to_string(), "cup.and.saucer"),
                FocusPhase::LongBreak => ("Pause longue".to_string(), "sofa"),
            };
            (
                Alert::new(AlertKind::FocusPhase, title, symbol).ttl(6.0),
                Tone::Phase,
            )
        }
        TimerEvent::FocusCycleComplete { rounds } => (
            Alert::new(
                AlertKind::FocusPhase,
                "Cycle Pomodoro terminé",
                "checkmark.seal",
            )
            .subtitle(format!("{rounds} séances de concentration"))
            .priority(AlertPriority::High)
            .ttl(8.0),
            Tone::TimerDone,
        ),
    }
}

pub struct TimersModule {
    shared: Option<Arc<Mutex<Shared>>>,
    worker: Option<Worker>,
    wake: Wake,
    clock: SharedClock,
    /// Overrides the state file (tests).
    path_override: Option<PathBuf>,
}

impl TimersModule {
    pub fn new() -> Self {
        Self {
            shared: None,
            worker: None,
            wake: Wake::new(),
            clock: SystemClock::shared(),
            path_override: None,
        }
    }

    #[cfg(test)]
    fn with(clock: SharedClock, path: PathBuf) -> Self {
        Self {
            shared: None,
            worker: None,
            wake: Wake::new(),
            clock,
            path_override: Some(path),
        }
    }

    fn state_path(&self) -> Option<PathBuf> {
        if let Some(p) = &self.path_override {
            return Some(p.clone());
        }
        match nox_core::paths::module_dir("timers") {
            Ok(d) => Some(d.join("state.json")),
            Err(e) => {
                log::warn!("timers: no state folder ({e}); state will not survive a restart");
                None
            }
        }
    }
}

impl Default for TimersModule {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(m: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Module for TimersModule {
    fn id(&self) -> ModuleId {
        ModuleId::Timers
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let path = self.state_path();
        let mut engine = TimerEngine::new(ctx.settings.timers.clone());
        let mut late = Vec::new();
        if let Some(p) = &path
            && let Ok(bytes) = std::fs::read(p)
        {
            match serde_json::from_slice::<Persisted>(&bytes) {
                Ok(saved) => late = engine.restore(saved, &*self.clock),
                Err(e) => log::warn!("timers: ignoring unreadable state ({e})"),
            }
        }
        let shared = Arc::new(Mutex::new(Shared {
            engine,
            clock: self.clock.clone(),
            hub: ctx.hub,
            path,
        }));
        {
            let s = lock(&shared);
            s.publish();
            if !late.is_empty() {
                s.announce(late);
                s.save();
            }
        }
        let s2 = shared.clone();
        let wake = self.wake.clone();
        self.worker = Some(Worker::spawn("timers", move |token| {
            loop {
                let sleep = {
                    let mut s = lock(&s2);
                    let clock = s.clock.clone();
                    let (events, changed) = s.engine.tick(&*clock);
                    if changed {
                        s.save();
                    }
                    if !events.is_empty() {
                        s.announce(events);
                    }
                    let active = s.engine.is_active();
                    if active || changed {
                        s.publish();
                    }
                    let next = s.engine.next_deadline(&*clock);
                    match (active, next) {
                        // 1 Hz while counting, but never past a deadline.
                        (true, Some(n)) => Duration::from_secs_f64(n.min(1.0)),
                        (true, None) => Duration::from_secs(1),
                        (false, Some(n)) => Duration::from_secs_f64(n),
                        (false, None) => IDLE_SLEEP,
                    }
                };
                if wake.sleep(&token, sleep.max(Duration::from_millis(5))) {
                    break;
                }
            }
        }));
        self.shared = Some(shared);
        Ok(())
    }

    fn stop(&mut self) {
        stop_worker(&mut self.worker, &self.wake);
        if let Some(shared) = self.shared.take() {
            let s = lock(&shared);
            s.save();
            s.hub.update(|st| st.timers = None);
        }
    }

    fn command(&mut self, cmd: Command) {
        let Command::Timers(cmd) = cmd else { return };
        let Some(shared) = &self.shared else { return };
        {
            let mut s = lock(shared);
            let clock = s.clock.clone();
            if !s.engine.command(cmd, &*clock) {
                return;
            }
            s.save();
            s.publish();
        }
        self.wake.wake();
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(shared) = &self.shared {
            lock(shared).engine.set_settings(settings.timers.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::test::ManualClock;
    use nox_core::model::timers::{TimerCommand, TimerRun};

    #[test]
    fn module_rings_restores_and_stops() {
        let dir = std::env::temp_dir().join(format!("nox-timers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("state.json");
        let clock = ManualClock::new(5_000.0);
        let hub = Hub::new();
        let mut settings = Settings::default();
        settings.timers.sound = false;
        let ctx = ModuleContext {
            hub: hub.clone(),
            settings: Arc::new(settings),
        };

        let mut m = TimersModule::with(clock.clone(), path.clone());
        m.start(ctx.clone()).unwrap();
        m.command(Command::Timers(TimerCommand::Start {
            label: "Pâtes".into(),
            secs: 0.05,
        }));
        assert_eq!(hub.read(|s| s.timers.as_ref().unwrap().timers.len()), 1);
        clock.advance(0.05);
        // Worker sleeps until the deadline (real 50 ms) then rings.
        let mut alerts = Vec::new();
        for _ in 0..100 {
            alerts.extend(hub.drain_alerts());
            if !alerts.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, AlertKind::TimerDone);
        assert_eq!(alerts[0].subtitle.as_deref(), Some("Pâtes"));

        m.command(Command::Timers(TimerCommand::Start {
            label: "Long".into(),
            secs: 600.0,
        }));
        m.stop();
        assert!(hub.read(|s| s.timers.is_none()), "slot cleared on stop");
        assert!(path.exists(), "state saved");

        clock.advance(100.0);
        let mut m2 = TimersModule::with(clock.clone(), path.clone());
        m2.start(ctx).unwrap();
        let t = hub.read(|s| s.timers.clone().unwrap());
        let long = t.timers.iter().find(|t| t.label == "Long").unwrap();
        assert_eq!(long.run, TimerRun::Running);
        assert!((long.remaining_secs - 500.0).abs() < 1e-6);
        m2.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
}
