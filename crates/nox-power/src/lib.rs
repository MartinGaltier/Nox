//! Keep-awake modes, power assertions and closed-lid support.
//! Owned by Agent Énergie. Public contract: [`module`], [`startup_recovery`],
//! [`run_lid_guard`].
//!
//! - `System` / `SystemAndDisplay`: IOPM assertions held by this process
//!   (released by the kernel if the process dies — nothing to recover).
//! - `ClosedLid`: system assertion + `pmset -a disablesleep 1` through a narrow
//!   sudoers rule installed once (see docs/SLEEP_RECOVERY.md). Restoration needs
//!   no root daemon: a journal, a guard process (`nox --lid-guard`) and a user
//!   LaunchAgent (`nox --lid-restore`).

pub mod assertions;
pub mod engine;
pub mod guard;
pub mod journal;
pub mod launch_agent;
pub mod lid;
mod module;
pub mod pmset;
pub mod probe;
mod recovery;
pub mod rule;
mod sys;

use nox_core::Module;

pub fn module() -> Box<dyn Module> {
    Box::new(module::PowerModule::new())
}

/// Called once at app launch, before modules start: if the previous run
/// ended with `SleepDisabled` set by Nox (crash, kill -9, force quit, reboot),
/// puts it back now and keeps a notice for the UI. Only does work when a
/// journal exists.
pub fn startup_recovery() {
    recovery::startup_recovery();
}

/// `nox --lid-guard <owner pid>`: waits for the owner, restores lid sleep if
/// it dies, and stops the session on battery or heavy thermal pressure.
pub fn run_lid_guard(owner: i32) -> i32 {
    guard::run_guard(owner)
}

/// `nox --lid-restore` (LaunchAgent at login): restores from the journal.
pub fn run_lid_restore() {
    recovery::startup_recovery();
}
