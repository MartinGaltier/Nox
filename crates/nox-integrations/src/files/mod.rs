// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchFileToolsService.swift, commit e916961.

//! File shelf (E4): references to files dropped on the island (no copies,
//! memory only), open/reveal, copy paths, AirDrop, compress to zip with a
//! one-shot `/usr/bin/ditto` or `/usr/bin/zip` (killed if the module stops).

use crate::sys::workspace;
use crossbeam_channel::{Receiver, Sender, unbounded};
use nox_core::model::files::{FileCommand, FilesState, ShelfItem};
use nox_core::{CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Worker};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_ITEMS: usize = 50;
const COMPRESS_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct FilesModule {
    worker: Option<Worker>,
    tx: Option<Sender<FileCommand>>,
}

impl Module for FilesModule {
    fn id(&self) -> ModuleId {
        ModuleId::Files
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        hub.update(|s| s.files = Some(FilesState::default()));
        self.worker = Some(Worker::spawn("files", move |token| run(hub, rx, token)));
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
        if let (nox_core::Command::Files(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(c);
        }
    }
}

/// Builds a shelf item for an existing path (`None` if it does not exist).
pub fn item_for(id: u64, path: &str) -> Option<ShelfItem> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return None;
    }
    let meta = std::fs::symlink_metadata(p).ok()?;
    Some(ShelfItem {
        id,
        path: path.to_owned(),
        name: p.file_name()?.to_string_lossy().into_owned(),
        is_dir: meta.is_dir(),
        size: meta.is_file().then_some(meta.len()),
    })
}

/// `dir/base.zip`, or `dir/base 2.zip`… if taken.
pub fn unique_zip(dir: &Path, base: &str) -> PathBuf {
    let first = dir.join(format!("{base}.zip"));
    if !first.exists() {
        return first;
    }
    (2..1000)
        .map(|n| dir.join(format!("{base} {n}.zip")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{base} {}.zip", std::process::id())))
}

/// The archive command: `ditto` keeps metadata for a single item; several
/// items from one folder go through `zip` run in that folder.
pub fn compress_command(paths: &[PathBuf]) -> Result<(Command, PathBuf), String> {
    let first = paths.first().ok_or("aucun élément")?;
    let parent = first
        .parent()
        .ok_or("dossier parent introuvable")?
        .to_path_buf();
    if paths.iter().any(|p| p.parent() != Some(parent.as_path())) {
        return Err("les éléments doivent être dans le même dossier".into());
    }
    if paths.len() == 1 {
        let name = first.file_name().ok_or("nom invalide")?.to_string_lossy();
        let out = unique_zip(&parent, &name);
        let mut c = Command::new("/usr/bin/ditto");
        c.args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
            .arg(first)
            .arg(&out);
        Ok((c, out))
    } else {
        let out = unique_zip(&parent, "Archive");
        let mut c = Command::new("/usr/bin/zip");
        c.args(["-r", "-y", "-q"]).arg(&out).current_dir(&parent);
        for p in paths {
            // "./name": a file named "-T" / "-TT" must never be read as a
            // zip option (-TT runs a command).
            c.arg(Path::new(".").join(p.file_name().ok_or("nom invalide")?));
        }
        Ok((c, out))
    }
}

struct Engine {
    hub: Hub,
    state: FilesState,
    next_id: u64,
}

impl Engine {
    fn publish(&self) {
        let s = self.state.clone();
        self.hub.update(move |st| {
            if st.files.is_some() {
                st.files = Some(s);
            }
        });
    }

    /// Drops items whose file moved or was deleted.
    fn prune(&mut self) {
        self.state
            .items
            .retain(|i| std::fs::symlink_metadata(&i.path).is_ok());
    }

    fn paths(&self, ids: &[u64]) -> Vec<String> {
        self.state
            .items
            .iter()
            .filter(|i| ids.contains(&i.id))
            .map(|i| i.path.clone())
            .collect()
    }

    fn result(&mut self, r: Result<String, String>) {
        self.state.last_result = Some(r);
    }

    fn on_command(&mut self, c: FileCommand, token: &CancelToken) {
        self.prune();
        match c {
            FileCommand::Add(paths) => {
                for p in paths {
                    if self.state.items.len() >= MAX_ITEMS {
                        self.result(Err(format!("étagère pleine ({MAX_ITEMS} éléments)")));
                        break;
                    }
                    if self.state.items.iter().any(|i| i.path == p) {
                        continue;
                    }
                    if let Some(item) = item_for(self.next_id, &p) {
                        self.next_id += 1;
                        self.state.items.push(item);
                    }
                }
            }
            FileCommand::Remove(id) => self.state.items.retain(|i| i.id != id),
            FileCommand::Clear => self.state.items.clear(),
            FileCommand::Open(id) => {
                if let Some(p) = self.paths(&[id]).first() {
                    workspace::open_path(p);
                }
            }
            FileCommand::Reveal(id) => {
                workspace::reveal(&self.paths(&[id]));
            }
            FileCommand::CopyPaths(ids) => {
                let paths = self.paths(&ids);
                if !paths.is_empty() {
                    let ok = workspace::copy_text(&paths.join("\n"));
                    self.result(if ok {
                        Ok(format!("{} chemin(s) copié(s)", paths.len()))
                    } else {
                        Err("copie impossible".into())
                    });
                }
            }
            FileCommand::AirDrop(ids) => {
                let paths = self.paths(&ids);
                if !paths.is_empty() && !workspace::airdrop(&paths) {
                    self.result(Err("AirDrop indisponible (Wi-Fi/Bluetooth ?)".into()));
                }
            }
            FileCommand::Compress(ids) => {
                let paths: Vec<PathBuf> = self.paths(&ids).into_iter().map(PathBuf::from).collect();
                self.state.compressing = true;
                self.publish();
                let r = compress(&paths, token);
                self.state.compressing = false;
                if let Ok(out) = &r {
                    // The new archive joins the shelf.
                    if let Some(item) = item_for(self.next_id, &out.to_string_lossy()) {
                        self.next_id += 1;
                        self.state.items.push(item);
                    }
                }
                self.result(r.map(|p| {
                    format!(
                        "{} créé",
                        p.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    )
                }));
            }
        }
        self.publish();
    }
}

/// Runs the archiver; kills it if the module stops or it exceeds the timeout.
fn compress(paths: &[PathBuf], token: &CancelToken) -> Result<PathBuf, String> {
    let (mut cmd, out) = compress_command(paths)?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(out),
            Ok(Some(status)) => {
                let _ = std::fs::remove_file(&out);
                return Err(format!("échec de la compression ({status})"));
            }
            Ok(None) if token.is_cancelled() || start.elapsed() > COMPRESS_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&out);
                return Err("compression interrompue".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn run(hub: Hub, rx: Receiver<FileCommand>, token: CancelToken) {
    let mut e = Engine {
        hub,
        state: FilesState::default(),
        next_id: 1,
    };
    e.publish();
    // Only commands wake this thread; `stop` drops the sender to end the loop.
    while let Ok(c) = rx.recv() {
        e.on_command(c, &token);
    }
    e.hub.update(|s| s.files = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nox-files-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn items_and_unique_names() {
        let d = tmp("items");
        std::fs::write(d.join("a.txt"), b"hello").unwrap();
        let it = item_for(1, &d.join("a.txt").to_string_lossy()).unwrap();
        assert_eq!(
            (it.name.as_str(), it.size, it.is_dir),
            ("a.txt", Some(5), false)
        );
        assert!(item_for(2, "relative/path").is_none());
        assert!(item_for(2, &d.join("missing").to_string_lossy()).is_none());
        std::fs::write(d.join("a.txt.zip"), b"").unwrap();
        assert_eq!(unique_zip(&d, "a.txt"), d.join("a.txt 2.zip"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn option_like_names_are_not_options() {
        let d = tmp("inject");
        // zip runs -TT's command in the archive's folder: it would create PWNED there.
        let marker = d.join("PWNED");
        let names = ["-T", "-TT", "touch PWNED"];
        for n in names {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let paths: Vec<PathBuf> = names.iter().map(|n| d.join(n)).collect();
        let out = compress(&paths, &CancelToken::new()).unwrap();
        assert!(
            !marker.exists(),
            "zip must not run a command from a file name"
        );
        assert!(std::fs::metadata(out).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn compress_requires_same_folder() {
        let r = compress_command(&[PathBuf::from("/a/x"), PathBuf::from("/b/y")]);
        assert!(r.is_err());
    }

    #[test]
    fn compresses_synthetic_files() {
        let d = tmp("zip");
        std::fs::write(d.join("one.txt"), b"1").unwrap();
        std::fs::write(d.join("two.txt"), b"2").unwrap();
        let t = CancelToken::new();
        let single = compress(&[d.join("one.txt")], &t).unwrap();
        assert_eq!(single, d.join("one.txt.zip"));
        let multi = compress(&[d.join("one.txt"), d.join("two.txt")], &t).unwrap();
        assert_eq!(multi, d.join("Archive.zip"));
        assert!(std::fs::metadata(&multi).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(d);
    }
}
