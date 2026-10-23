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
    /// Video-call / event URL if any.
    pub url: Option<String>,
    pub is_reminder: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalendarState {
    pub access: PermissionState,
    pub events: Vec<CalendarEvent>,
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
    /// Calendar titles to hide.
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
