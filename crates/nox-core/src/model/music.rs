use super::Timestamp;
use crate::reading::Reading;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub bundle_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NowPlaying {
    pub player: Option<PlayerInfo>,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Encoded image bytes (PNG/JPEG/TIFF) — decoded by the UI.
    #[serde(skip)]
    pub artwork: Option<Arc<Vec<u8>>>,
    /// Changes whenever artwork changes; lets the UI cache decoded images.
    pub artwork_id: u64,
    pub duration: Option<f64>,
    /// Elapsed seconds at `observed_at`; UI extrapolates using `rate`.
    pub elapsed: f64,
    pub observed_at: Timestamp,
    pub playing: bool,
    pub rate: f64,
    pub shuffle: Option<bool>,
    pub repeat: Option<RepeatMode>,
    pub can_seek: bool,
    pub can_skip: bool,
}

impl NowPlaying {
    pub fn elapsed_now(&self, now: Timestamp) -> f64 {
        let e = if self.playing {
            self.elapsed + (now - self.observed_at).max(0.0) * self.rate
        } else {
            self.elapsed
        };
        match self.duration {
            Some(d) if d > 0.0 => e.clamp(0.0, d),
            _ => e.max(0.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub title: String,
    pub artist: String,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    /// Start time in seconds (None for unsynced lyrics).
    pub time: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    pub synced: bool,
    pub lines: Vec<LyricLine>,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MusicState {
    pub now_playing: Option<NowPlaying>,
    pub queue: Reading<Vec<QueueItem>>,
    pub lyrics: Reading<Lyrics>,
    /// Normalised 0..1 band levels for the visualiser (empty = not available).
    pub levels: Vec<f32>,
    /// Whether `levels` comes from real audio or a playback-driven animation.
    pub levels_measured: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MusicCommand {
    PlayPause,
    Next,
    Previous,
    Seek(f64),
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    /// Bring the source player to the front.
    OpenPlayer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicSettings {
    pub lyrics: bool,
    pub queue: bool,
    pub visualizer: bool,
    /// Preferred player bundle id when several are playing.
    pub preferred_player: Option<String>,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            lyrics: true,
            queue: true,
            visualizer: true,
            preferred_player: None,
        }
    }
}
