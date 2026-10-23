//! Music, calendar, notifications, camera, accessories, downloads, files, shortcuts.
//! Owned by Agent Intégrations. Public contract: [`modules`].

use nox_core::{Module, ModuleId};

pub fn modules() -> Vec<Box<dyn Module>> {
    [
        ModuleId::Music,
        ModuleId::Calendar,
        ModuleId::Notifications,
        ModuleId::Accessories,
        ModuleId::Downloads,
        ModuleId::Files,
        ModuleId::Shortcuts,
        ModuleId::Camera,
    ]
    .into_iter()
    .map(todo_module::Placeholder::boxed)
    .collect()
}

mod todo_module;
