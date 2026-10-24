//! Recent screenshots / screen recordings (E11). The folder comes from
//! `com.apple.screencapture location` (read in-process, default ~/Desktop)
//! and is watched with FSEvents. A file counts as a capture only if macOS
//! tagged it (`kMDItemIsScreenCapture` extended attribute) — never guessed
//! from localized file names. References only; off by default (the Desktop
//! is TCC-protected, the prompt appears when the module first reads it).

use crate::sys::{fsevents, prefs, workspace};
use crate::util::{expand_tilde, fnv1a, home};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::captures::{CaptureCommand, CaptureItem, CaptureSettings, CapturesState};
use nox_core::permissions::PermissionState;
use nox_core::{CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Settings, Worker};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

pub const CAPTURE_XATTR: &str = "com.apple.metadata:kMDItemIsScreenCapture";
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "heic", "tiff", "gif", "pdf"];
const VIDEO_EXT: &[&str] = &["mov", "mp4"];
const DEBOUNCE: Duration = Duration::from_millis(300);

enum Msg {
    Command(CaptureCommand),
    Settings(Arc<Settings>),
}

#[derive(Default)]
pub struct CapturesModule {
    worker: Option<Worker>,
    tx: Option<Sender<Msg>>,
}

impl Module for CapturesModule {
    fn id(&self) -> ModuleId {
        ModuleId::Captures
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        let settings = ctx.settings.captures.clone();
        hub.update(|s| s.captures = Some(CapturesState::default()));
        self.worker = Some(Worker::spawn("captures", move |token| {
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
        if let (nox_core::Command::Captures(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.clone()));
        }
    }
}

/// Where macOS saves captures.
pub fn capture_folder() -> PathBuf {
    prefs::app_string("com.apple.screencapture", "location")
        .map(|l| expand_tilde(l.trim()))
        .filter(|p| p.is_absolute() && p.is_dir())
        .unwrap_or_else(|| home().join("Desktop"))
}

fn kind(path: &Path) -> Option<bool> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXT.contains(&ext.as_str()) {
        Some(false)
    } else if VIDEO_EXT.contains(&ext.as_str()) {
        Some(true)
    } else {
        None
    }
}

/// Tagged captures directly inside `folder`, newest first, at most `max`.
/// `Err` when the folder cannot be read (permission).
pub fn scan(folder: &Path, max: usize) -> std::io::Result<Vec<CaptureItem>> {
    let mut items: Vec<CaptureItem> = std::fs::read_dir(folder)?
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let is_video = kind(&path)?;
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() || !prefs::has_xattr(&path, CAPTURE_XATTR) {
                return None;
            }
            let created = meta
                .created()
                .or_else(|_| meta.modified())
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_secs_f64();
            let p = path.to_string_lossy().into_owned();
            Some(CaptureItem {
                id: fnv1a(p.as_bytes()),
                name: path.file_name()?.to_string_lossy().into_owned(),
                path: p,
                created_at: created,
                size: Some(meta.len()),
                is_video,
            })
        })
        .collect();
    items.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
    items.truncate(max.clamp(1, 100));
    Ok(items)
}

struct Engine {
    hub: Hub,
    settings: CaptureSettings,
    state: CapturesState,
    folder: PathBuf,
    stream: Option<fsevents::Stream>,
    fs_tx: Sender<()>,
}

impl Engine {
    fn watch(&mut self) {
        self.stream = None;
        self.folder = capture_folder();
        self.state.folder = self.folder.to_string_lossy().into_owned();
        self.stream = fsevents::Stream::new(
            std::slice::from_ref(&self.state.folder),
            0.3,
            self.fs_tx.clone(),
        );
    }

    fn rescan(&mut self) {
        match scan(&self.folder, self.settings.max_items) {
            Ok(items) => {
                self.state.access = PermissionState::Granted;
                self.state.items = items;
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                self.state.access = PermissionState::Denied;
                self.state.items.clear();
            }
            Err(_) => self.state.items.clear(),
        }
        let s = self.state.clone();
        self.hub.update(move |st| {
            if st.captures.is_some() {
                st.captures = Some(s);
            }
        });
    }

    fn path(&self, id: u64) -> Option<String> {
        self.state
            .items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.path.clone())
    }

    fn on_command(&mut self, c: CaptureCommand) {
        match c {
            CaptureCommand::Open(id) => {
                if let Some(p) = self.path(id) {
                    workspace::open_path(&p);
                }
            }
            CaptureCommand::Reveal(id) => {
                if let Some(p) = self.path(id) {
                    workspace::reveal(&[p]);
                }
            }
            CaptureCommand::CopyImage(id) => {
                if let Some(p) = self.path(id).filter(|p| kind(Path::new(p)) == Some(false)) {
                    workspace::copy_image(&p);
                }
            }
            CaptureCommand::MoveToTrash(id) => {
                if let Some(p) = self.path(id)
                    && let Err(e) = workspace::move_to_trash(&p)
                {
                    log::info!("captures: trash failed: {e}");
                }
                self.rescan();
            }
            CaptureCommand::Refresh => {
                // The user may have changed the capture folder.
                if capture_folder() != self.folder {
                    self.watch();
                }
                self.rescan();
            }
        }
    }
}

fn run(hub: Hub, settings: CaptureSettings, rx: Receiver<Msg>, token: CancelToken) {
    let (fs_tx, fs_rx) = crossbeam_channel::bounded(1);
    let mut e = Engine {
        hub,
        settings,
        state: CapturesState::default(),
        folder: PathBuf::new(),
        stream: None,
        fs_tx,
    };
    e.watch();
    e.rescan();
    while !token.is_cancelled() {
        select! {
            recv(fs_rx) -> _ => {
                // Screenshots are written then tagged: let the burst settle.
                std::thread::sleep(DEBOUNCE);
                while fs_rx.try_recv().is_ok() {}
                e.rescan();
            },
            recv(rx) -> m => match m {
                Ok(Msg::Command(c)) => e.on_command(c),
                Ok(Msg::Settings(s)) => {
                    e.settings = s.captures.clone();
                    e.rescan();
                }
                Err(_) => break,
            },
            default(Duration::from_millis(500)) => {},
        }
    }
    e.stream = None;
    e.hub.update(|s| s.captures = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    fn tag(path: &Path) {
        let p = CString::new(path.as_os_str().as_bytes()).unwrap();
        let n = CString::new(CAPTURE_XATTR).unwrap();
        // A binary plist "true" is what macOS writes; any value marks it.
        let v = b"bplist00\x09\x08\x00\x00\x00\x00\x00\x00\x01\x01\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x09";
        // SAFETY: valid NUL-terminated strings and buffer.
        let r = unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0, 0) };
        assert_eq!(r, 0);
    }

    #[test]
    fn only_tagged_captures_newest_first() {
        let d = std::env::temp_dir().join(format!("nox-captures-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for (n, tagged) in [
            ("a.png", true),
            ("b.png", false),
            ("c.mov", true),
            ("d.txt", true),
        ] {
            std::fs::write(d.join(n), b"x").unwrap();
            if tagged {
                tag(&d.join(n));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let items = scan(&d, 12).unwrap();
        let names: Vec<_> = items
            .iter()
            .map(|i| (i.name.as_str(), i.is_video))
            .collect();
        assert_eq!(
            names,
            vec![("c.mov", true), ("a.png", false)],
            "untagged and non-media files ignored"
        );
        assert_eq!(scan(&d, 1).unwrap().len(), 1);
        assert!(scan(&d.join("missing"), 12).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    #[ignore = "real machine: reads the capture folder preference only (no folder access)"]
    fn live_capture_folder() {
        eprintln!(
            "capture folder is absolute: {}",
            capture_folder().is_absolute()
        );
    }
}
