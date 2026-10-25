//! Launch-time reconciliation (`startup_recovery`, and `nox --lid-restore`
//! from the LaunchAgent after a reboot). If the previous run left a journal
//! with lid sleep set by Nox, put it back now and tell the user.

use crate::guard::{self, RestoreOutcome};
use crate::journal::{self, Journal};
use crate::pmset::{Pmset, SleepSwitch};
use std::path::Path;
use std::sync::Mutex;

static NOTICE: Mutex<Option<String>> = Mutex::new(None);

/// Taken once by the module when it starts (shown as notice + alert).
pub fn take_notice() -> Option<String> {
    NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub fn startup_recovery() {
    let path = journal::default_path();
    let Some(j) = journal::load(&path) else {
        return;
    };
    let note = reconcile(&j, &path, &Pmset);
    log::warn!("startup recovery: {note}");
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(note);
}

/// A previous run ended while Nox held the closed-lid state.
pub(crate) fn reconcile(j: &Journal, path: &Path, sw: &dyn SleepSwitch) -> String {
    let unexpected = "Nox s’est arrêté pendant le mode capot fermé : ";
    stop_leftover_guard(j);
    if !j.active {
        journal::clear(path);
        return format!("{unexpected}la veille capot fermé était déjà rétablie.");
    }
    match guard::restore(j, sw) {
        Ok(RestoreOutcome::Restored) => {
            journal::clear(path);
            format!("{unexpected}veille capot fermé rétablie.")
        }
        Ok(RestoreOutcome::Untouched) => {
            journal::clear(path);
            format!(
                "{unexpected}la veille avait déjà été modifiée hors de Nox, laissée telle quelle."
            )
        }
        Err(e) => {
            // Kept: the next launch, or the LaunchAgent, tries again.
            format!(
                "{unexpected}rétablissement impossible ({e}), nouvel essai au prochain lancement."
            )
        }
    }
}

fn stop_leftover_guard(j: &Journal) {
    let own = std::env::current_exe().ok();
    guard::stop_pid(j.guard_pid, own.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmset::fake::FakeSwitch;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nox-recovery-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("lid-state.json")
    }

    #[test]
    fn leftover_active_journal_is_restored_at_once() {
        let path = tmp("active");
        let sw = FakeSwitch::new(true);
        let j = Journal::starting(999_999, 0.0, false, 20);
        journal::store(&path, &j).unwrap();
        let note = reconcile(&j, &path, &sw);
        assert!(note.contains("rétablie"), "{note}");
        assert!(!sw.with(|s| s.value));
        assert_eq!(journal::load(&path), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn users_own_setting_is_respected() {
        let path = tmp("user");
        let sw = FakeSwitch::new(false);
        sw.with(|s| s.value = false);
        let j = Journal::starting(1, 0.0, false, 20);
        let note = reconcile(&j, &path, &sw);
        assert!(note.contains("laissée"), "{note}");
        assert!(sw.with(|s| s.writes.is_empty()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn failed_restore_keeps_the_journal_for_the_next_launch() {
        let path = tmp("fail");
        let sw = FakeSwitch::new(true);
        sw.with(|s| s.refuse = true);
        let j = Journal::starting(1, 0.0, false, 20);
        journal::store(&path, &j).unwrap();
        let note = reconcile(&j, &path, &sw);
        assert!(note.contains("nouvel essai"), "{note}");
        assert!(journal::load(&path).is_some());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
