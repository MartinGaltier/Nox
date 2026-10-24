//! AI agent tracking, window-region watch, timers and companion.
//! Owned by Agent Activités. Public contract: [`modules`].

use nox_core::{Module, ModuleId};

pub mod agents;
pub mod clock;
pub mod sound;
pub mod timers;
pub mod watch;

pub fn modules() -> Vec<Box<dyn Module>> {
    let mut v: Vec<Box<dyn Module>> = vec![
        Box::new(agents::AgentsModule::new()),
        Box::new(timers::TimersModule::new()),
        Box::new(watch::WatchModule::new()),
    ];
    v.extend(
        [ModuleId::Companion]
            .into_iter()
            .map(todo_module::Placeholder::boxed),
    );
    v
}

mod todo_module;
