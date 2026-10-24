use super::Timestamp;
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

/// One-shot animation played once when `id` changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompanionReactionKind {
    /// A task finished / timer rang.
    Celebrate,
    /// Something changed (watch triggered, agent needs you).
    Perk,
    /// A source went away (watch lost).
    Confused,
    /// Back from inactivity.
    WakeUp,
    /// Falling asleep.
    Yawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CompanionReaction {
    pub kind: CompanionReactionKind,
    pub id: u64,
    pub at: Timestamp,
}

/// What caused the mood (lets the UI pick an accessory: note, laptop, battery…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MoodCause {
    #[default]
    Nothing,
    Music,
    Agent,
    AgentFinished,
    AgentWaiting,
    Timer,
    Focus,
    Battery,
    Inactivity,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompanionState {
    pub mood: CompanionMood,
    /// Short caption ("Claude travaille…", "Batterie faible").
    pub caption: Option<String>,
    /// 0..1: how strongly to animate (number of busy agents, battery level…).
    #[serde(default)]
    pub intensity: f64,
    #[serde(default)]
    pub cause: MoodCause,
    /// When the current mood began.
    #[serde(default)]
    pub since: Timestamp,
    /// Dance tempo when known; the UI picks a default otherwise.
    #[serde(default)]
    pub tempo_bpm: Option<f64>,
    #[serde(default)]
    pub reaction: Option<CompanionReaction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompanionSide {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionSettings {
    pub enabled_in_compact: bool,
    /// "cat", "ghost", "blob".
    pub character: String,
    /// "pearl", "lemon", "peach", "rose", "lilac", "sky", "mint".
    pub palette: String,
    pub side: CompanionSide,
    /// Play one-shot reactions to events.
    pub reactions: bool,
    /// No keyboard/mouse input for this long → sleepy.
    pub sleepy_after_secs: u64,
}

impl Default for CompanionSettings {
    fn default() -> Self {
        Self {
            enabled_in_compact: true,
            character: "blob".into(),
            palette: "pearl".into(),
            side: CompanionSide::Left,
            reactions: true,
            sleepy_after_secs: 300,
        }
    }
}
