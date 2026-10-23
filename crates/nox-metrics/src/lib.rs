//! System monitor collectors (CPU, GPU, sensors, memory, battery, power, network, disk).
//! Owned by Agent Mesures. Public contract: [`module`].

use nox_core::Module;

/// The metrics module (single shared sampler feeding `IslandState::metrics`).
pub fn module() -> Box<dyn Module> {
    todo_module::Placeholder::boxed(nox_core::ModuleId::Metrics)
}

mod todo_module;
