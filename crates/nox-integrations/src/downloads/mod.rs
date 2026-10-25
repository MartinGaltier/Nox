// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchDownloadService.swift, commit e916961.

//! Downloads (E3): FSEvents on the watched folders, partial files
//! (`.crdownload`, Safari `.download`, `.part`…), totals from NSProgress
//! publishers or Safari's Info.plist when known, byte rate, finish alert.

pub mod track;

use crate::sys::{fsevents, progress, workspace};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::downloads::{
    DownloadCommand, DownloadSettings, DownloadStatus, DownloadsState,
};
use nox_core::permissions::PermissionState;
use nox_core::{
    Alert, AlertKind, CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Settings,
    Worker,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use track::{Event, Tracker};

/// Re-scan cadence while something is downloading (sizes grow without
/// always producing FSEvents in time).
const LIVE_TICK: Duration = Duration::from_secs(1);
const IDLE_TICK: Duration = Duration::from_secs(30);

enum Msg {
    Command(DownloadCommand),
    Settings(Arc<Settings>),
}

#[derive(Default)]
pub struct DownloadsModule {
    worker: Option<Worker>,
    tx: Option<Sender<Msg>>,
}

impl Module for DownloadsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Downloads
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        let settings = ctx.settings.downloads.clone();
        hub.update(|s| s.downloads = Some(DownloadsState::default()));
        self.worker = Some(Worker::spawn("downloads", move |token| {
            run(hub, settings, rx, token)
        }));
        self.tx = Some(tx);
        Ok(())
    }

    fn stop(&mut self) {
        self.tx.take();
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn command(&mut self, cmd: nox_core::Command) {
        if let (nox_core::Command::Downloads(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.clone()));
        }
    }
}

pub fn folders(s: &DownloadSettings) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = s
        .folders
        .iter()
        .filter(|f| !f.trim().is_empty())
        .map(|f| crate::util::expand_tilde(f))
        .collect();
    if v.is_empty() {
        v.push(crate::util::home().join("Downloads"));
    }
    v.dedup();
    v
}

fn final_exists(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir() || m.len() > 0)
}

struct Engine {
    hub: Hub,
    settings: DownloadSettings,
    folders: Vec<PathBuf>,
    tracker: Tracker,
    access: PermissionState,
    subs: Option<progress::Subscriptions>,
    stream: Option<fsevents::Stream>,
    fs_tx: Sender<()>,
    finished_tx: Sender<String>,
    subs_tx: Sender<progress::Subscriptions>,
}

impl Engine {
    fn watch(&mut self) {
        self.stream = None;
        if let Some(mut s) = self.subs.take() {
            s.remove();
        }
        let paths: Vec<String> = self
            .folders
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        self.stream = fsevents::Stream::new(&paths, 0.3, self.fs_tx.clone());
        // Registered on the main thread (its run loop delivers them); the
        // result comes back through `subs_tx` without blocking this worker.
        let tx = self.finished_tx.clone();
        let back = self.subs_tx.clone();
        nox_core::main_thread::run_async(move || {
            let subs = progress::subscribe(&paths, tx);
            // If the worker is gone, dropping `subs` here removes them.
            let _ = back.send(subs);
        });
        if self.stream.is_none() {
            log::warn!("downloads: FSEvents stream not started");
        }
    }

    fn scan(&mut self) {
        let now = nox_core::model::now();
        let published = self.subs.as_ref().map(|s| s.snapshot()).unwrap_or_default();
        let mut partials = vec![];
        let mut readable = false;
        for f in &self.folders {
            match std::fs::read_dir(f) {
                Ok(_) => readable = true,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => continue,
                Err(_) => continue,
            }
            for mut p in track::scan(f) {
                if p.source == "Safari"
                    && let Some((bytes, total)) =
                        progress::safari_progress(&p.partial_path.to_string_lossy())
                {
                    p.bytes = p.bytes.max(bytes);
                    p.total = total;
                }
                let pp = p.partial_path.to_string_lossy();
                let fp = p.final_path.to_string_lossy();
                if let Some(pub_) = published
                    .iter()
                    .find(|x| x.path == pp || x.path == fp)
                    .filter(|x| !x.indeterminate && x.total > 0)
                {
                    p.total = Some(pub_.total as u64);
                    p.bytes = p.bytes.max(pub_.completed.max(0) as u64);
                }
                partials.push(p);
            }
        }
        self.access = if readable {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        };
        let events = self.tracker.update(partials, now, &final_exists);
        self.tracker.expire(now, self.settings.keep_finished_secs);
        for e in events {
            self.alert(e);
        }
        self.publish();
    }

    fn alert(&self, e: Event) {
        let Event::Finished(item) = e;
        let mut a = Alert::new(
            AlertKind::DownloadFinished,
            item.file_name.clone(),
            "arrow.down.circle.fill",
        )
        .subtitle(format!("Téléchargé · {}", item.source))
        .ttl(5.0);
        a.progress = Some(1.0);
        self.hub.push_alert(a);
    }

    fn publish(&self) {
        let s = DownloadsState {
            access: self.access,
            items: self.tracker.items(),
        };
        self.hub.update(move |st| {
            if st.downloads.is_some() {
                st.downloads = Some(s);
            }
        });
    }

    fn on_finished_announcement(&mut self, path: String) {
        let p = Path::new(&path);
        // Only files that land directly in a watched folder.
        let inside = p
            .parent()
            .is_some_and(|parent| self.folders.iter().any(|f| f == parent));
        if !inside {
            return;
        }
        if let Some(e) = self.tracker.announce_finished(p, nox_core::model::now()) {
            self.alert(e);
            self.publish();
        }
    }

    fn on_command(&mut self, c: DownloadCommand) {
        let item = |id: &str| self.tracker.items().into_iter().find(|i| i.id == id);
        match c {
            DownloadCommand::Reveal(id) => {
                if let Some(i) = item(&id) {
                    let target = if i.status == DownloadStatus::InProgress {
                        // Reveal the folder while the final file does not exist yet.
                        Path::new(&i.path)
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                    } else {
                        Some(i.path.clone())
                    };
                    if let Some(t) = target {
                        workspace::reveal(&[t]);
                    }
                }
            }
            DownloadCommand::Open(id) => {
                if let Some(i) = item(&id).filter(|i| i.status == DownloadStatus::Finished) {
                    workspace::open_path(&i.path);
                }
            }
            DownloadCommand::Dismiss(id) => {
                self.tracker.dismiss(&id);
                self.publish();
            }
        }
    }
}

fn run(hub: Hub, settings: DownloadSettings, rx: Receiver<Msg>, token: CancelToken) {
    let (fs_tx, fs_rx) = crossbeam_channel::bounded(1);
    let (finished_tx, finished_rx) = unbounded();
    let (subs_tx, subs_rx) = unbounded::<progress::Subscriptions>();
    let mut e = Engine {
        hub,
        folders: folders(&settings),
        settings,
        tracker: Tracker::default(),
        access: PermissionState::NotDetermined,
        subs: None,
        stream: None,
        fs_tx,
        finished_tx,
        subs_tx,
    };
    e.watch();
    e.scan();
    let mut last_scan = Instant::now();
    while !token.is_cancelled() {
        let tick = if e.tracker.has_live() {
            LIVE_TICK
        } else {
            IDLE_TICK
        };
        // Sleep until the next rescan is due (or an event arrives), not 250 ms.
        let due = tick.saturating_sub(last_scan.elapsed());
        select! {
            recv(fs_rx) -> _ => {
                e.scan();
                last_scan = Instant::now();
            },
            recv(subs_rx) -> sub => if let Ok(sub) = sub {
                // A newer `watch()` may have replaced the folder set: keep the last.
                if let Some(mut old) = e.subs.replace(sub) {
                    old.remove();
                }
            },
            recv(finished_rx) -> p => if let Ok(p) = p { e.on_finished_announcement(p) },
            recv(rx) -> m => match m {
                Ok(Msg::Command(c)) => e.on_command(c),
                Ok(Msg::Settings(s)) => {
                    let new = folders(&s.downloads);
                    e.settings = s.downloads.clone();
                    if new != e.folders {
                        e.folders = new;
                        e.watch();
                    }
                    e.scan();
                }
                Err(_) => break,
            },
            default(due) => {},
        }
        if last_scan.elapsed() >= tick {
            e.scan();
            last_scan = Instant::now();
        }
    }
    e.stream = None;
    if let Some(mut s) = e.subs.take() {
        s.remove();
    }
    e.hub.update(|s| s.downloads = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_folder_is_downloads() {
        let f = folders(&DownloadSettings::default());
        assert!(f[0].ends_with("Downloads"));
        let f = folders(&DownloadSettings {
            folders: vec!["~/A".into(), "~/A".into(), " ".into()],
            ..Default::default()
        });
        assert_eq!(f.len(), 1);
    }

    #[test]
    #[ignore = "real machine: watches a temp folder with FSEvents and simulates a Chrome download"]
    fn live_fsevents_simulated_download() {
        // FSEvents reports real paths: resolve /var → /private/var first.
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let dir = tmp.join(format!("nox-dl-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hub = Hub::new();
        let mut m = DownloadsModule::default();
        let mut s = Settings::default();
        s.downloads.folders = vec![dir.to_string_lossy().into_owned()];
        m.start(ModuleContext {
            hub: hub.clone(),
            settings: Arc::new(s),
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let part = dir.join("synthetic.bin.crdownload");
        std::fs::write(&part, vec![0u8; 1000]).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let seen = hub.read(|s| s.downloads.clone()).unwrap();
        eprintln!("in progress: {:?}", seen.items);
        assert_eq!(seen.items.len(), 1);
        assert_eq!(seen.items[0].status, DownloadStatus::InProgress);
        std::fs::write(&part, vec![0u8; 5000]).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        std::fs::rename(&part, dir.join("synthetic.bin")).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let done = hub.read(|s| s.downloads.clone()).unwrap();
        eprintln!("after rename: {:?}", done.items);
        assert_eq!(done.items[0].status, DownloadStatus::Finished);
        assert_eq!(done.items[0].bytes, 5000);
        let alerts = hub.drain_alerts();
        assert!(alerts.iter().any(|a| a.kind == AlertKind::DownloadFinished));
        m.stop();
        assert!(hub.read(|s| s.downloads.is_none()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
