use serde::{Deserialize, Serialize};

/// File tray ("shelf") inside the island: files dropped on the island are
/// kept by reference (paths only, no copies), in memory only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShelfItem {
    pub id: u64,
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilesState {
    pub items: Vec<ShelfItem>,
    /// A compression is running.
    pub compressing: bool,
    /// Outcome of the last action that can fail (compress, AirDrop…):
    /// `Ok(message)` / `Err(reason)`, in French for display.
    pub last_result: Option<Result<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FileCommand {
    Add(Vec<String>),
    Remove(u64),
    Clear,
    Open(u64),
    Reveal(u64),
    /// Share via AirDrop (NSSharingService).
    AirDrop(Vec<u64>),
    CopyPaths(Vec<u64>),
    /// Compress the selected items into a zip next to the first item.
    Compress(Vec<u64>),
}
