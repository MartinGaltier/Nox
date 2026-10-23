//! AI agent tracking, window-region watch, timers and companion.
//! Owned by Agent Activités. Public contract: [`modules`].

use nox_core::{Module, ModuleId};

pub mod clock;
pub mod sound;
pub mod timers;
mod wake;

pub fn modules() -> Vec<Box<dyn Module>> {
    let mut v: Vec<Box<dyn Module>> = vec![Box::new(timers::TimersModule::new())];
    v.extend(
        [ModuleId::Agents, ModuleId::Watch, ModuleId::Companion]
            .into_iter()
            .map(todo_module::Placeholder::boxed),
    );
    v
}

mod todo_module;
