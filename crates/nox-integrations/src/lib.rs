//! Music, calendar, notifications, camera, accessories, downloads, files, shortcuts.
//! Owned by Agent Intégrations. Public contract: [`modules`].

use nox_core::Module;

pub mod accessories;
pub mod calendar;
pub mod camera;
pub mod downloads;
pub mod files;
pub mod music;
pub mod notifications;
pub mod shortcuts;
pub mod sys;
pub mod util;

pub fn modules() -> Vec<Box<dyn Module>> {
    let v: Vec<Box<dyn Module>> = vec![
        Box::new(music::MusicModule::default()),
        Box::new(calendar::CalendarModule::default()),
        Box::new(camera::CameraModule::default()),
        Box::new(downloads::DownloadsModule::default()),
        Box::new(files::FilesModule::default()),
        Box::new(shortcuts::ShortcutsModule::default()),
        Box::new(accessories::AccessoriesModule::default()),
        Box::new(notifications::NotificationsModule::default()),
    ];
    v
}

#[cfg(test)]
mod tests {
    use nox_core::ModuleId;

    #[test]
    fn one_real_module_per_id() {
        let mut ids: Vec<ModuleId> = super::modules().iter().map(|m| m.id()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            n,
            "duplicate module ids would shadow each other in the manager"
        );
        assert_eq!(
            ids,
            vec![
                ModuleId::Music,
                ModuleId::Calendar,
                ModuleId::Notifications,
                ModuleId::Accessories,
                ModuleId::Downloads,
                ModuleId::Files,
                ModuleId::Shortcuts,
                ModuleId::Camera,
            ]
        );
    }
}
