//! Music, calendar, notifications, camera, accessories, downloads, files, shortcuts.
//! Owned by Agent Intégrations. Public contract: [`modules`].

use nox_core::{Module, ModuleId};

pub mod calendar;
pub mod camera;
pub mod music;
pub mod sys;
pub mod util;

mod todo_module;

pub fn modules() -> Vec<Box<dyn Module>> {
    let mut v: Vec<Box<dyn Module>> = vec![
        Box::new(music::MusicModule::default()),
        Box::new(calendar::CalendarModule::default()),
        Box::new(camera::CameraModule::default()),
    ];
    v.extend(
        [
            ModuleId::Notifications,
            ModuleId::Accessories,
            ModuleId::Downloads,
            ModuleId::Files,
            ModuleId::Shortcuts,
        ]
        .into_iter()
        .map(todo_module::Placeholder::boxed),
    );
    v
}
