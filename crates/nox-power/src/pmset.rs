//! `SleepDisabled`: read from the IORegistry, written through the sudoers
//! rule (`sudo -n /usr/bin/pmset -a disablesleep 1|0`). Nox never sees or
//! stores a password: `-n` makes sudo fail instead of prompting.

use crate::probe;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const SUDO: &str = "/usr/bin/sudo";
pub const PMSET: &str = "/usr/bin/pmset";
const SAFE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The `sudo` argument list. The sudoers rule grants `PMSET -a disablesleep
/// 1|0` (see `rule.rs`), and sudo matches those arguments exactly.
pub fn sudo_args(on: bool) -> [&'static str; 5] {
    [
        "-n",
        PMSET,
        "-a",
        "disablesleep",
        if on { "1" } else { "0" },
    ]
}

/// Reading and writing `SleepDisabled`. Trait so the state machine is tested
/// with a fake.
pub trait SleepSwitch: Send {
    fn read(&self) -> Result<bool, String>;
    /// Requests the change; does not confirm it.
    fn write(&self, on: bool) -> Result<(), String>;
}

/// The real switch: IORegistry for reads, `sudo -n pmset` for writes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pmset;

impl SleepSwitch for Pmset {
    fn read(&self) -> Result<bool, String> {
        probe::sleep_disabled()
    }

    fn write(&self, on: bool) -> Result<(), String> {
        let out = Command::new(SUDO)
            .args(sudo_args(on))
            .env_clear()
            .env("PATH", SAFE_PATH)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("sudo : {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            // `sudo -n` never prompts: a failure here means the rule is missing
            // or does not match, or pmset itself refused.
            Err(format!(
                "sudo -n pmset a échoué (règle absente ou inactive ?) : {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// Writes `on` and waits (up to 3 s) until the registry shows it.
pub fn set_confirmed(sw: &dyn SleepSwitch, on: bool) -> Result<(), String> {
    sw.write(on)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if sw.read()? == on {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "pmset accepté mais SleepDisabled ne vaut pas {} après 3 s",
                u8::from(on)
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// In-memory `SleepDisabled` for the state-machine tests.
#[cfg(test)]
pub mod fake {
    use super::SleepSwitch;
    use std::sync::{Arc, Mutex};

    /// Fake `SleepDisabled`: records writes, can refuse them.
    #[derive(Clone, Default)]
    pub struct FakeSwitch(pub Arc<Mutex<SwState>>);
    #[derive(Default)]
    pub struct SwState {
        pub value: bool,
        pub writes: Vec<bool>,
        pub refuse: bool,
        pub unreadable: bool,
    }
    impl FakeSwitch {
        pub fn new(value: bool) -> Self {
            Self(Arc::new(Mutex::new(SwState {
                value,
                ..Default::default()
            })))
        }
        pub fn with<R>(&self, f: impl FnOnce(&mut SwState) -> R) -> R {
            f(&mut self.0.lock().unwrap())
        }
    }
    impl SleepSwitch for FakeSwitch {
        fn read(&self) -> Result<bool, String> {
            let s = self.with(|s| (s.value, s.unreadable));
            if s.1 {
                Err("unreadable".into())
            } else {
                Ok(s.0)
            }
        }
        fn write(&self, on: bool) -> Result<(), String> {
            self.with(|s| {
                s.writes.push(on);
                if s.refuse {
                    return Err("sudo -n refused".into());
                }
                s.value = on;
                Ok(())
            })
        }
    }
}
