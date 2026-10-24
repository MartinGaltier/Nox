//! Engine + real helper client + real helper server loop (unprivileged, on a
//! temporary socket, fake pmset backend): the closed-lid path end to end.

use nox_core::model::power::{
    HelperStatus, KeepAwakeDuration, KeepAwakeMode, PowerSettings, StopReason,
};
use nox_helper::backend::FakeBackend;
use nox_helper::core::HelperCore;
use nox_helper::journal::{JournalStore, Loaded};
use nox_helper::peer::SameUserPolicy;
use nox_helper::server::{Server, StopHandle, stop_pair};
use nox_power::assertions::{AssertionId, AssertionKind, Assertions};
use nox_power::engine::Engine;
use nox_power::lid::{HelperClient, LidControl, Marker};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct NoAssertions;
impl Assertions for NoAssertions {
    fn create(&mut self, _: AssertionKind) -> Result<AssertionId, String> {
        Ok(1)
    }
    fn release(&mut self, _: AssertionId) -> Result<(), String> {
        Ok(())
    }
}

struct Helper {
    stop: StopHandle,
    thread: Option<JoinHandle<()>>,
    fake: FakeBackend,
    journal: JournalStore,
    sock: PathBuf,
    dir: PathBuf,
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn helper(name: &str) -> Helper {
    let dir = PathBuf::from(format!("/tmp/nxp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("h.sock");
    let journal = JournalStore::new(dir.join("helper-state.json"));
    let fake = FakeBackend::new();
    let (stop, rx) = stop_pair().unwrap();
    let mut server = Server::bind(
        &sock,
        SameUserPolicy {
            allowed_client: None,
        },
    )
    .unwrap();
    let mut core = HelperCore::new(fake.clone(), journal.clone());
    let thread = std::thread::spawn(move || {
        server.run(&mut core, &rx).unwrap();
        core.shutdown();
    });
    Helper {
        stop,
        thread: Some(thread),
        fake,
        journal,
        sock,
        dir,
    }
}

fn now() -> f64 {
    nox_core::model::now()
}

#[test]
fn closed_lid_session_sets_and_restores() {
    let h = helper("session");
    let marker = Marker::new(h.dir.join("marker.json"));
    let client = HelperClient::with_socket(h.sock.clone(), Some(marker.clone()));
    let mut e = Engine::new(NoAssertions, client, PowerSettings::default());
    e.refresh();
    assert_eq!(e.state().helper, HelperStatus::Installed);
    e.start(
        KeepAwakeMode::ClosedLid,
        KeepAwakeDuration::Minutes(15),
        now(),
    );
    assert!(e.is_active(), "{:?}", e.state().last_error);
    assert_eq!(h.fake.with(|s| s.sleep_disabled), Some(true));
    assert!(matches!(h.journal.load(), Loaded::Present(_)));
    assert!(marker.exists());
    // A heartbeat round trip.
    e.tick(now() + 16.0);
    assert!(e.is_active());
    e.stop(StopReason::User, now());
    assert_eq!(h.fake.with(|s| s.sleep_disabled), Some(false));
    assert_eq!(h.fake.with(|s| s.writes.clone()), vec![true, false]);
    assert_eq!(h.journal.load(), Loaded::Absent);
    assert!(!marker.exists());
}

#[test]
fn already_disabled_by_user_is_left_alone() {
    let h = helper("user");
    h.fake.with(|s| s.sleep_disabled = Some(true));
    let client = HelperClient::with_socket(h.sock.clone(), None);
    let mut e = Engine::new(NoAssertions, client, PowerSettings::default());
    e.start(
        KeepAwakeMode::ClosedLid,
        KeepAwakeDuration::Unlimited,
        now(),
    );
    assert!(e.is_active());
    e.stop(StopReason::User, now());
    assert!(h.fake.with(|s| s.writes.is_empty()));
    assert_eq!(h.fake.with(|s| s.sleep_disabled), Some(true));
}

/// The app "crashes" (engine and client dropped, no stop, no release): the
/// helper restores once the lease runs out.
#[test]
fn app_crash_is_restored_by_the_helper() {
    let h = helper("crash");
    let client = HelperClient::with_socket(h.sock.clone(), None).with_lease_secs(5);
    let mut e = Engine::new(NoAssertions, client, PowerSettings::default());
    e.start(
        KeepAwakeMode::ClosedLid,
        KeepAwakeDuration::Unlimited,
        now(),
    );
    assert_eq!(h.fake.with(|s| s.sleep_disabled), Some(true));
    let t = Instant::now();
    std::mem::forget(e); // no destructor at all, like kill -9…
    // …except the kernel closing our socket, which forget() prevents; so the
    // helper sees no disconnect and must rely on the missing heartbeats.
    while h.fake.with(|s| s.sleep_disabled) != Some(false) && t.elapsed() < Duration::from_secs(15)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(h.fake.with(|s| s.sleep_disabled), Some(false));
    assert!(t.elapsed() >= Duration::from_secs(4));
    assert_eq!(h.journal.load(), Loaded::Absent);
}

#[test]
fn helper_battery_guard_reaches_the_engine() {
    let h = helper("battery");
    let client = HelperClient::with_socket(h.sock.clone(), None).with_lease_secs(20);
    let mut e = Engine::new(NoAssertions, client, PowerSettings::default());
    e.start(
        KeepAwakeMode::ClosedLid,
        KeepAwakeDuration::Unlimited,
        now(),
    );
    assert!(e.is_active());
    // Unplugged and drained while the app saw nothing (the helper polls on
    // its own every 10 s).
    h.fake.with(|s| {
        s.power = Some(nox_helper::sys::PowerSourceInfo {
            on_battery: true,
            battery_percent: Some(5),
        })
    });
    let t = Instant::now();
    while h.fake.with(|s| s.sleep_disabled) != Some(false) && t.elapsed() < Duration::from_secs(15)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(h.fake.with(|s| s.sleep_disabled), Some(false));
    e.tick(now() + 6.0);
    assert!(!e.is_active());
    assert_eq!(e.state().last_stop, Some(StopReason::BatteryThreshold));
}

#[test]
fn status_without_helper() {
    let mut c = HelperClient::with_socket(PathBuf::from("/tmp/nox-no-such.sock"), None);
    assert_eq!(c.status(), HelperStatus::Unreachable);
}
