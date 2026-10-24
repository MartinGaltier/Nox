//! The helper's only side effects, behind a trait so the lease/restore logic
//! is tested with a fake.

use crate::sys::PowerSourceInfo;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Thermal pressure level at which the helper gives lid sleep back
/// (`kOSThermalPressureLevelHeavy`).
pub const THERMAL_HEAVY: u64 = 2;

pub trait SleepBackend: Send {
    /// Current `SleepDisabled`.
    fn sleep_disabled(&mut self) -> Result<bool, String>;
    /// Sets `SleepDisabled` (`pmset -a disablesleep 1|0`).
    fn set_sleep_disabled(&mut self, on: bool) -> Result<(), String>;
    /// `None` = unknown (guards do nothing on unknown).
    fn power_source(&mut self) -> Option<PowerSourceInfo>;
    /// `None` = unknown.
    fn thermal_pressure(&mut self) -> Option<u64>;
    /// After lid sleep is re-enabled: if the lid is already closed (and the
    /// kernel would sleep on lid close), request that sleep now — macOS only
    /// evaluates the lid when it moves, so a Mac closed during the session
    /// would otherwise stay awake in the bag.
    fn sleep_if_lid_closed(&mut self);
}

/// Real implementation: IOKit reads + `/usr/bin/pmset` (absolute path, fixed
/// arguments, empty environment). Needs root for writes.
#[derive(Default)]
pub struct RealBackend;

const PMSET_TIMEOUT: Duration = Duration::from_secs(15);
/// powerd forwards the new value to the kernel asynchronously after pmset
/// returns; wait this long for the registry to reflect it.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

impl SleepBackend for RealBackend {
    fn sleep_disabled(&mut self) -> Result<bool, String> {
        crate::sys::sleep_disabled()
    }

    fn set_sleep_disabled(&mut self, on: bool) -> Result<(), String> {
        use std::process::{Command, Stdio};
        let mut child = Command::new(crate::PMSET)
            .args(["-a", "disablesleep", if on { "1" } else { "0" }])
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn pmset: {e}"))?;
        let deadline = Instant::now() + PMSET_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("pmset timed out".into());
                }
                Err(e) => return Err(format!("wait pmset: {e}")),
            }
        };
        if !status.success() {
            return Err(format!("pmset exited with {status}"));
        }
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            match crate::sys::sleep_disabled() {
                Ok(v) if v == on => return Ok(()),
                _ if Instant::now() >= deadline => {
                    return Err("pmset succeeded but SleepDisabled did not change".into());
                }
                _ => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    fn power_source(&mut self) -> Option<PowerSourceInfo> {
        crate::sys::power_source()
    }

    fn thermal_pressure(&mut self) -> Option<u64> {
        crate::sys::thermal_pressure()
    }

    fn sleep_if_lid_closed(&mut self) {
        // Bounded retry: the kernel refuses sleep until powerd has handed it
        // the cleared flag.
        for _ in 0..10 {
            if crate::sys::clamshell_closed() != Some(true)
                || crate::sys::clamshell_causes_sleep() != Some(true)
            {
                return;
            }
            match crate::sys::request_system_sleep() {
                Ok(()) => {
                    crate::log_line("lid closed after restore: sleep requested");
                    return;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        crate::log_line("lid closed after restore: sleep request refused");
    }
}

/// Observable fake for tests (shared state, cloneable handle).
#[derive(Debug, Clone)]
pub struct FakeState {
    pub sleep_disabled: Option<bool>,
    pub fail_writes: bool,
    pub writes: Vec<bool>,
    pub power: Option<PowerSourceInfo>,
    pub thermal: Option<u64>,
    pub sleep_requests: u32,
    pub lid_closed: bool,
    /// When set, each enabling write records whether this file existed at
    /// that moment (checks "journal before pmset").
    pub journal_probe: Option<std::path::PathBuf>,
    pub journal_present_at_enable: Vec<bool>,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            sleep_disabled: Some(false),
            fail_writes: false,
            writes: Vec::new(),
            power: Some(PowerSourceInfo {
                on_battery: false,
                battery_percent: Some(80),
            }),
            thermal: Some(0),
            sleep_requests: 0,
            lid_closed: false,
            journal_probe: None,
            journal_present_at_enable: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct FakeBackend(pub Arc<Mutex<FakeState>>);

impl FakeBackend {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with<R>(&self, f: impl FnOnce(&mut FakeState) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl SleepBackend for FakeBackend {
    fn sleep_disabled(&mut self) -> Result<bool, String> {
        self.with(|s| s.sleep_disabled.ok_or_else(|| "unreadable".to_string()))
    }
    fn set_sleep_disabled(&mut self, on: bool) -> Result<(), String> {
        self.with(|s| {
            if s.fail_writes {
                return Err("pmset exited with 1".into());
            }
            if on && let Some(p) = &s.journal_probe {
                let present = p.exists();
                s.journal_present_at_enable.push(present);
            }
            s.writes.push(on);
            s.sleep_disabled = Some(on);
            Ok(())
        })
    }
    fn power_source(&mut self) -> Option<PowerSourceInfo> {
        self.with(|s| s.power)
    }
    fn thermal_pressure(&mut self) -> Option<u64> {
        self.with(|s| s.thermal)
    }
    fn sleep_if_lid_closed(&mut self) {
        self.with(|s| {
            if s.lid_closed {
                s.sleep_requests += 1;
            }
        })
    }
}
