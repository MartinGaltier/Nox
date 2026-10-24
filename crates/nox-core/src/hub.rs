//! The state hub: modules publish snapshots here; the UI reads them on the
//! main thread after being woken (coalesced).

use crate::alert::Alert;
use crate::command::Command;
use crate::model::{
    accessories::AccessoriesState, agents::AgentsState, calendar::CalendarState,
    camera::CameraState, companion::CompanionState, downloads::DownloadsState, files::FilesState,
    metrics::MetricsState, music::MusicState, notifications::NotificationsState, power::PowerState,
    shortcuts::ShortcutsState, timers::TimersState, watch::WatchState,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// Everything the island can display. `None` = module not running.
#[derive(Debug, Clone, Default)]
pub struct IslandState {
    pub music: Option<MusicState>,
    pub calendar: Option<CalendarState>,
    pub notifications: Option<NotificationsState>,
    pub accessories: Option<AccessoriesState>,
    pub downloads: Option<DownloadsState>,
    pub files: Option<FilesState>,
    pub shortcuts: Option<ShortcutsState>,
    pub camera: Option<CameraState>,
    pub agents: Option<AgentsState>,
    pub watch: Option<WatchState>,
    pub timers: Option<TimersState>,
    pub companion: Option<CompanionState>,
    pub metrics: Option<MetricsState>,
    pub power: Option<PowerState>,
    pub captures: Option<crate::model::captures::CapturesState>,
    pub scratchpad: Option<crate::model::scratchpad::ScratchpadState>,
    pub clipboard: Option<crate::model::clipboard::ClipboardState>,
    pub mixer: Option<crate::model::mixer::MixerState>,
}

type Waker = Box<dyn Fn() + Send + Sync>;
type CommandSink = Box<dyn Fn(Command) + Send + Sync>;

struct Inner {
    state: RwLock<IslandState>,
    generation: AtomicU64,
    alerts: Mutex<VecDeque<Alert>>,
    next_alert_id: AtomicU64,
    waker: RwLock<Option<Waker>>,
    wake_pending: AtomicBool,
    commands: RwLock<Option<CommandSink>>,
}

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

const MAX_PENDING_ALERTS: usize = 32;

impl Hub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: RwLock::new(IslandState::default()),
                generation: AtomicU64::new(0),
                alerts: Mutex::new(VecDeque::new()),
                next_alert_id: AtomicU64::new(1),
                waker: RwLock::new(None),
                wake_pending: AtomicBool::new(false),
                commands: RwLock::new(None),
            }),
        }
    }

    /// Mutates the shared state and wakes the UI once (coalesced).
    pub fn update(&self, f: impl FnOnce(&mut IslandState)) {
        {
            let mut s = self.inner.state.write().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
        }
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.wake();
    }

    pub fn read<R>(&self, f: impl FnOnce(&IslandState) -> R) -> R {
        let s = self.inner.state.read().unwrap_or_else(|e| e.into_inner());
        f(&s)
    }

    pub fn snapshot(&self) -> IslandState {
        self.read(|s| s.clone())
    }

    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    pub fn push_alert(&self, mut alert: Alert) -> u64 {
        let id = self.inner.next_alert_id.fetch_add(1, Ordering::SeqCst);
        alert.id = id;
        {
            let mut q = self.inner.alerts.lock().unwrap_or_else(|e| e.into_inner());
            q.push_back(alert);
            while q.len() > MAX_PENDING_ALERTS {
                q.pop_front();
            }
        }
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.wake();
        id
    }

    /// UI side: takes all pending alerts.
    pub fn drain_alerts(&self) -> Vec<Alert> {
        let mut q = self.inner.alerts.lock().unwrap_or_else(|e| e.into_inner());
        q.drain(..).collect()
    }

    /// UI installs the waker; it is called from any thread and must be cheap
    /// (typically: dispatch to main queue). Calls are coalesced until
    /// `acknowledge_wake` runs.
    pub fn set_waker(&self, w: impl Fn() + Send + Sync + 'static) {
        *self.inner.waker.write().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(w));
    }

    pub fn acknowledge_wake(&self) {
        self.inner.wake_pending.store(false, Ordering::SeqCst);
    }

    fn wake(&self) {
        if self.inner.wake_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(w) = self
            .inner
            .waker
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            w();
        } else {
            self.inner.wake_pending.store(false, Ordering::SeqCst);
        }
    }

    /// Installed by the module manager.
    pub fn set_command_sink(&self, f: impl Fn(Command) + Send + Sync + 'static) {
        *self
            .inner
            .commands
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    /// Sends a command to its module (non-blocking).
    pub fn send(&self, cmd: Command) {
        match self
            .inner
            .commands
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            Some(sink) => sink(cmd),
            None => log::warn!("command dropped (no manager): {cmd:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert::AlertKind;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn wake_is_coalesced_until_acknowledged() {
        let hub = Hub::new();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        hub.set_waker(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        hub.update(|s| s.music = Some(Default::default()));
        hub.update(|s| s.music = None);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        hub.acknowledge_wake();
        hub.push_alert(Alert::new(AlertKind::Info, "x", "bell"));
        assert_eq!(n.load(Ordering::SeqCst), 2);
        assert_eq!(hub.drain_alerts().len(), 1);
        assert!(hub.drain_alerts().is_empty());
    }
}
