use super::Timestamp;
use serde::{Deserialize, Serialize};

/// Island notepad. Persisted on purpose (user content) in
/// `app_support_dir()/scratchpad/note.txt`, 0600, never logged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScratchpadState {
    pub text: String,
    pub saved_at: Option<Timestamp>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScratchpadCommand {
    SetText(String),
    Clear,
}
