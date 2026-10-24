//! Clipboard history (E10). Memory only: never written to disk or logs, wiped
//! on stop and on `Clear`. Full contents stay inside the module; only short
//! previews are published.

use super::Timestamp;
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
    /// Most recent first.
    pub items: Vec<ClipEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipboardCommand {
    Restore(u64),
    Remove(u64),
    Clear,
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
