//! Closed-lid control, client side: the sudoers rule, `SleepDisabled`, the
//! journal and the guard process. Runs as the user; no root process and no
//! open connection. See docs/SLEEP_RECOVERY.md.

use crate::guard::{self, GuardReason, RestoreOutcome};
use crate::journal::{self, Journal};
use crate::pmset::{Pmset, SleepSwitch, set_confirmed};
use crate::rule;
use nox_core::Reading;
use nox_core::model::now;
use nox_core::model::power::LidRuleStatus;
use nox_core::reading::Unavailable;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// How often the engine checks the journal while a closed-lid session runs.
pub const POLL_SECS: f64 = 10.0;

/// Battery parameters the guard applies (copied to the journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LidPolicy {
    pub only_on_ac: bool,
    pub min_battery: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LidError {
    /// The sudoers rule is not installed (or does not match).
    NotInstalled,
    /// `sudo -n pmset` refused, or the value did not change.
    Refused(String),
    /// The journal could not be written.
    Journal(String),
    /// The guard process could not be started.
    Guard(String),
}

impl LidError {
    pub fn message_fr(&self) -> String {
        match self {
            Self::NotInstalled => {
                "Le mode capot fermé demande une autorisation : installez-la depuis Maintien éveillé."
                    .into()
            }
            Self::Refused(d) => format!("Le mode capot fermé n’a pas pu être activé : {d}"),
            Self::Journal(d) => format!("Journal de restauration non écrit : {d}"),
            Self::Guard(d) => format!("Gardien de restauration non lancé : {d}"),
        }
    }
}

pub trait LidControl: Send {
    /// Detected without privileges (`sudo -n -l`).
    fn rule_status(&mut self) -> LidRuleStatus;
    /// `SleepDisabled` read directly from the IORegistry.
    fn sleep_disabled(&mut self) -> Reading<bool>;
    /// Disables lid-close sleep for this session. `Ok(true)`: Nox set it.
    /// `Ok(false)`: it was already disabled and is left as it is.
    fn acquire(&mut self, policy: LidPolicy) -> Result<bool, LidError>;
    /// Once the guard restored lid sleep on its own (battery, thermal, or Nox
    /// vanished), the reason. Clears the journal.
    fn take_guard_stop(&mut self) -> Option<GuardReason>;
    fn release(&mut self) -> Result<RestoreOutcome, LidError>;
}

pub struct LidClient<S: SleepSwitch> {
    switch: S,
    journal: PathBuf,
    installed: Box<dyn Fn() -> bool + Send>,
    /// Executable started as `<exe> --lid-guard <pid>`. `None`: no guard (tests).
    guard_exe: Option<PathBuf>,
    /// This client set `SleepDisabled` to 1 and has not released it.
    held: bool,
    guard: Option<Child>,
}

impl LidClient<Pmset> {
    /// The real system: sudoers rule, IORegistry, the app's own executable.
    pub fn system() -> Self {
        Self::new(
            Pmset,
            journal::default_path(),
            Box::new(rule::installed),
            std::env::current_exe().ok(),
        )
    }
}

impl<S: SleepSwitch> LidClient<S> {
    pub fn new(
        switch: S,
        journal: PathBuf,
        installed: Box<dyn Fn() -> bool + Send>,
        guard_exe: Option<PathBuf>,
    ) -> Self {
        Self {
            switch,
            journal,
            installed,
            guard_exe,
            held: false,
            guard: None,
        }
    }

    fn spawn_guard(&self, owner: i32) -> Result<Option<Child>, String> {
        let Some(exe) = &self.guard_exe else {
            return Ok(None);
        };
        Command::new(exe)
            .arg("--lid-guard")
            .arg(owner.to_string())
            // Own process group: a terminal Ctrl-C or a group kill aimed at Nox
            // does not reach the guard, which must outlive a crash.
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(Some)
            .map_err(|e| e.to_string())
    }

    fn finish(&mut self) {
        self.held = false;
        if let Some(mut child) = self.guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl<S: SleepSwitch> LidControl for LidClient<S> {
    fn rule_status(&mut self) -> LidRuleStatus {
        if (self.installed)() {
            LidRuleStatus::Installed
        } else {
            LidRuleStatus::NotInstalled
        }
    }

    fn sleep_disabled(&mut self) -> Reading<bool> {
        match self.switch.read() {
            Ok(v) => Reading::Measured(v),
            Err(e) => Reading::Unavailable(Unavailable::Error(e)),
        }
    }

    fn acquire(&mut self, policy: LidPolicy) -> Result<bool, LidError> {
        if self.held {
            // Same session goes on: only the guard parameters change.
            let mut j = journal::load(&self.journal)
                .filter(|j| j.active)
                .ok_or_else(|| LidError::Journal("journal absent pendant la session".into()))?;
            j.only_on_ac = policy.only_on_ac;
            j.min_battery = policy.min_battery;
            journal::store(&self.journal, &j).map_err(|e| LidError::Journal(e.to_string()))?;
            return Ok(true);
        }
        if !(self.installed)() {
            return Err(LidError::NotInstalled);
        }
        let initial = self.switch.read().map_err(LidError::Refused)?;
        if initial {
            // Set by the user (or left over): not ours, nothing to journal.
            return Ok(false);
        }

        // Journal first, then the guard, then the change: whatever happens
        // next, a process can find out what to put back.
        let owner = std::process::id() as i32;
        let mut j = Journal::starting(owner, now(), policy.only_on_ac, policy.min_battery);
        journal::store(&self.journal, &j).map_err(|e| LidError::Journal(e.to_string()))?;
        match self.spawn_guard(owner) {
            Ok(child) => {
                j.guard_pid = child.as_ref().map_or(0, |c| c.id() as i32);
                self.guard = child;
            }
            Err(e) => {
                journal::clear(&self.journal);
                return Err(LidError::Guard(e));
            }
        }
        if let Err(e) = journal::store(&self.journal, &j) {
            self.finish();
            journal::clear(&self.journal);
            return Err(LidError::Journal(e.to_string()));
        }
        if let Err(e) = set_confirmed(&self.switch, true) {
            // Not applied: whatever the value is now, put back what was there.
            let _ = guard::restore(&j, &self.switch);
            self.finish();
            journal::clear(&self.journal);
            return Err(LidError::Refused(e));
        }
        self.held = true;
        Ok(true)
    }

    fn take_guard_stop(&mut self) -> Option<GuardReason> {
        if !self.held {
            return None;
        }
        let j = journal::load(&self.journal)?;
        if j.active {
            return None;
        }
        journal::clear(&self.journal);
        self.finish();
        Some(j.guard_reason.unwrap_or(GuardReason::External))
    }

    fn release(&mut self) -> Result<RestoreOutcome, LidError> {
        let Some(j) = journal::load(&self.journal) else {
            self.finish();
            return Ok(RestoreOutcome::Untouched);
        };
        if !j.active {
            journal::clear(&self.journal);
            self.finish();
            return Ok(RestoreOutcome::Untouched);
        }
        match guard::restore(&j, &self.switch) {
            Ok(outcome) => {
                journal::clear(&self.journal);
                self.finish();
                Ok(outcome)
            }
            Err(e) => {
                // The journal and the guard stay: the guard keeps retrying.
                self.held = false;
                Err(LidError::Refused(e))
            }
        }
    }
}
