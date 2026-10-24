//! Launch-time reconciliation. The helper restores lid sleep on its own
//! (lease, journal); this only shortens the wait and tells the user what
//! happened when the previous run ended while a closed-lid session was held.

use crate::lid::{HelperClient, LidControl, LidError, Marker};
use nox_core::model::power::HelperStatus;
use nox_helper::protocol::RestoreOutcome;
use std::sync::Mutex;

static NOTICE: Mutex<Option<String>> = Mutex::new(None);

/// Taken once by the module when it starts (shown as notice + alert).
pub fn take_notice() -> Option<String> {
    NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub fn startup_recovery() {
    let marker = Marker::default_location();
    if !marker.exists() {
        return;
    }
    let mut client = HelperClient::system();
    let note = reconcile(&mut client, &marker);
    log::warn!("startup recovery: {note}");
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(note);
}

/// A previous run held a closed-lid lease and never released it.
pub(crate) fn reconcile(client: &mut impl LidControl, marker: &Marker) -> String {
    let unexpected = "Nox s’est arrêté pendant le mode capot fermé : ";
    match client.status() {
        HelperStatus::Installed | HelperStatus::Outdated => match client.release() {
            Ok(RestoreOutcome::Restored) => {
                marker.clear();
                format!("{unexpected}veille capot fermé rétablie.")
            }
            Ok(RestoreOutcome::Untouched | RestoreOutcome::Nothing) => {
                marker.clear();
                format!("{unexpected}l’assistant avait déjà rétabli la veille.")
            }
            Ok(RestoreOutcome::Failed) => format!(
                "{unexpected}rétablissement en échec, l’assistant réessaie. \
                 Vérifiez avec « pmset -g | grep SleepDisabled »."
            ),
            Err(e) => format!("{unexpected}{}", e.message_fr()),
        },
        HelperStatus::NotInstalled => {
            marker.clear();
            match client.sleep_disabled().value() {
                Some(true) => format!(
                    "{unexpected}l’assistant n’est plus installé et la veille capot fermé \
                     est toujours désactivée. Rétablissez-la avec « sudo pmset -a disablesleep 0 »."
                ),
                _ => format!("{unexpected}la veille capot fermé est normale."),
            }
        }
        HelperStatus::Unreachable | HelperStatus::ClientRejected => format!(
            "{unexpected}{} L’assistant rétablit la veille de lui-même après {} s.",
            LidError::Unreachable("pas de réponse".into()).message_fr(),
            crate::lid::LEASE_SECS
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_helper::backend::FakeBackend;
    use nox_helper::core::HelperCore;
    use nox_helper::journal::JournalStore;
    use nox_helper::peer::SameUserPolicy;
    use nox_helper::server::{Server, stop_pair};
    use std::path::PathBuf;

    #[test]
    fn relaunch_releases_the_orphaned_lease_at_once() {
        let dir = PathBuf::from(format!("/tmp/nxr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("h.sock");
        let fake = FakeBackend::new();
        let (stop, rx) = stop_pair().unwrap();
        let mut server = Server::bind(
            &sock,
            SameUserPolicy {
                allowed_client: None,
            },
        )
        .unwrap();
        let mut core = HelperCore::new(fake.clone(), JournalStore::new(dir.join("j.json")));
        let t = std::thread::spawn(move || {
            server.run(&mut core, &rx).unwrap();
        });

        let marker = Marker::new(dir.join("marker.json"));
        // Previous run: acquires (default 60 s lease) then dies.
        {
            let mut old = HelperClient::with_socket(sock.clone(), Some(marker.clone()));
            old.acquire(20).unwrap();
        }
        assert!(marker.exists());
        assert_eq!(fake.with(|s| s.sleep_disabled), Some(true));

        let mut fresh = HelperClient::with_socket(sock.clone(), Some(marker.clone()));
        let note = reconcile(&mut fresh, &marker);
        assert!(note.contains("rétablie"), "{note}");
        assert_eq!(fake.with(|s| s.sleep_disabled), Some(false), "no 60 s wait");
        assert!(!marker.exists());

        stop.stop();
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
