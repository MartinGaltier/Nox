//! Closed-lid guard: a small process (`nox --lid-guard <owner pid>`) that
//! outlives Nox when Nox dies, and restores lid sleep from the journal.
//!
//! It is the same binary, in its own process group, so a terminal Ctrl-C or a
//! `kill -9` of Nox leaves it running. It waits on the owner with kqueue
//! (`EVFILT_PROC`/`NOTE_EXIT`) and checks the battery and thermal pressure
//! every 10 s while Nox is alive, because `disablesleep` also disables the
//! software battery and thermal sleep protections. It never runs as root: the
//! only write is `sudo -n pmset -a disablesleep 0` through the sudoers rule.

use crate::journal::{self, Journal};
use crate::pmset::{Pmset, SleepSwitch, set_confirmed};
use crate::probe::{self, PowerSourceInfo};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Battery floor applied even when the user disabled the threshold (0).
pub const BATTERY_FLOOR_PERCENT: u8 = 5;
/// `com.apple.system.thermalpressurelevel`: 2 = heavy.
pub const THERMAL_HEAVY: u64 = 2;
const PERIOD: Duration = Duration::from_secs(10);
/// Retries after a failed restore (10 min at `PERIOD`), then the guard gives up.
const MAX_RESTORE_ATTEMPTS: u32 = 60;

/// Why lid sleep was restored while a session was running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardReason {
    Battery,
    Thermal,
    /// Restored by someone else (the user, or a guard after Nox disappeared).
    External,
}

pub type Sensors = (Option<PowerSourceInfo>, Option<u64>);

/// Reads the guard's sensors. A failed read never trips the guard.
pub fn sensors() -> Sensors {
    (probe::power_source(), probe::thermal_pressure())
}

/// The reason to restore now, if any. Battery: on battery and either AC-only
/// or at/below max(threshold, 5 %). Thermal: heavy pressure or worse.
pub fn guard_reason(
    j: &Journal,
    power: Option<PowerSourceInfo>,
    thermal: Option<u64>,
) -> Option<GuardReason> {
    if let Some(p) = power
        && p.on_battery
    {
        if j.only_on_ac {
            return Some(GuardReason::Battery);
        }
        let floor = j.min_battery.max(BATTERY_FLOOR_PERCENT);
        if p.battery_percent.is_some_and(|c| c <= floor) {
            return Some(GuardReason::Battery);
        }
    }
    thermal
        .is_some_and(|t| t >= THERMAL_HEAVY)
        .then_some(GuardReason::Thermal)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// `SleepDisabled` was set back to 0.
    Restored,
    /// The value was already 0 (changed outside Nox): nothing written.
    Untouched,
}

/// Puts lid sleep back, but only if the journal says Nox set it and the value
/// is still 1. A value the user changed is never overwritten. The journal is
/// left to the caller.
pub fn restore(j: &Journal, sw: &dyn SleepSwitch) -> Result<RestoreOutcome, String> {
    if !j.active {
        return Ok(RestoreOutcome::Untouched);
    }
    match sw.read() {
        Ok(false) => Ok(RestoreOutcome::Untouched),
        // Unreadable: restore to 0, the safe side.
        Ok(true) | Err(_) => {
            set_confirmed(sw, false)?;
            Ok(RestoreOutcome::Restored)
        }
    }
}

/// Stops a guard process, but only if the pid still is a Nox executable.
pub fn stop_pid(pid: i32, own_exe: Option<&Path>) {
    if pid <= 0 {
        return;
    }
    let is_ours = match (probe::pid_path(pid), own_exe) {
        (Some(p), Some(exe)) => p == exe,
        _ => false,
    };
    if is_ours {
        // SAFETY: kill(2) on a pid we just checked belongs to a Nox process.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Wait {
    Exited,
    Timeout,
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence and permission.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}

/// Waits up to `timeout` for `pid` to exit (kqueue `NOTE_EXIT`).
fn wait_for_exit(pid: i32, timeout: Duration) -> Wait {
    // SAFETY: kqueue/kevent take descriptors and structs we own; every
    // result is checked and the descriptor is closed on each path.
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return fallback(pid, timeout);
        }
        let change = libc::kevent {
            ident: pid as usize,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        if libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) < 0 {
            let gone = std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            libc::close(kq);
            return if gone {
                Wait::Exited
            } else {
                fallback(pid, timeout)
            };
        }
        let mut out: libc::kevent = std::mem::zeroed();
        let ts = libc::timespec {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_nsec: timeout.subsec_nanos() as libc::c_long,
        };
        let n = libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, &ts);
        libc::close(kq);
        match n {
            n if n > 0 && out.fflags & libc::NOTE_EXIT != 0 => Wait::Exited,
            0 => Wait::Timeout,
            _ => fallback(pid, timeout),
        }
    }
}

/// Without kqueue: poll existence once per period.
fn fallback(pid: i32, timeout: Duration) -> Wait {
    if !alive(pid) {
        return Wait::Exited;
    }
    std::thread::sleep(timeout);
    if alive(pid) {
        Wait::Timeout
    } else {
        Wait::Exited
    }
}

/// The guard loop. Returns the process exit code.
pub fn watch(
    owner: i32,
    journal_path: &Path,
    sw: &dyn SleepSwitch,
    sensors: &dyn Fn() -> Sensors,
    period: Duration,
) -> i32 {
    let mut attempts = 0;
    loop {
        let exited = wait_for_exit(owner, period) == Wait::Exited;
        let Some(j) = journal::load(journal_path) else {
            return 0;
        };
        if !j.active || j.owner_pid != owner {
            return 0;
        }
        let (power, thermal) = sensors();
        let reason = guard_reason(&j, power, thermal);
        if !exited && reason.is_none() {
            continue;
        }
        match restore(&j, sw) {
            Ok(outcome) => {
                log::info!("lid guard: {outcome:?} (owner exited: {exited}, reason: {reason:?})");
                if exited {
                    journal::clear(journal_path);
                } else {
                    // Nox is alive: it reads the reason and stops the session.
                    let stopped = Journal {
                        active: false,
                        guard_reason: reason.or(Some(GuardReason::External)),
                        ..j
                    };
                    if let Err(e) = journal::store(journal_path, &stopped) {
                        log::warn!("lid guard: journal not updated: {e}");
                    }
                }
                return 0;
            }
            Err(e) => {
                attempts += 1;
                log::warn!("lid guard: restore failed ({attempts}/{MAX_RESTORE_ATTEMPTS}): {e}");
                if attempts >= MAX_RESTORE_ATTEMPTS {
                    return 1;
                }
                // The owner may already be gone, in which case waiting on it
                // returns at once: pace the retries here.
                std::thread::sleep(period);
            }
        }
    }
}

/// Entry point of `nox --lid-guard <owner pid>`.
pub fn run_guard(owner: i32) -> i32 {
    watch(owner, &journal::default_path(), &Pmset, &sensors, PERIOD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmset::fake::FakeSwitch;

    fn active(owner: i32) -> Journal {
        Journal::starting(owner, 0.0, false, 20)
    }

    fn battery(on_battery: bool, percent: u8) -> Option<PowerSourceInfo> {
        Some(PowerSourceInfo {
            on_battery,
            battery_percent: Some(percent),
        })
    }

    #[test]
    fn battery_rules_respect_the_floor_and_ac_only() {
        let mut j = active(1);
        j.min_battery = 0;
        // Floor of 5 % even when the threshold is off.
        assert_eq!(
            guard_reason(&j, battery(true, 5), None),
            Some(GuardReason::Battery)
        );
        assert_eq!(guard_reason(&j, battery(true, 6), None), None);
        j.min_battery = 20;
        assert_eq!(
            guard_reason(&j, battery(true, 20), None),
            Some(GuardReason::Battery)
        );
        assert_eq!(
            guard_reason(&j, battery(false, 1), None),
            None,
            "on AC never trips"
        );
        j.only_on_ac = true;
        assert_eq!(
            guard_reason(&j, battery(true, 90), None),
            Some(GuardReason::Battery)
        );
        assert_eq!(
            guard_reason(&j, None, None),
            None,
            "unknown source never trips"
        );
    }

    #[test]
    fn thermal_heavy_trips_and_moderate_does_not() {
        let j = active(1);
        assert_eq!(guard_reason(&j, None, Some(1)), None);
        assert_eq!(guard_reason(&j, None, Some(2)), Some(GuardReason::Thermal));
        assert_eq!(guard_reason(&j, None, Some(4)), Some(GuardReason::Thermal));
    }

    #[test]
    fn restore_sets_zero_only_when_nox_set_one() {
        let sw = FakeSwitch::new(true);
        assert_eq!(restore(&active(1), &sw), Ok(RestoreOutcome::Restored));
        assert_eq!(
            sw.with(|s| (s.value, s.writes.clone())),
            (false, vec![false])
        );
    }

    #[test]
    fn restore_never_overwrites_a_user_change() {
        let sw = FakeSwitch::new(false);
        assert_eq!(restore(&active(1), &sw), Ok(RestoreOutcome::Untouched));
        assert!(sw.with(|s| s.writes.is_empty()));
    }

    #[test]
    fn restore_is_a_noop_when_not_active() {
        let sw = FakeSwitch::new(true);
        let mut j = active(1);
        j.active = false;
        j.initial = true;
        assert_eq!(restore(&j, &sw), Ok(RestoreOutcome::Untouched));
        assert!(
            sw.with(|s| s.writes.is_empty()),
            "a value Nox did not set is left alone"
        );
    }

    /// Unreadable value: 0 is written, but the confirmation read fails, so the
    /// caller keeps the journal and retries.
    #[test]
    fn unreadable_value_writes_zero_and_reports_the_failed_confirmation() {
        let sw = FakeSwitch::new(true);
        sw.with(|s| s.unreadable = true);
        assert!(restore(&active(1), &sw).is_err());
        assert_eq!(sw.with(|s| s.writes.clone()), vec![false]);
    }

    #[test]
    fn refused_write_is_an_error_and_keeps_the_switch() {
        let sw = FakeSwitch::new(true);
        sw.with(|s| s.refuse = true);
        assert!(restore(&active(1), &sw).is_err());
        assert!(sw.with(|s| s.value));
    }

    /// A stand-in for Nox: a `sleep` child we can kill like a crash.
    fn spawn_owner() -> std::process::Child {
        std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap()
    }

    fn tmp_journal(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nox-guard-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("lid-state.json")
    }

    /// The guard restores lid sleep when its owner dies, without any root
    /// process and without the app running.
    #[test]
    fn guard_restores_when_the_owner_dies() {
        let path = tmp_journal("dies");
        let sw = FakeSwitch::new(false);
        let mut owner = spawn_owner();
        let pid = owner.id() as i32;
        journal::store(&path, &active(pid)).unwrap();
        sw.with(|s| s.value = true);

        let (tx, rx) = std::sync::mpsc::channel();
        let path2 = path.clone();
        let sw2 = sw.clone();
        let t = std::thread::spawn(move || {
            let code = watch(
                pid,
                &path2,
                &sw2,
                &|| (None, None),
                Duration::from_millis(20),
            );
            tx.send(code).unwrap();
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            rx.try_recv().is_err(),
            "guard must wait while the owner lives"
        );

        owner.kill().unwrap();
        let _ = owner.wait();
        let code = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("guard exits");
        t.join().unwrap();
        assert_eq!(code, 0);
        assert!(!sw.with(|s| s.value));
        assert_eq!(journal::load(&path), None, "journal cleared after restore");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Battery trips while Nox is alive: the guard restores and records why,
    /// so Nox can end the session and tell the user.
    #[test]
    fn guard_restores_on_battery_while_owner_lives_and_records_reason() {
        let path = tmp_journal("battery");
        let sw = FakeSwitch::new(true);
        let mut owner = spawn_owner();
        let pid = owner.id() as i32;
        journal::store(&path, &active(pid)).unwrap();
        let code = watch(
            pid,
            &path,
            &sw,
            &|| (battery(true, 4), None),
            Duration::from_millis(10),
        );
        let _ = owner.kill();
        let _ = owner.wait();
        assert_eq!(code, 0);
        assert!(!sw.with(|s| s.value));
        let j = journal::load(&path).unwrap();
        assert!(!j.active);
        assert_eq!(j.guard_reason, Some(GuardReason::Battery));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn guard_exits_at_once_without_a_journal() {
        let path = tmp_journal("none");
        journal::clear(&path);
        let sw = FakeSwitch::new(true);
        assert_eq!(
            watch(
                std::process::id() as i32,
                &path,
                &sw,
                &|| (None, None),
                Duration::from_millis(10)
            ),
            0
        );
        assert!(sw.with(|s| s.writes.is_empty()));
    }

    #[test]
    fn wait_for_exit_reports_a_dead_pid_and_a_timeout() {
        let mut child = spawn_owner();
        let pid = child.id() as i32;
        assert_eq!(wait_for_exit(pid, Duration::from_millis(10)), Wait::Timeout);
        child.kill().unwrap();
        let _ = child.wait();
        assert_eq!(wait_for_exit(pid, Duration::from_millis(10)), Wait::Exited);
    }
}
