//! The restore journal. Written (fsync + atomic rename, mode 0600) *before*
//! the helper changes `SleepDisabled`, removed only once the original value is
//! back (or found changed by someone else). Its presence at helper start —
//! i.e. after a crash of the helper itself or a reboot — triggers a restore.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    /// `SleepDisabled` before Nox touched it. Only `false` journals are ever
    /// written (when it was already `true` Nox changes nothing).
    pub initial_sleep_disabled: bool,
    /// The value Nox set (always `true`).
    pub applied_sleep_disabled: bool,
    /// Client uid, for diagnostics.
    pub uid: u32,
    /// Unix seconds.
    pub since: u64,
}

impl Journal {
    pub fn new(uid: u32) -> Self {
        Self {
            version: 1,
            initial_sleep_disabled: false,
            applied_sleep_disabled: true,
            uid,
            since: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

/// What was found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    Absent,
    Present(Journal),
    /// Unreadable / corrupt. Only Nox writes this file, so it is treated as
    /// "Nox set SleepDisabled=1 from 0".
    Corrupt,
}

#[derive(Debug, Clone)]
pub struct JournalStore {
    path: PathBuf,
}

impl JournalStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Loaded {
        match std::fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice::<Journal>(&bytes) {
                Ok(j) => Loaded::Present(j),
                Err(_) => Loaded::Corrupt,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Absent,
            Err(_) => Loaded::Corrupt,
        }
    }

    /// Durable write: the record must survive a power cut right after the
    /// `pmset` call that follows.
    pub fn store(&self, j: &Journal) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(j).map_err(std::io::Error::other)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        if let Some(parent) = self.path.parent()
            && let Ok(d) = std::fs::File::open(parent)
        {
            let _ = d.sync_all();
        }
        Ok(())
    }

    pub fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_load_clear() {
        let dir = std::env::temp_dir().join(format!("nox-journal-{}", std::process::id()));
        let store = JournalStore::new(dir.join("helper-state.json"));
        assert_eq!(store.load(), Loaded::Absent);
        let j = Journal::new(501);
        store.store(&j).unwrap();
        assert_eq!(store.load(), Loaded::Present(j));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::write(store.path(), b"{garbage").unwrap();
        assert_eq!(store.load(), Loaded::Corrupt);
        store.clear().unwrap();
        store.clear().unwrap();
        assert_eq!(store.load(), Loaded::Absent);
        let _ = std::fs::remove_dir_all(dir);
    }
}
