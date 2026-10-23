use super::Timestamp;
use crate::permissions::PermissionState;
use serde::{Deserialize, Serialize};

/// A notification observed on screen. Kept in memory only; never persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationItem {
    pub id: u64,
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
    pub body: Option<String>,
    pub received_at: Timestamp,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationsState {
    pub access: PermissionState,
    pub items: Vec<NotificationItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationCommand {
    Dismiss(u64),
    ClearAll,
    OpenApp(u64),
    RequestAccess,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub max_items: usize,
    pub hide_body: bool,
    pub excluded_apps: Vec<String>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            max_items: 20,
            hide_body: false,
            excluded_apps: vec![],
        }
    }
}
