//! AI agent tracking, window-region watch, timers and companion.
//! Owned by Agent Activités. Public contract: [`modules`].

use nox_core::{Module, ModuleId};

pub fn modules() -> Vec<Box<dyn Module>> {
    [
        ModuleId::Agents,
        ModuleId::Watch,
        ModuleId::Timers,
        ModuleId::Companion,
    ]
    .into_iter()
    .map(todo_module::Placeholder::boxed)
    .collect()
}

mod todo_module;
