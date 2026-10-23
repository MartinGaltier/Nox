use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompanionMood {
    #[default]
    Idle,
    Sleepy,
    Happy,
    Dancing,
    Busy,
    Worried,
    Celebrating,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompanionState {
    pub mood: CompanionMood,
    /// Short caption ("Claude travaille…", "Batterie faible").
    pub caption: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionSettings {
    pub enabled_in_compact: bool,
    /// "cat", "ghost", "blob".
    pub character: String,
}

impl Default for CompanionSettings {
    fn default() -> Self {
        Self {
            enabled_in_compact: true,
            character: "blob".into(),
        }
    }
}
