//! Island notepad (E12). Persisted on purpose (user content) in
//! `app_support_dir()/scratchpad/note.txt`: atomic writes, mode 0600,
//! 1 s debounce, 256 KiB maximum, flushed on stop, never logged.

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::scratchpad::{ScratchpadCommand, ScratchpadState};
use nox_core::{CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Worker};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MAX_BYTES: usize = 256 * 1024;
const DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct ScratchpadModule {
    worker: Option<Worker>,
    tx: Option<Sender<ScratchpadCommand>>,
}

pub fn note_path() -> PathBuf {
    nox_core::paths::app_support_dir()
        .join("scratchpad")
        .join("note.txt")
}

impl Module for ScratchpadModule {
    fn id(&self) -> ModuleId {
        ModuleId::Scratchpad
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        hub.update(|s| s.scratchpad = Some(ScratchpadState::default()));
        let path = note_path();
        self.worker = Some(Worker::spawn("scratchpad", move |token| {
            run(hub, path, rx, token)
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
        if let (nox_core::Command::Scratchpad(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(c);
        }
    }
}

/// Cuts `text` to at most `MAX_BYTES` on a character boundary.
pub fn clamp(mut text: String) -> (String, bool) {
    if text.len() <= MAX_BYTES {
        return (text, false);
    }
    let mut cut = MAX_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    (text, true)
}

pub fn load(path: &Path) -> Result<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(clamp(String::from_utf8_lossy(&bytes).into_owned()).0),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("lecture impossible : {e}")),
    }
}

struct Engine {
    hub: Hub,
    path: PathBuf,
    state: ScratchpadState,
    dirty_since: Option<Instant>,
}

impl Engine {
    fn publish(&self) {
        let s = self.state.clone();
        self.hub.update(move |st| {
            if st.scratchpad.is_some() {
                st.scratchpad = Some(s);
            }
        });
    }

    fn save(&mut self) {
        self.dirty_since = None;
        match nox_core::paths::write_atomic(&self.path, self.state.text.as_bytes()) {
            Ok(()) => {
                self.state.saved_at = Some(nox_core::model::now());
                if self
                    .state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("enregistrement"))
                {
                    self.state.error = None;
                }
            }
            Err(e) => self.state.error = Some(format!("enregistrement impossible : {e}")),
        }
        self.publish();
    }

    fn on_command(&mut self, c: ScratchpadCommand) {
        match c {
            ScratchpadCommand::SetText(t) => {
                let (t, cut) = clamp(t);
                self.state.text = t;
                self.state.error = cut.then(|| "texte tronqué (256 Kio maximum)".to_owned());
                self.dirty_since.get_or_insert_with(Instant::now);
                self.publish();
            }
            ScratchpadCommand::Clear => {
                self.state.text.clear();
                self.state.error = None;
                self.save();
            }
        }
    }
}

fn run(hub: Hub, path: PathBuf, rx: Receiver<ScratchpadCommand>, token: CancelToken) {
    let mut e = Engine {
        hub,
        state: ScratchpadState::default(),
        dirty_since: None,
        path,
    };
    match load(&e.path) {
        Ok(t) => e.state.text = t,
        Err(err) => e.state.error = Some(err),
    }
    e.publish();
    while !token.is_cancelled() {
        select! {
            recv(rx) -> c => match c {
                Ok(c) => e.on_command(c),
                Err(_) => break,
            },
            default(Duration::from_millis(250)) => {},
        }
        if e.dirty_since.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            e.save();
        }
    }
    // Never lose the last keystrokes.
    if e.dirty_since.is_some() {
        e.save();
    }
    e.hub.update(|s| s.scratchpad = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn clamps_on_char_boundary() {
        let (t, cut) = clamp("é".repeat(MAX_BYTES));
        assert!(cut && t.len() <= MAX_BYTES && t.chars().all(|c| c == 'é'));
        assert_eq!(clamp("abc".into()), ("abc".into(), false));
    }

    #[test]
    fn debounced_save_flush_on_stop_and_private_mode() {
        let dir = std::env::temp_dir().join(format!("nox-scratch-{}", std::process::id()));
        let path = dir.join("note.txt");
        let hub = Hub::new();
        hub.update(|s| s.scratchpad = Some(Default::default()));
        let (tx, rx) = unbounded();
        let p2 = path.clone();
        let h2 = hub.clone();
        let mut w = Worker::spawn("scratch-test", move |t| run(h2, p2, rx, t));
        tx.send(ScratchpadCommand::SetText("synthetic note".into()))
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!path.exists(), "not written before the debounce");
        assert_eq!(
            hub.read(|s| s.scratchpad.clone()).unwrap().text,
            "synthetic note"
        );
        std::thread::sleep(Duration::from_millis(1300));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "synthetic note");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        tx.send(ScratchpadCommand::SetText("last words".into()))
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        w.stop();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "last words",
            "flushed on stop"
        );
        assert_eq!(load(&path).unwrap(), "last words");
        let _ = std::fs::remove_dir_all(dir);
    }
}
