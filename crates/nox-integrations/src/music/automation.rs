//! Shuffle / repeat / upcoming tracks through AppleScript for players with a
//! scripting dictionary (Music, Spotify). Scripts only run while the player
//! is already running (`tell application id` would otherwise launch it), and
//! every Apple Event is bounded by `with timeout`.

use crate::sys::applescript::{self, ScriptError};
use crate::sys::workspace;
use nox_core::model::music::{QueueItem, RepeatMode};

pub const MAX_QUEUE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    Music,
    Spotify,
}

impl Player {
    pub fn from_bundle(id: &str) -> Option<Self> {
        match id {
            "com.apple.Music" => Some(Self::Music),
            "com.spotify.client" => Some(Self::Spotify),
            _ => None,
        }
    }
    pub fn bundle_id(self) -> &'static str {
        match self {
            Self::Music => "com.apple.Music",
            Self::Spotify => "com.spotify.client",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlayerState {
    pub shuffle: Option<bool>,
    pub repeat: Option<RepeatMode>,
}

fn wrap(player: Player, body: &str) -> String {
    format!(
        "with timeout of 3 seconds\ntell application id \"{}\"\n{body}\nend tell\nend timeout",
        player.bundle_id()
    )
}

fn run(player: Player, body: &str) -> Result<Option<String>, ScriptError> {
    if !workspace::is_running(player.bundle_id()) {
        return Err(ScriptError::AppNotRunning);
    }
    applescript::run(&wrap(player, body))
}

pub fn parse_state(player: Player, out: &str) -> PlayerState {
    let mut it = out.trim().split('|').map(|s| s.trim().to_ascii_lowercase());
    let shuffle = match it.next().as_deref() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let repeat = match (player, it.next().as_deref()) {
        (Player::Music, Some("off")) => Some(RepeatMode::Off),
        (Player::Music, Some("one")) => Some(RepeatMode::One),
        (Player::Music, Some("all")) => Some(RepeatMode::All),
        (Player::Spotify, Some("true")) => Some(RepeatMode::All),
        (Player::Spotify, Some("false")) => Some(RepeatMode::Off),
        _ => None,
    };
    PlayerState { shuffle, repeat }
}

pub fn read_state(player: Player) -> Result<PlayerState, ScriptError> {
    let body = match player {
        Player::Music => "return ((shuffle enabled) as text) & \"|\" & ((song repeat) as text)",
        Player::Spotify => "return ((shuffling) as text) & \"|\" & ((repeating) as text)",
    };
    Ok(parse_state(player, &run(player, body)?.unwrap_or_default()))
}

pub fn set_shuffle(player: Player, on: bool) -> Result<(), ScriptError> {
    let body = match player {
        Player::Music => format!("set shuffle enabled to {on}"),
        Player::Spotify => format!("set shuffling to {on}"),
    };
    run(player, &body).map(|_| ())
}

pub fn set_repeat(player: Player, mode: RepeatMode) -> Result<(), ScriptError> {
    let body = match (player, mode) {
        (Player::Music, RepeatMode::Off) => "set song repeat to off".to_owned(),
        (Player::Music, RepeatMode::One) => "set song repeat to one".to_owned(),
        (Player::Music, RepeatMode::All) => "set song repeat to all".to_owned(),
        (Player::Spotify, RepeatMode::Off) => "set repeating to false".to_owned(),
        (Player::Spotify, RepeatMode::All) => "set repeating to true".to_owned(),
        (Player::Spotify, RepeatMode::One) => {
            return Err(ScriptError::Other(
                0,
                "Spotify: répétition d’un titre non scriptable".into(),
            ));
        }
    };
    run(player, &body).map(|_| ())
}

/// Tracks after the current one in Music's current playlist. This is the
/// playlist order, not Music's "Up Next" (not scriptable): callers mark it
/// as estimated. `Ok(None)` when shuffle makes the order meaningless.
pub fn music_queue() -> Result<Option<Vec<QueueItem>>, ScriptError> {
    let body = format!(
        "if shuffle enabled then return \"SHUFFLE\"\n\
         set out to \"\"\n\
         set p to current playlist\n\
         set i to index of current track\n\
         set n to count of tracks of p\n\
         set k to i + 1\n\
         repeat while k ≤ n and k ≤ i + {MAX_QUEUE}\n\
         set tr to track k of p\n\
         set out to out & (name of tr) & tab & (artist of tr) & tab & ((duration of tr) as text) & linefeed\n\
         set k to k + 1\n\
         end repeat\n\
         return out"
    );
    let out = run(Player::Music, &body)?.unwrap_or_default();
    if out.trim() == "SHUFFLE" {
        return Ok(None);
    }
    Ok(Some(parse_queue(&out)))
}

pub fn parse_queue(out: &str) -> Vec<QueueItem> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let title = f.next()?.trim().to_owned();
            if title.is_empty() {
                return None;
            }
            let artist = f.next().unwrap_or("").trim().to_owned();
            // `duration as text` follows the user's locale ("201,5" in French).
            let duration = f
                .next()
                .and_then(|d| d.trim().replace(',', ".").parse::<f64>().ok())
                .filter(|d| d.is_finite() && *d > 0.0);
            Some(QueueItem {
                title,
                artist,
                duration,
            })
        })
        .take(MAX_QUEUE)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_player_state() {
        assert_eq!(
            parse_state(Player::Music, "true|one"),
            PlayerState {
                shuffle: Some(true),
                repeat: Some(RepeatMode::One)
            }
        );
        assert_eq!(
            parse_state(Player::Spotify, "false|true"),
            PlayerState {
                shuffle: Some(false),
                repeat: Some(RepeatMode::All)
            }
        );
        assert_eq!(parse_state(Player::Music, ""), PlayerState::default());
    }

    #[test]
    fn parses_queue_with_locale_decimals() {
        let q = parse_queue("Alpha\tSynth Band\t201,5\nBeta\t\t\n\t\t\nGamma\tX\tabc\n");
        assert_eq!(q.len(), 3);
        assert_eq!(q[0].duration, Some(201.5));
        assert_eq!(q[1].artist, "");
        assert_eq!(q[2].duration, None);
    }

    #[test]
    fn scripts_are_bounded() {
        let s = wrap(Player::Music, "return 1");
        assert!(s.starts_with("with timeout of 3 seconds"));
        assert!(s.contains("application id \"com.apple.Music\""));
    }
}
