//! Closed-lid journal: what Nox changed, written *before* it changes it.
//!
//! A user-owned file (0600) under Application Support. It is what lets a
//! later process (the guard, the next launch, the LaunchAgent after a reboot)
//! put `SleepDisabled` back without any root daemon: `pmset -a disablesleep 0`
//! runs through the sudoers rule, and the journal says whether Nox is the one
//! that set it.

use crate::guard::GuardReason;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Journal {
    /// Nox set `SleepDisabled` to 1 and has not restored it yet.
    pub active: bool,
    /// `SleepDisabled` before Nox touched it. Never `true` while `active`:
    /// a value already set by the user is left alone.
    pub initial: bool,
    /// Pid of the Nox process that owns the session.
    pub owner_pid: i32,
    /// Pid of the guard process (`nox --lid-guard`), 0 when none.
    pub guard_pid: i32,
    pub since: f64,
    /// Battery guard parameters, copied from the settings so the guard can
    /// apply them without the app.
    pub only_on_ac: bool,
    pub min_battery: u8,
    /// Set by the guard when it restored lid sleep on its own, so the app can
    /// tell the user why the session stopped.
    pub guard_reason: Option<GuardReason>,
}

impl Journal {
    /// The journal of a session that is starting, before anything is changed.
    pub fn starting(owner_pid: i32, since: f64, only_on_ac: bool, min_battery: u8) -> Self {
        Self {
            active: true,
            initial: false,
            owner_pid,
            guard_pid: 0,
            since,
            only_on_ac,
            min_battery,
            guard_reason: None,
        }
    }
}

pub fn default_path() -> PathBuf {
    nox_core::paths::app_support_dir()
        .join("power")
        .join("lid-state.json")
}

/// `None` when there is no journal. A journal that cannot be read is treated
/// as "Nox set it" (the conservative reading: restoring `0` is the safe side).
pub fn load(path: &Path) -> Option<Journal> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("closed-lid journal unreadable ({e}), assuming Nox set it");
            return Some(corrupt());
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(j) => Some(j),
        Err(e) => {
            log::warn!("closed-lid journal corrupt ({e}), assuming Nox set it");
            Some(corrupt())
        }
    }
}

fn corrupt() -> Journal {
    Journal {
        active: true,
        ..Journal::default()
    }
}

pub fn store(path: &Path, j: &Journal) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(j).map_err(std::io::Error::other)?;
    nox_core::paths::write_atomic(path, &body)
}

pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nox-journal-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("lid-state.json")
    }

    #[test]
    fn round_trip_and_clear() {
        let p = tmp("round");
        clear(&p);
        assert_eq!(load(&p), None);
        let mut j = Journal::starting(42, 1.5, true, 20);
        j.guard_pid = 7;
        store(&p, &j).unwrap();
        assert_eq!(load(&p), Some(j));
        clear(&p);
        assert_eq!(load(&p), None);
    }

    #[test]
    fn corrupt_journal_means_nox_set_it() {
        let p = tmp("corrupt");
        std::fs::write(&p, b"{ not json").unwrap();
        let j = load(&p).unwrap();
        assert!(j.active && !j.initial);
        clear(&p);
    }

    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("mode");
        clear(&p);
        store(&p, &Journal::starting(1, 0.0, false, 0)).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        clear(&p);
    }
}
