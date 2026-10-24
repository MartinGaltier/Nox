use super::Timestamp;
use crate::permissions::PermissionState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub all_day: bool,
    pub calendar: String,
    /// sRGB 0..1
    pub color: [f32; 3],
    pub location: Option<String>,
    /// The event's own URL field, if any.
    pub url: Option<String>,
    /// Video call detected in the URL, location or notes.
    pub call: Option<VideoCall>,
    pub is_reminder: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallService {
    Meet,
    Zoom,
    Teams,
    FaceTime,
    Webex,
    Other,
}

impl CallService {
    pub fn label(self) -> &'static str {
        match self {
            CallService::Meet => "Google Meet",
            CallService::Zoom => "Zoom",
            CallService::Teams => "Teams",
            CallService::FaceTime => "FaceTime",
            CallService::Webex => "Webex",
            CallService::Other => "Visio",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoCall {
    pub service: CallService,
    pub url: String,
}

/// A calendar the user can hide in settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
    pub color: [f32; 3],
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalendarState {
    pub access: PermissionState,
    /// Reminders access (`NotRequired` while `show_reminders` is off).
    pub reminders_access: PermissionState,
    /// Sorted by start time; hidden calendars are already filtered out.
    pub events: Vec<CalendarEvent>,
    /// All event calendars (for the "hidden calendars" setting).
    pub calendars: Vec<CalendarInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CalendarCommand {
    RequestAccess,
    OpenEvent(String),
    JoinCall(String),
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarSettings {
    pub days_ahead: u32,
    /// Calendars to hide, by identifier (`CalendarInfo::id`) or title.
    pub hidden_calendars: Vec<String>,
    pub show_reminders: bool,
    /// Minutes before start to raise an island alert (0 = off).
    pub alert_minutes_before: u32,
}

impl Default for CalendarSettings {
    fn default() -> Self {
        Self {
            days_ahead: 7,
            hidden_calendars: vec![],
            show_reminders: false,
            alert_minutes_before: 5,
        }
    }
}
