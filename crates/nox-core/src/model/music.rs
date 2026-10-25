use super::Timestamp;
use crate::permissions::PermissionState;
use crate::reading::{Reading, Unavailable};
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
    /// `Some` when the Now Playing source itself cannot be read (adapter
    /// missing, crashed repeatedly…). `now_playing == None` with
    /// `unavailable == None` means nothing is playing.
    pub unavailable: Option<Unavailable>,
    pub now_playing: Option<NowPlaying>,
    /// Automation (Apple Events) towards the current player, used for
    /// shuffle/repeat/queue. `NotRequired` for players without a dictionary.
    pub automation: PermissionState,
    pub queue: Reading<Vec<QueueItem>>,
    pub lyrics: Reading<Lyrics>,
    /// Normalised 0..1 band levels for the visualiser (empty = not available).
    pub levels: Vec<f32>,
    /// Whether `levels` comes from real audio or a playback-driven animation.
    pub levels_measured: bool,
    /// Live levels were started but only silence arrived: the "System Audio
    /// Recording" permission is probably refused. Cleared when the user
    /// switches `live_levels` off and on again.
    #[serde(default)]
    pub levels_refused: bool,
    /// Every player currently publishing a track (the followed one included),
    /// for a source picker; `now_playing.player` is the followed one.
    pub players: Vec<PlayerInfo>,
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
    /// Ask for Automation towards the current player (shows the system
    /// prompt once). Shuffle/repeat/queue never prompt on their own.
    RequestAutomation,
    /// Delete every cached lyric (settings "purge").
    ClearLyricsCache,
    /// The music section is (not) on screen: gates live audio levels.
    Visible(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicSettings {
    pub lyrics: bool,
    /// Look lyrics up on LRCLIB (sends title/artist/album/duration). Off by default.
    pub lyrics_online: bool,
    pub queue: bool,
    pub visualizer: bool,
    /// Preferred player bundle id when several are playing.
    pub preferred_player: Option<String>,
    /// Show players that are not music apps (browsers, video players…).
    pub include_other_players: bool,
    /// Short compact alert when the track changes.
    pub track_change_alert: bool,
    /// Look up a cover for browser playback that publishes none (network).
    pub browser_artwork: bool,
    /// Real audio levels for the visualiser (Core Audio tap; asks for
    /// system audio recording permission). Off by default.
    pub live_levels: bool,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            lyrics: true,
            lyrics_online: false,
            queue: true,
            visualizer: true,
            preferred_player: None,
            include_other_players: true,
            track_change_alert: true,
            browser_artwork: false,
            live_levels: false,
        }
    }
}
