use super::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    InProgress,
    Finished,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: String,
    pub file_name: String,
    pub path: String,
    /// 0..1 when the total is known.
    pub progress: Option<f64>,
    pub bytes: u64,
    pub total: Option<u64>,
    /// Bytes per second (measured over the last samples).
    pub rate: Option<f64>,
    pub status: DownloadStatus,
    /// e.g. "Safari", "Chrome", "Firefox", "Fichier partiel".
    pub source: String,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadsState {
    pub items: Vec<DownloadItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DownloadCommand {
    Reveal(String),
    Open(String),
    Dismiss(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    /// Folders to watch; empty = ~/Downloads.
    pub folders: Vec<String>,
    pub keep_finished_secs: u64,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            folders: vec![],
            keep_finished_secs: 600,
        }
    }
}
