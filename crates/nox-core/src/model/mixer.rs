//! Per-app volume mixer (E9) through Core Audio process taps. Opt-in: no tap
//! exists for an app at volume 1 and unmuted. Owned by Agent Mesures/Audio.

use crate::permissions::PermissionState;
use crate::reading::Unavailable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixerApp {
    pub id: String,
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: i32,
    pub playing: bool,
    /// 0..=2, 1 = unchanged.
    pub volume: f32,
    pub muted: bool,
    /// A tap is currently active for this app.
    pub engaged: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MixerState {
    pub apps: Vec<MixerApp>,
    /// Inferred (persistent silence from a tap = probably denied).
    pub permission: PermissionState,
    pub unavailable: Option<Unavailable>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MixerCommand {
    SetVolume { app: String, volume: f32 },
    SetMuted { app: String, muted: bool },
    Reset { app: String },
    ResetAll,
    Visible(bool),
}

/// Persisted per bundle id only (no bare process names on disk).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixerSettings {
    pub volumes: BTreeMap<String, f32>,
    pub muted: Vec<String>,
}
