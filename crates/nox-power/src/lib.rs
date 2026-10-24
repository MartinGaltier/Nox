//! Keep-awake modes, power assertions and closed-lid helper client.
//! Owned by Agent Énergie. Public contract: [`module`], [`startup_recovery`].
//!
//! - `System` / `SystemAndDisplay`: IOPM assertions held by this process
//!   (released by the kernel if the process dies — nothing to recover).
//! - `ClosedLid`: system assertion + a lease on the root helper
//!   (`nox-helper`), which sets `pmset disablesleep 1` and restores it by
//!   itself when the lease ends (see docs/SLEEP_RECOVERY.md).

pub mod assertions;
pub mod engine;
pub mod install;
pub mod lid;
mod module;
mod recovery;
mod sys;

use nox_core::Module;

pub fn module() -> Box<dyn Module> {
    Box::new(module::PowerModule::new())
}

/// Called once at app launch, before modules start: if the previous run
/// ended while holding a closed-lid lease (crash, kill -9, force quit), asks
/// the helper to restore lid sleep now rather than after the grace period,
/// and keeps a notice for the UI. Bounded: a few seconds at most, and only in
/// that case.
pub fn startup_recovery() {
    recovery::startup_recovery();
}
