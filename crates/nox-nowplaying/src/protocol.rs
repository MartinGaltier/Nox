//! Line protocol between the adapter (stdout/stdin of the perl process) and
//! `nox-integrations::music::adapter`. Pure code, unit tested.
//!
//! Adapter → app, one JSON object per line:
//! * `{"type":"hello","version":1,"notifications":bool}`
//! * `{"type":"state", …}` — see [`state_reply`]; `{"type":"empty"}` when nothing plays
//! * `{"type":"sent","cmd":"next","ok":true}`
//! * `{"type":"error","message":"…"}`
//!
//! App → adapter, one JSON object per line:
//! `{"cmd":"play"|"pause"|"toggle"|"next"|"previous"|"refresh"}` or
//! `{"cmd":"seek","position":12.5}`.

use crate::sys::{Client, Info};
use serde_json::{Map, Value, json};

pub const VERSION: u32 = 1;
/// Artwork above this many bytes is dropped instead of pushed through the pipe.
pub const MAX_ARTWORK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Seek(f64),
    Refresh,
}

impl Request {
    pub fn parse(line: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(line.trim()).ok()?;
        Some(match v.get("cmd")?.as_str()? {
            "play" => Self::Play,
            "pause" => Self::Pause,
            "toggle" => Self::Toggle,
            "next" => Self::Next,
            "previous" => Self::Previous,
            "refresh" => Self::Refresh,
            "seek" => {
                let p = v.get("position")?.as_f64()?;
                if !p.is_finite() || p < 0.0 {
                    return None;
                }
                Self::Seek(p)
            }
            _ => return None,
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Pause => "pause",
            Self::Toggle => "toggle",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Seek(_) => "seek",
            Self::Refresh => "refresh",
        }
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// FNV-1a: a cheap stable identity for artwork bytes.
pub fn artwork_id(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Builds the `state` line. `previous_artwork` is the id sent last time: the
/// bytes are only re-sent when they change. `now` is Unix seconds.
pub fn state_reply(
    info: &Info,
    client: Option<&Client>,
    pid: Option<i32>,
    is_playing: Option<bool>,
    previous_artwork: Option<u64>,
    now: f64,
) -> (Value, Option<u64>) {
    let mut m = Map::new();
    m.insert("type".into(), json!("state"));
    let mut put = |k: &str, v: Value| {
        if !v.is_null() {
            m.insert(k.into(), v);
        }
    };
    put("title", json!(info.title));
    put("artist", json!(info.artist));
    put("album", json!(info.album));
    put("duration", json!(info.duration.filter(|d| *d > 0.0)));
    let rate = info.rate.unwrap_or(0.0).max(0.0);
    // The player's own state outranks a rate it has not updated yet.
    let playing = is_playing.unwrap_or(rate > 0.0);
    put("playing", json!(playing));
    put("rate", json!(if playing { rate } else { 0.0 }));
    if let Some(e) = info.elapsed {
        put("elapsed", json!(e.max(0.0)));
        put("observed_at", json!(info.timestamp.unwrap_or(now)));
    }
    put("item_id", json!(info.item_id));
    put("shuffle_mode", json!(info.shuffle_mode));
    put("repeat_mode", json!(info.repeat_mode));
    put("queue_index", json!(info.queue_index));
    put("queue_count", json!(info.queue_count));
    put("pid", json!(client.and_then(|c| c.pid).or(pid)));
    if let Some(c) = client {
        put("bundle_id", json!(c.bundle_id));
        put("parent_bundle_id", json!(c.parent_bundle_id));
        put("player_name", json!(c.display_name));
    }
    let mut sent_artwork = None;
    if let Some(bytes) = info
        .artwork
        .as_deref()
        .filter(|b| b.len() <= MAX_ARTWORK_BYTES)
    {
        let id = artwork_id(bytes);
        put("artwork_id", json!(id));
        put("artwork_mime", json!(info.artwork_mime));
        if previous_artwork != Some(id) {
            put("artwork", json!(base64(bytes)));
        }
        sent_artwork = Some(id);
    }
    (Value::Object(m), sent_artwork)
}

/// Nothing is playing (no title).
pub fn is_empty(info: &Info) -> bool {
    info.title.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn parses_requests() {
        assert_eq!(Request::parse(r#"{"cmd":"next"}"#), Some(Request::Next));
        assert_eq!(
            Request::parse(r#"{"cmd":"seek","position":12.5}"#),
            Some(Request::Seek(12.5))
        );
        assert_eq!(Request::parse(r#"{"cmd":"seek","position":-1}"#), None);
        assert_eq!(Request::parse(r#"{"cmd":"rm -rf"}"#), None);
        assert_eq!(Request::parse("garbage"), None);
    }

    fn info() -> Info {
        Info {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            duration: Some(200.0),
            elapsed: Some(10.0),
            timestamp: Some(1000.0),
            rate: Some(1.0),
            artwork: Some(vec![1, 2, 3]),
            ..Default::default()
        }
    }

    #[test]
    fn artwork_is_sent_once() {
        let (v, id) = state_reply(&info(), None, Some(42), Some(true), None, 1001.0);
        assert!(v.get("artwork").is_some());
        assert_eq!(v["pid"], 42);
        let (v2, id2) = state_reply(&info(), None, Some(42), Some(true), id, 1002.0);
        assert!(v2.get("artwork").is_none());
        assert_eq!(v2["artwork_id"], v["artwork_id"]);
        assert_eq!(id, id2);
    }

    #[test]
    fn player_state_outranks_stale_rate() {
        let (v, _) = state_reply(&info(), None, None, Some(false), None, 1001.0);
        assert_eq!(v["playing"], false);
        assert_eq!(v["rate"], 0.0);
        assert_eq!(v["observed_at"], 1000.0);
        let mut i = info();
        i.rate = None;
        i.timestamp = None;
        let (v, _) = state_reply(&i, None, None, None, None, 5.0);
        assert_eq!(v["playing"], false, "no rate and no state = paused");
        assert_eq!(v["observed_at"], 5.0);
    }
}
