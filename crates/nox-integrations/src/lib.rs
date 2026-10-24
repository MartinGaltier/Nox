//! Music, calendar, notifications, camera, accessories, downloads, files, shortcuts.
//! Owned by Agent Intégrations. Public contract: [`modules`].

use nox_core::{Module, ModuleId};

pub mod calendar;
pub mod camera;
pub mod downloads;
pub mod files;
pub mod music;
pub mod shortcuts;
pub mod sys;
pub mod util;

mod todo_module;

pub fn modules() -> Vec<Box<dyn Module>> {
    let mut v: Vec<Box<dyn Module>> = vec![
        Box::new(music::MusicModule::default()),
        Box::new(calendar::CalendarModule::default()),
        Box::new(camera::CameraModule::default()),
        Box::new(downloads::DownloadsModule::default()),
        Box::new(files::FilesModule::default()),
        Box::new(shortcuts::ShortcutsModule::default()),
    ];
    v.extend(
        [ModuleId::Notifications, ModuleId::Accessories]
            .into_iter()
            .map(todo_module::Placeholder::boxed),
    );
    v
}
