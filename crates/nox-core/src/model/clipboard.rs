//! Clipboard history (E10). Memory only: never written to disk or logs, wiped
//! on stop and on `Clear`. Full contents stay inside the module; only short
//! previews are published.

use super::Timestamp;
use crate::permissions::PermissionState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKind {
    Text,
    Url,
    Files(usize),
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipEntry {
    pub id: u64,
    pub kind: ClipKind,
    /// At most 200 characters.
    pub preview: String,
    pub source_app: Option<String>,
    pub copied_at: Timestamp,
    pub byte_len: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClipboardState {
    /// macOS paste privacy (`NSPasteboard.accessBehavior`, 15.4+):
    /// `Granted` = "Always allow" — the only state in which copies are
    /// recorded (anything else would show the system alert on every copy);
    /// `NotDetermined` = never asked; `Denied` = "Ask" or "Always deny".
    pub access: PermissionState,
    /// Most recent first.
    pub items: Vec<ClipEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipboardCommand {
    Restore(u64),
    Remove(u64),
    Clear,
    /// One read of the clipboard on a user action, so macOS shows its
    /// paste-access alert once and lists Nox in Privacy settings (where the
    /// user can choose "Always allow").
    RequestAccess,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardSettings {
    /// Capped at 100 by the module.
    pub max_items: usize,
    pub max_bytes_per_item: usize,
    /// Bundle ids whose copies are never recorded.
    pub excluded_apps: Vec<String>,
    pub capture_images: bool,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            max_items: 20,
            max_bytes_per_item: 1 << 20,
            excluded_apps: vec![
                "com.apple.keychainaccess".into(),
                "com.1password.1password".into(),
                "com.agilebits.onepassword7".into(),
            ],
            capture_images: false,
        }
    }
}
