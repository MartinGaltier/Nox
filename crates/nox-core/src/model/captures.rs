use super::Timestamp;
use crate::permissions::PermissionState;
use serde::{Deserialize, Serialize};

/// A recent screenshot / screen recording (paths only, nothing copied).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureItem {
    pub id: u64,
    pub path: String,
    pub name: String,
    pub created_at: Timestamp,
    pub size: Option<u64>,
    pub is_video: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapturesState {
    pub access: PermissionState,
    /// Folder macOS saves captures to (`com.apple.screencapture location`).
    pub folder: String,
    pub items: Vec<CaptureItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CaptureCommand {
    Open(u64),
    Reveal(u64),
    CopyImage(u64),
    MoveToTrash(u64),
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub max_items: usize,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self { max_items: 12 }
    }
}
