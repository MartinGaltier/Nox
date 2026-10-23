//! Commands sent by the UI (or menu bar) to modules.

use crate::model::{
    agents::AgentCommand, calendar::CalendarCommand, camera::CameraCommand,
    downloads::DownloadCommand, files::FileCommand, music::MusicCommand,
    notifications::NotificationCommand, power::PowerCommand, shortcuts::ShortcutCommand,
    timers::TimerCommand, watch::WatchCommand,
};
use crate::module::ModuleId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Music(MusicCommand),
    Calendar(CalendarCommand),
    Notifications(NotificationCommand),
    Downloads(DownloadCommand),
    Files(FileCommand),
    Shortcuts(ShortcutCommand),
    Camera(CameraCommand),
    Agents(AgentCommand),
    Watch(WatchCommand),
    Timers(TimerCommand),
    Power(PowerCommand),
    /// The UI tells metrics whether something on screen displays them, so the
    /// collector can switch between foreground and background intervals.
    MetricsVisible(bool),
}

impl Command {
    pub fn target(&self) -> ModuleId {
        match self {
            Command::Music(_) => ModuleId::Music,
            Command::Calendar(_) => ModuleId::Calendar,
            Command::Notifications(_) => ModuleId::Notifications,
            Command::Downloads(_) => ModuleId::Downloads,
            Command::Files(_) => ModuleId::Files,
            Command::Shortcuts(_) => ModuleId::Shortcuts,
            Command::Camera(_) => ModuleId::Camera,
            Command::Agents(_) => ModuleId::Agents,
            Command::Watch(_) => ModuleId::Watch,
            Command::Timers(_) => ModuleId::Timers,
            Command::Power(_) => ModuleId::Power,
            Command::MetricsVisible(_) => ModuleId::Metrics,
        }
    }
}
