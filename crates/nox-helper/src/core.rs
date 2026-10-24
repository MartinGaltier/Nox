//! Lease state machine and restore logic. Pure: time is injected, side
//! effects go through [`SleepBackend`] and [`JournalStore`].
//!
//! Invariants:
//! - `SleepDisabled` is only set to 1 after a journal recording the original
//!   0 is durably on disk; the journal is only removed once 0 is back, or once
//!   the setting is found changed by someone else.
//! - Nox never writes when the original value was already 1, and never writes
//!   0 over a value it did not set (re-read before restoring).
//! - Without a live, heartbeating client the setting is restored after the
//!   lease (grace) — the app's destructors are never relied upon.

use crate::backend::{SleepBackend, THERMAL_HEAVY};
use crate::journal::{Journal, JournalStore, Loaded};
use crate::log_line;
use crate::protocol::{
    ErrorCode, LeaseState, Request, Response, RestoreOutcome, RevokeReason, Status,
};
use std::time::{Duration, Instant};

pub type ClientId = u64;

/// How often guards (battery, thermal, external change) are evaluated while
/// a lease is held.
pub const GUARD_INTERVAL: Duration = Duration::from_secs(10);
/// Retry period for a restore that failed (journal kept).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct Lease {
    owner: ClientId,
    uid: u32,
    lease: Duration,
    min_battery: u8,
    last_seen: Instant,
    disconnected_at: Option<Instant>,
    /// Nox changed the setting (false when it was already 1).
    applied: bool,
}

impl Lease {
    fn deadline(&self) -> Instant {
        self.disconnected_at.unwrap_or(self.last_seen) + self.lease
    }
}

pub struct HelperCore<B: SleepBackend> {
    backend: B,
    journal: JournalStore,
    lease: Option<Lease>,
    revoked: Option<RevokeReason>,
}

impl<B: SleepBackend> HelperCore<B> {
    pub fn new(backend: B, journal: JournalStore) -> Self {
        Self {
            backend,
            journal,
            lease: None,
            revoked: None,
        }
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Helper start (boot, or relaunch by launchd after a crash): no client
    /// can hold a lease yet, so a journal means a pending restore.
    pub fn startup(&mut self) -> RestoreOutcome {
        let o = self.restore_journal();
        if o != RestoreOutcome::Nothing {
            log_line(&format!("startup recovery: {}", o.as_str()));
        }
        o
    }

    /// Helper stopping (SIGTERM from `launchctl bootout`, shutdown).
    pub fn shutdown(&mut self) -> RestoreOutcome {
        let o = if self.lease.is_some() {
            self.end_lease(RevokeReason::Shutdown)
        } else {
            self.restore_journal()
        };
        if o != RestoreOutcome::Nothing {
            log_line(&format!("shutdown: {}", o.as_str()));
        }
        o
    }

    pub fn handle(&mut self, client: ClientId, uid: u32, req: Request, now: Instant) -> Response {
        match req {
            Request::Version => Response::Version {
                protocol: crate::PROTOCOL_VERSION,
                version: crate::VERSION.into(),
            },
            Request::Status => Response::Status(Status {
                sleep_disabled: self.backend.sleep_disabled().ok(),
                lease: match &self.lease {
                    None => LeaseState::None,
                    Some(l) if l.disconnected_at.is_some() => LeaseState::Orphaned,
                    Some(_) => LeaseState::Held,
                },
                applied: self.lease.as_ref().is_some_and(|l| l.applied),
                revoked: self.revoked,
            }),
            Request::Acquire {
                lease_secs,
                min_battery,
            } => self.acquire(client, uid, lease_secs, min_battery, now),
            Request::Heartbeat => match &mut self.lease {
                Some(l) if l.owner == client => {
                    l.last_seen = now;
                    Response::Heartbeat
                }
                Some(_) => Response::err(ErrorCode::NoLease, "lease held by another connection"),
                None => match self.revoked {
                    Some(r) => Response::err(ErrorCode::Revoked(r), ""),
                    None => Response::err(ErrorCode::NoLease, ""),
                },
            },
            Request::Release => match &self.lease {
                Some(l) if l.owner == client || l.uid == uid || uid == 0 => {
                    let o = self.restore_lease();
                    self.lease = None;
                    self.revoked = None;
                    log_line(&format!("released by client: {}", o.as_str()));
                    Response::Released(o)
                }
                Some(_) => Response::err(ErrorCode::Busy, "lease held by another user"),
                None => Response::Released(self.restore_journal()),
            },
        }
    }

    fn acquire(
        &mut self,
        client: ClientId,
        uid: u32,
        lease_secs: u32,
        min_battery: u8,
        now: Instant,
    ) -> Response {
        if let Some(p) = self.backend.power_source()
            && p.below(min_battery)
        {
            return Response::err(
                ErrorCode::Battery,
                format!("on battery at {} %", p.battery_percent.unwrap_or(0)),
            );
        }
        if self
            .backend
            .thermal_pressure()
            .is_some_and(|t| t >= THERMAL_HEAVY)
        {
            return Response::err(ErrorCode::Thermal, "thermal pressure heavy");
        }
        let lease = Duration::from_secs(u64::from(lease_secs));

        if let Some(l) = &mut self.lease {
            // Same client refreshing, or the app relaunched and taking its
            // orphaned lease back: nothing to change on the system.
            if l.owner != client && l.uid != uid && l.disconnected_at.is_none() {
                return Response::err(ErrorCode::Busy, "lease held by another user");
            }
            l.owner = client;
            l.uid = uid;
            l.lease = lease;
            l.min_battery = min_battery;
            l.last_seen = now;
            l.disconnected_at = None;
            self.revoked = None;
            return Response::Acquired { applied: l.applied };
        }

        let current = match self.backend.sleep_disabled() {
            Ok(v) => v,
            Err(e) => return Response::err(ErrorCode::Failed, format!("read SleepDisabled: {e}")),
        };
        let leftover = !matches!(self.journal.load(), Loaded::Absent);
        let applied = if current && leftover {
            // A previous restore failed: the 1 is ours, the journal stays.
            true
        } else if current {
            // Already disabled outside Nox: leave it exactly as is.
            false
        } else {
            if leftover && let Err(e) = self.journal.clear() {
                return Response::err(ErrorCode::Failed, format!("clear journal: {e}"));
            }
            if let Err(e) = self.journal.store(&Journal::new(uid)) {
                return Response::err(ErrorCode::Failed, format!("write journal: {e}"));
            }
            if let Err(e) = self.backend.set_sleep_disabled(true) {
                if self.backend.sleep_disabled() == Ok(true) {
                    log_line(&format!("pmset reported {e} but SleepDisabled is 1"));
                } else {
                    let _ = self.journal.clear();
                    return Response::err(ErrorCode::Failed, e);
                }
            }
            true
        };
        log_line(&format!(
            "lease acquired (uid {uid}, {lease_secs} s, applied={applied})"
        ));
        self.lease = Some(Lease {
            owner: client,
            uid,
            lease,
            min_battery,
            last_seen: now,
            disconnected_at: None,
            applied,
        });
        self.revoked = None;
        Response::Acquired { applied }
    }

    /// The connection `client` closed.
    pub fn disconnected(&mut self, client: ClientId, now: Instant) {
        if let Some(l) = &mut self.lease
            && l.owner == client
            && l.disconnected_at.is_none()
        {
            l.disconnected_at = Some(now);
            log_line("lease owner disconnected; grace started");
        }
    }

    /// Evaluates deadlines and guards. Returns when to call again (`None` =
    /// only on the next event).
    pub fn tick(&mut self, now: Instant) -> Option<Duration> {
        if let Some(l) = &self.lease {
            let reason = if now >= l.deadline() {
                Some(if l.disconnected_at.is_some() {
                    RevokeReason::ClientGone
                } else {
                    RevokeReason::LeaseExpired
                })
            } else if self
                .backend
                .power_source()
                .is_some_and(|p| p.below(l.min_battery))
            {
                Some(RevokeReason::Battery)
            } else if self
                .backend
                .thermal_pressure()
                .is_some_and(|t| t >= THERMAL_HEAVY)
            {
                Some(RevokeReason::Thermal)
            } else if l.applied && self.backend.sleep_disabled() == Ok(false) {
                Some(RevokeReason::External)
            } else {
                None
            };
            if let Some(r) = reason {
                let o = self.end_lease(r);
                log_line(&format!("lease revoked ({}): {}", r.as_str(), o.as_str()));
            }
        } else if !matches!(self.journal.load(), Loaded::Absent) {
            let o = self.restore_journal();
            log_line(&format!("restore retry: {}", o.as_str()));
        }

        match &self.lease {
            Some(l) => Some(
                l.deadline()
                    .saturating_duration_since(now)
                    .min(GUARD_INTERVAL),
            ),
            None if !matches!(self.journal.load(), Loaded::Absent) => Some(RETRY_INTERVAL),
            None => None,
        }
    }

    fn end_lease(&mut self, reason: RevokeReason) -> RestoreOutcome {
        let o = if reason == RevokeReason::External {
            // Someone else already put lid sleep back: just forget our record.
            let _ = self.journal.clear();
            RestoreOutcome::Untouched
        } else {
            self.restore_lease()
        };
        self.lease = None;
        self.revoked = Some(reason);
        o
    }

    fn restore_lease(&mut self) -> RestoreOutcome {
        match &self.lease {
            Some(l) if l.applied => self.restore_journal(),
            Some(_) => RestoreOutcome::Untouched,
            None => self.restore_journal(),
        }
    }

    /// Restores from the journal, re-reading the state first.
    fn restore_journal(&mut self) -> RestoreOutcome {
        let ours = match self.journal.load() {
            Loaded::Absent => return RestoreOutcome::Nothing,
            Loaded::Present(j) => !j.initial_sleep_disabled && j.applied_sleep_disabled,
            Loaded::Corrupt => true,
        };
        if !ours {
            let _ = self.journal.clear();
            return RestoreOutcome::Untouched;
        }
        match self.backend.sleep_disabled() {
            Ok(false) => {
                // Changed back by the user (or never applied): respect it.
                let _ = self.journal.clear();
                RestoreOutcome::Untouched
            }
            // Unreadable: fail safe towards normal sleep (a Mac that cannot
            // sleep in a bag is the dangerous outcome).
            Ok(true) | Err(_) => match self.backend.set_sleep_disabled(false) {
                Ok(()) => {
                    if let Err(e) = self.journal.clear() {
                        log_line(&format!("restored but journal not removed: {e}"));
                    }
                    self.backend.sleep_if_lid_closed();
                    RestoreOutcome::Restored
                }
                Err(e) => {
                    log_line(&format!("restore failed, journal kept: {e}"));
                    RestoreOutcome::Failed
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::FakeBackend;
    use crate::sys::PowerSourceInfo;

    const UID: u32 = 501;

    struct T {
        core: HelperCore<FakeBackend>,
        fake: FakeBackend,
        store: JournalStore,
        t0: Instant,
        _dir: TempDir,
    }

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str) -> T {
        let dir = std::env::temp_dir().join(format!("nox-core-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = JournalStore::new(dir.join("helper-state.json"));
        let fake = FakeBackend::new();
        T {
            core: HelperCore::new(fake.clone(), store.clone()),
            fake,
            store,
            t0: Instant::now(),
            _dir: TempDir(dir),
        }
    }

    fn acquire(lease: u32, min_battery: u8) -> Request {
        Request::Acquire {
            lease_secs: lease,
            min_battery,
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn acquire_writes_journal_then_release_restores() {
        let mut t = setup("basic");
        let probe = t.store.path().to_owned();
        t.fake.with(|s| s.journal_probe = Some(probe));
        let r = t.core.handle(1, UID, acquire(60, 20), t.t0);
        assert_eq!(r, Response::Acquired { applied: true });
        assert!(matches!(t.store.load(), Loaded::Present(j) if !j.initial_sleep_disabled));
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(true));
        assert_eq!(
            t.fake.with(|s| s.journal_present_at_enable.clone()),
            vec![true],
            "journal is durable before pmset runs"
        );
        assert_eq!(
            t.core.handle(1, UID, Request::Release, t.t0 + secs(5)),
            Response::Released(RestoreOutcome::Restored)
        );
        assert_eq!(t.fake.with(|s| s.writes.clone()), vec![true, false]);
        assert_eq!(t.store.load(), Loaded::Absent);
        assert_eq!(t.core.tick(t.t0 + secs(6)), None, "idle: no wakeups");
    }

    #[test]
    fn already_disabled_is_left_alone() {
        let mut t = setup("already");
        t.fake.with(|s| s.sleep_disabled = Some(true));
        assert_eq!(
            t.core.handle(1, UID, acquire(60, 0), t.t0),
            Response::Acquired { applied: false }
        );
        assert_eq!(t.store.load(), Loaded::Absent);
        assert_eq!(
            t.core.handle(1, UID, Request::Release, t.t0),
            Response::Released(RestoreOutcome::Untouched)
        );
        assert!(t.fake.with(|s| s.writes.is_empty()), "never wrote");
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(true));
    }

    #[test]
    fn client_crash_restores_after_grace_only() {
        let mut t = setup("crash");
        t.core.handle(1, UID, acquire(30, 0), t.t0);
        t.core.handle(1, UID, Request::Heartbeat, t.t0 + secs(10));
        t.core.disconnected(1, t.t0 + secs(12));
        t.core.tick(t.t0 + secs(40));
        assert_eq!(
            t.fake.with(|s| s.sleep_disabled),
            Some(true),
            "within grace"
        );
        let next = t.core.tick(t.t0 + secs(41)).unwrap();
        assert!(next <= secs(1));
        t.core.tick(t.t0 + secs(42));
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(false));
        assert_eq!(t.store.load(), Loaded::Absent);
        // A late client learns why.
        assert_eq!(
            t.core.handle(2, UID, Request::Heartbeat, t.t0 + secs(43)),
            Response::err(ErrorCode::Revoked(RevokeReason::ClientGone), "")
        );
    }

    #[test]
    fn relaunched_app_takes_orphaned_lease_without_writes() {
        let mut t = setup("takeover");
        t.core.handle(1, UID, acquire(30, 0), t.t0);
        t.core.disconnected(1, t.t0 + secs(1));
        let r = t.core.handle(2, UID, Request::Status, t.t0 + secs(2));
        assert!(matches!(r, Response::Status(s) if s.lease == LeaseState::Orphaned));
        assert_eq!(
            t.core.handle(2, UID, acquire(30, 0), t.t0 + secs(5)),
            Response::Acquired { applied: true }
        );
        t.core.tick(t.t0 + secs(33));
        assert_eq!(
            t.fake.with(|s| s.writes.clone()),
            vec![true],
            "one write only"
        );
        t.core.handle(2, UID, Request::Heartbeat, t.t0 + secs(20));
        t.core.handle(2, UID, Request::Release, t.t0 + secs(21));
        assert_eq!(t.fake.with(|s| s.writes.clone()), vec![true, false]);
    }

    #[test]
    fn frozen_app_heartbeat_timeout_restores() {
        let mut t = setup("frozen");
        t.core.handle(1, UID, acquire(20, 0), t.t0);
        t.core.handle(1, UID, Request::Heartbeat, t.t0 + secs(15));
        t.core.tick(t.t0 + secs(34));
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(true));
        t.core.tick(t.t0 + secs(35));
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(false));
        assert_eq!(
            t.core.handle(1, UID, Request::Heartbeat, t.t0 + secs(36)),
            Response::err(ErrorCode::Revoked(RevokeReason::LeaseExpired), "")
        );
    }

    #[test]
    fn boot_with_journal_restores() {
        let mut t = setup("boot");
        t.store.store(&Journal::new(UID)).unwrap();
        t.fake.with(|s| s.sleep_disabled = Some(true));
        assert_eq!(t.core.startup(), RestoreOutcome::Restored);
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(false));
        assert_eq!(t.store.load(), Loaded::Absent);
    }

    #[test]
    fn boot_respects_user_change() {
        let mut t = setup("bootuser");
        t.store.store(&Journal::new(UID)).unwrap();
        // User already ran `sudo pmset -a disablesleep 0`.
        t.fake.with(|s| s.sleep_disabled = Some(false));
        assert_eq!(t.core.startup(), RestoreOutcome::Untouched);
        assert!(t.fake.with(|s| s.writes.is_empty()));
        assert_eq!(t.store.load(), Loaded::Absent);
    }

    #[test]
    fn corrupt_or_unreadable_fails_safe() {
        let mut t = setup("corrupt");
        std::fs::create_dir_all(t.store.path().parent().unwrap()).unwrap();
        std::fs::write(t.store.path(), b"\x00garbage").unwrap();
        t.fake.with(|s| s.sleep_disabled = None);
        assert_eq!(t.core.startup(), RestoreOutcome::Restored);
        assert_eq!(t.fake.with(|s| s.writes.clone()), vec![false]);
    }

    #[test]
    fn user_change_during_lease_is_respected() {
        let mut t = setup("external");
        t.core.handle(1, UID, acquire(60, 0), t.t0);
        t.fake.with(|s| s.sleep_disabled = Some(false));
        t.core.tick(t.t0 + secs(10));
        assert_eq!(t.store.load(), Loaded::Absent);
        assert_eq!(
            t.fake.with(|s| s.writes.clone()),
            vec![true],
            "no extra write"
        );
        assert_eq!(
            t.core.handle(1, UID, Request::Heartbeat, t.t0 + secs(11)),
            Response::err(ErrorCode::Revoked(RevokeReason::External), "")
        );
    }

    #[test]
    fn battery_guard_refuses_and_revokes() {
        let mut t = setup("battery");
        let low = PowerSourceInfo {
            on_battery: true,
            battery_percent: Some(15),
        };
        t.fake.with(|s| s.power = Some(low));
        assert!(matches!(
            t.core.handle(1, UID, acquire(60, 20), t.t0),
            Response::Err {
                code: ErrorCode::Battery,
                ..
            }
        ));
        assert!(t.fake.with(|s| s.writes.is_empty()));
        t.fake.with(|s| {
            s.power = Some(PowerSourceInfo {
                on_battery: false,
                battery_percent: Some(15),
            })
        });
        assert_eq!(
            t.core.handle(1, UID, acquire(60, 20), t.t0),
            Response::Acquired { applied: true }
        );
        // Unplugged while the app is frozen: the helper acts on its own.
        t.fake.with(|s| s.power = Some(low));
        t.core.tick(t.t0 + secs(10));
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(false));
        assert_eq!(
            t.core.handle(1, UID, Request::Heartbeat, t.t0 + secs(11)),
            Response::err(ErrorCode::Revoked(RevokeReason::Battery), "")
        );
    }

    #[test]
    fn thermal_guard_revokes() {
        let mut t = setup("thermal");
        t.core.handle(1, UID, acquire(60, 0), t.t0);
        t.fake.with(|s| s.thermal = Some(THERMAL_HEAVY));
        t.core.tick(t.t0 + secs(10));
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(false));
        assert!(matches!(
            t.core.handle(1, UID, acquire(60, 0), t.t0 + secs(11)),
            Response::Err {
                code: ErrorCode::Thermal,
                ..
            }
        ));
    }

    #[test]
    fn failed_restore_keeps_journal_and_retries() {
        let mut t = setup("retry");
        t.core.handle(1, UID, acquire(60, 0), t.t0);
        t.fake.with(|s| s.fail_writes = true);
        assert_eq!(
            t.core.handle(1, UID, Request::Release, t.t0),
            Response::Released(RestoreOutcome::Failed)
        );
        assert!(matches!(t.store.load(), Loaded::Present(_)));
        assert_eq!(t.core.tick(t.t0 + secs(1)), Some(RETRY_INTERVAL));
        t.fake.with(|s| s.fail_writes = false);
        assert_eq!(t.core.tick(t.t0 + secs(31)), None);
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(false));
        assert_eq!(t.store.load(), Loaded::Absent);
    }

    #[test]
    fn failed_enable_leaves_no_journal() {
        let mut t = setup("failenable");
        t.fake.with(|s| s.fail_writes = true);
        assert!(matches!(
            t.core.handle(1, UID, acquire(60, 0), t.t0),
            Response::Err {
                code: ErrorCode::Failed,
                ..
            }
        ));
        assert_eq!(t.store.load(), Loaded::Absent);
        assert_eq!(t.core.tick(t.t0), None);
    }

    #[test]
    fn restore_with_lid_closed_requests_sleep() {
        let mut t = setup("lid");
        t.core.handle(1, UID, acquire(60, 0), t.t0);
        t.fake.with(|s| s.lid_closed = true);
        t.core.disconnected(1, t.t0);
        t.core.tick(t.t0 + secs(60));
        assert_eq!(t.fake.with(|s| s.sleep_requests), 1);
    }

    #[test]
    fn shutdown_restores() {
        let mut t = setup("shutdown");
        t.core.handle(1, UID, acquire(60, 0), t.t0);
        assert_eq!(t.core.shutdown(), RestoreOutcome::Restored);
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(false));
    }

    #[test]
    fn other_user_cannot_steal_live_lease() {
        let mut t = setup("busy");
        t.core.handle(1, UID, acquire(60, 0), t.t0);
        assert!(matches!(
            t.core.handle(2, 502, acquire(60, 0), t.t0),
            Response::Err {
                code: ErrorCode::Busy,
                ..
            }
        ));
        assert!(matches!(
            t.core.handle(2, 502, Request::Release, t.t0),
            Response::Err {
                code: ErrorCode::Busy,
                ..
            }
        ));
        assert_eq!(t.fake.with(|s| s.sleep_disabled), Some(true));
    }
}
