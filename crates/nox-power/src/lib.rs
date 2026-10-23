//! Keep-awake modes, power assertions and closed-lid helper client.
//! Owned by Agent Énergie. Public contract: [`module`], [`startup_recovery`].

use nox_core::Module;

pub fn module() -> Box<dyn Module> {
    todo_module::Placeholder::boxed(nox_core::ModuleId::Power)
}

/// Called once at app launch, before modules start: repairs state left by a
/// previous crash / forced quit (e.g. a stale lid-sleep override).
pub fn startup_recovery() {}

mod todo_module;
