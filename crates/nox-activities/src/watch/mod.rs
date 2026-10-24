//! Region watch (D10). The UI picks a region (its own overlay) and sends
//! `WatchCommand::Start`; a worker captures it at low frequency, compares
//! grey thumbnails and alerts on a change or once activity settles. The
//! module is off by default; Screen Recording is requested only when the user
//! starts a selection, and capture stops as soon as the watch ends.

pub mod capture;
pub mod tracker;

use crate::sound::{self, Tone};
use nox_core::model::watch::{
    Rect, WatchCommand, WatchOutcome, WatchPhase, WatchSettings, WatchState, WatchTrigger,
};
use nox_core::permissions::PermissionState;
use nox_core::{
    Alert, AlertKind, AlertPriority, Command, Hub, Module, ModuleContext, ModuleError, ModuleId,
    Settings, Worker,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracker::Tracker;

/// Consecutive capture failures before the watch is declared lost.
const MAX_FAILURES: u32 = 3;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
/// Delay before the reference capture of a new watch.
const BASELINE_DELAY: Duration = Duration::from_millis(300);

pub type Capturer = Arc<dyn Fn(Rect) -> Result<Vec<u8>, String> + Send + Sync>;
pub type AccessCheck = Arc<dyn Fn(bool) -> bool + Send + Sync>;

const DENIED: &str = "Autorisez « Enregistrement de l’écran » pour Nox dans Réglages Système › Confidentialité et sécurité.";

struct Shared {
    state: WatchState,
    settings: WatchSettings,
    hub: Hub,
}

impl Shared {
    fn publish(&self) {
        let s = self.state.clone();
        self.hub.update(|st| st.watch = Some(s));
    }
}

pub struct WatchModule {
    shared: Option<Arc<Mutex<Shared>>>,
    worker: Option<Worker>,
    capturer: Capturer,
    /// `access(request)`: checks (or requests when `true`) Screen Recording.
    access: AccessCheck,
}

fn lock(m: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn valid(r: &Rect) -> bool {
    [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w >= 4.0 && r.h >= 4.0
}

impl WatchModule {
    pub fn new() -> Self {
        Self {
            shared: None,
            worker: None,
            capturer: Arc::new(|r| capture::capture(r, CAPTURE_TIMEOUT)),
            access: Arc::new(|request| {
                if request {
                    capture::request_access()
                } else {
                    capture::has_access()
                }
            }),
        }
    }

    #[cfg(test)]
    fn with(capturer: Capturer, access: AccessCheck) -> Self {
        Self {
            shared: None,
            worker: None,
            capturer,
            access,
        }
    }

    fn stop_worker(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn begin(&mut self, region: Rect, trigger: WatchTrigger) {
        self.stop_worker();
        let Some(shared) = self.shared.clone() else {
            return;
        };
        {
            let mut s = lock(&shared);
            if !valid(&region) {
                s.state.error = Some("Zone trop petite ou invalide.".into());
                s.publish();
                return;
            }
            if !(self.access)(false) {
                s.state.access = PermissionState::Denied;
                s.state.phase = WatchPhase::Idle;
                s.state.error = Some(DENIED.into());
                s.publish();
                return;
            }
            let now = nox_core::model::now();
            s.state = WatchState {
                access: PermissionState::Granted,
                phase: WatchPhase::Watching,
                region: Some(region),
                target: None,
                trigger,
                last_change_at: None,
                last_diff: None,
                started_at: Some(now),
                outcome: None,
                error: None,
            };
            s.publish();
        }
        let capturer = self.capturer.clone();
        self.worker = Some(Worker::spawn("watch", move |token| {
            let (mut tracker, interval) = {
                let s = lock(&shared);
                let w = &s.settings;
                (
                    Tracker::new(
                        trigger,
                        w.sensitivity,
                        w.settle_secs as f64,
                        w.change_confirm_secs as f64,
                    ),
                    Duration::from_millis(w.interval_ms.clamp(250, 10_000)),
                )
            };
            let mut failures = 0u32;
            // The picker overlay may still be on screen for a frame after it
            // closed: the reference capture waits so its veil is not taken
            // for the region's content.
            if token.sleep(BASELINE_DELAY) {
                return;
            }
            loop {
                let shot = capturer(region);
                if token.is_cancelled() {
                    break;
                }
                let now = nox_core::model::now();
                let outcome = {
                    let mut s = lock(&shared);
                    match shot {
                        Ok(thumb) => {
                            failures = 0;
                            let o = tracker.observe(thumb, now);
                            let diff = (o.diff * 1000.0).round() / 1000.0;
                            let phase = if o.changing {
                                WatchPhase::Changing
                            } else {
                                WatchPhase::Watching
                            };
                            let changed = s.state.last_diff != Some(diff)
                                || s.state.phase != phase
                                || o.outcome.is_some();
                            s.state.last_diff = Some(diff);
                            s.state.phase = phase;
                            s.state.last_change_at = tracker.last_change;
                            s.state.error = None;
                            if let Some(out) = o.outcome {
                                s.state.phase = WatchPhase::Triggered;
                                s.state.outcome = Some(out);
                            }
                            if changed {
                                s.publish();
                            }
                            o.outcome
                        }
                        Err(e) => {
                            failures += 1;
                            log::warn!("watch: capture failed ({failures}): {e}");
                            s.state.error = Some(e);
                            if failures >= MAX_FAILURES {
                                s.state.phase = WatchPhase::Triggered;
                                s.state.outcome = Some(WatchOutcome::Lost);
                                s.state.access = if capture_access_lost(&s) {
                                    PermissionState::Denied
                                } else {
                                    s.state.access
                                };
                                s.publish();
                                Some(WatchOutcome::Lost)
                            } else {
                                None
                            }
                        }
                    }
                };
                if let Some(out) = outcome {
                    let s = lock(&shared);
                    s.hub.push_alert(alert_for(out));
                    if s.settings.sound && out != WatchOutcome::Lost {
                        sound::play(Tone::Watch);
                    }
                    // The watch is over: capture stops here.
                    break;
                }
                if token.sleep(interval) {
                    break;
                }
            }
        }));
    }
}

fn capture_access_lost(s: &Shared) -> bool {
    s.state
        .error
        .as_deref()
        .is_some_and(|e| e.contains("autoris") || e.contains("permission") || e.contains("TCC"))
}

pub fn alert_for(out: WatchOutcome) -> Alert {
    match out {
        WatchOutcome::Changed => Alert::new(
            AlertKind::WatchTriggered,
            "La zone surveillée a changé",
            "eye",
        )
        .priority(AlertPriority::High)
        .ttl(8.0),
        WatchOutcome::Settled => Alert::new(
            AlertKind::WatchTriggered,
            "Activité terminée dans la zone surveillée",
            "checkmark.circle",
        )
        .subtitle("Plus aucun changement")
        .priority(AlertPriority::High)
        .ttl(8.0),
        WatchOutcome::Lost => Alert::new(
            AlertKind::WatchTriggered,
            "Surveillance interrompue",
            "eye.slash",
        )
        .subtitle("La capture de la zone a échoué")
        .ttl(6.0),
    }
}

impl Default for WatchModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for WatchModule {
    fn id(&self) -> ModuleId {
        ModuleId::Watch
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let state = WatchState {
            // Preflight only: no prompt until the user starts a selection.
            access: if (self.access)(false) {
                PermissionState::Granted
            } else {
                PermissionState::NotDetermined
            },
            ..Default::default()
        };
        let shared = Shared {
            state,
            settings: ctx.settings.watch.clone(),
            hub: ctx.hub,
        };
        shared.publish();
        self.shared = Some(Arc::new(Mutex::new(shared)));
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_worker();
        if let Some(sh) = self.shared.take() {
            lock(&sh).hub.update(|s| s.watch = None);
        }
    }

    fn command(&mut self, cmd: Command) {
        let Command::Watch(cmd) = cmd else { return };
        let Some(shared) = self.shared.clone() else {
            return;
        };
        match cmd {
            WatchCommand::BeginSelection | WatchCommand::RequestAccess => {
                let selecting = matches!(cmd, WatchCommand::BeginSelection);
                let granted = (self.access)(false) || (self.access)(true);
                let mut s = lock(&shared);
                s.state.access = if granted {
                    PermissionState::Granted
                } else {
                    PermissionState::Denied
                };
                if granted {
                    s.state.error = None;
                    if selecting {
                        drop(s);
                        self.stop_worker();
                        let mut s = lock(&shared);
                        s.state.phase = WatchPhase::Selecting;
                        s.state.outcome = None;
                        s.publish();
                        return;
                    }
                } else {
                    s.state.error = Some(DENIED.into());
                }
                s.publish();
            }
            WatchCommand::CancelSelection => {
                let mut s = lock(&shared);
                if s.state.phase == WatchPhase::Selecting {
                    s.state.phase = WatchPhase::Idle;
                    s.publish();
                }
            }
            WatchCommand::Start { region, trigger } => self.begin(region, trigger),
            WatchCommand::Again => {
                let (region, trigger) = {
                    let s = lock(&shared);
                    (s.state.region, s.state.trigger)
                };
                if let Some(r) = region {
                    self.begin(r, trigger);
                }
            }
            WatchCommand::SetTrigger(t) => {
                let region = {
                    let mut s = lock(&shared);
                    s.state.trigger = t;
                    s.publish();
                    matches!(s.state.phase, WatchPhase::Watching | WatchPhase::Changing)
                        .then_some(s.state.region)
                        .flatten()
                };
                // A new rule starts from what the region shows now.
                if let Some(r) = region {
                    self.begin(r, t);
                }
            }
            WatchCommand::Stop => {
                self.stop_worker();
                let mut s = lock(&shared);
                let access = s.state.access;
                let trigger = s.state.trigger;
                s.state = WatchState {
                    access,
                    trigger,
                    ..Default::default()
                };
                s.publish();
            }
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(sh) = &self.shared {
            lock(sh).settings = settings.watch.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ctx(hub: &Hub, interval_ms: u64) -> ModuleContext {
        let mut s = Settings::default();
        s.watch.interval_ms = interval_ms;
        s.watch.change_confirm_secs = 0;
        s.watch.sound = false;
        ModuleContext {
            hub: hub.clone(),
            settings: Arc::new(s),
        }
    }

    fn wait(hub: &Hub, f: impl Fn(&WatchState) -> bool) -> bool {
        for _ in 0..300 {
            if hub.read(|s| s.watch.as_ref().is_some_and(&f)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    const REGION: Rect = Rect {
        x: 10.0,
        y: 10.0,
        w: 200.0,
        h: 100.0,
    };

    #[test]
    fn change_triggers_alert_and_stops_capturing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        // Third capture onwards differs from the first.
        let capturer: Capturer = Arc::new(move |_| {
            let n = c2.fetch_add(1, Ordering::SeqCst);
            Ok(vec![
                if n < 2 { 10 } else { 200 };
                tracker::SIDE * tracker::SIDE
            ])
        });
        let hub = Hub::new();
        let mut m = WatchModule::with(capturer, Arc::new(|_| true));
        m.start(ctx(&hub, 250)).unwrap();
        m.command(Command::Watch(WatchCommand::BeginSelection));
        assert!(wait(&hub, |w| w.phase == WatchPhase::Selecting));
        m.command(Command::Watch(WatchCommand::Start {
            region: REGION,
            trigger: WatchTrigger::AnyChange,
        }));
        assert!(wait(&hub, |w| w.outcome == Some(WatchOutcome::Changed)));
        let alerts = hub.drain_alerts();
        assert!(alerts.iter().any(|a| a.kind == AlertKind::WatchTriggered));
        let n = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(calls.load(Ordering::SeqCst), n, "no capture after trigger");
        m.stop();
        assert!(hub.read(|s| s.watch.is_none()));
    }

    #[test]
    fn reference_capture_waits_for_the_overlay_to_go() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let capturer: Capturer = Arc::new(move |_| {
            c2.fetch_add(1, Ordering::SeqCst);
            Ok(vec![1; tracker::SIDE * tracker::SIDE])
        });
        let hub = Hub::new();
        let mut m = WatchModule::with(capturer, Arc::new(|_| true));
        m.start(ctx(&hub, 250)).unwrap();
        m.command(Command::Watch(WatchCommand::Start {
            region: REGION,
            trigger: WatchTrigger::AnyChange,
        }));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no capture right after Start"
        );
        std::thread::sleep(Duration::from_millis(400));
        assert!(calls.load(Ordering::SeqCst) >= 1);
        // Stopping during the delay ends the worker at once.
        m.command(Command::Watch(WatchCommand::Again));
        let t = std::time::Instant::now();
        m.stop();
        assert!(t.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn denied_access_never_captures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let capturer: Capturer = Arc::new(move |_| {
            c2.fetch_add(1, Ordering::SeqCst);
            Ok(vec![0; 4])
        });
        let hub = Hub::new();
        let mut m = WatchModule::with(capturer, Arc::new(|_| false));
        m.start(ctx(&hub, 250)).unwrap();
        m.command(Command::Watch(WatchCommand::BeginSelection));
        assert!(wait(&hub, |w| w.access == PermissionState::Denied
            && w.error.is_some()));
        m.command(Command::Watch(WatchCommand::Start {
            region: REGION,
            trigger: WatchTrigger::Settled,
        }));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        m.stop();
    }

    #[test]
    fn failures_end_the_watch_and_stop_is_immediate() {
        let capturer: Capturer = Arc::new(|_| Err("capture vide".into()));
        let hub = Hub::new();
        let mut m = WatchModule::with(capturer, Arc::new(|_| true));
        m.start(ctx(&hub, 250)).unwrap();
        m.command(Command::Watch(WatchCommand::Start {
            region: REGION,
            trigger: WatchTrigger::Settled,
        }));
        assert!(wait(&hub, |w| w.outcome == Some(WatchOutcome::Lost)));
        // A fresh watch, stopped by the user.
        let ok: Capturer = Arc::new(|_| Ok(vec![5; tracker::SIDE * tracker::SIDE]));
        m.capturer = ok;
        m.command(Command::Watch(WatchCommand::Again));
        assert!(wait(&hub, |w| w.phase == WatchPhase::Watching));
        let t = std::time::Instant::now();
        m.command(Command::Watch(WatchCommand::Stop));
        assert!(t.elapsed() < Duration::from_secs(1));
        assert!(wait(&hub, |w| w.phase == WatchPhase::Idle && w.region.is_none()));
        m.stop();
    }
}
