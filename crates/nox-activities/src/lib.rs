//! AI agent tracking, window-region watch, timers and companion.
//! Owned by Agent Activités. Public contract: [`modules`].

use nox_core::Module;

pub mod agents;
pub mod clock;
pub mod companion;
pub mod sound;
pub mod timers;
pub mod watch;

/// Agents, Watch, Timers and Companion; each `stop()` ends all its work.
pub fn modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(agents::AgentsModule::new()),
        Box::new(watch::WatchModule::new()),
        Box::new(timers::TimersModule::new()),
        Box::new(companion::CompanionModule::new()),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn exposes_the_four_modules() {
        use nox_core::ModuleId::*;
        let ids: Vec<_> = super::modules().iter().map(|m| m.id()).collect();
        assert_eq!(ids, vec![Agents, Watch, Timers, Companion]);
    }
}
