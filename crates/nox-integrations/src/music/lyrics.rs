//! Lyrics: disk cache first, then LRCLIB (`https://lrclib.net/api/get`) when
//! the user enabled online lookups. The cache is keyed by a hash of the track
//! identity and stores lyrics only (no title/artist in clear).

use super::lrc;
use crate::sys::http;
use crate::util::fnv1a;
use nox_core::model::music::Lyrics;
use nox_core::{Reading, Unavailable};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

const ENDPOINT: &str = "https://lrclib.net/api/get";
/// Not-found answers are retried after a week.
const MISSING_TTL_SECS: f64 = 7.0 * 86_400.0;
const SOURCE: &str = "LRCLIB";

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<f64>,
}

impl Track {
    pub fn key(&self) -> u64 {
        let norm = |s: &str| s.trim().to_lowercase();
        let d = self.duration.map(|d| d.round() as i64).unwrap_or(-1);
        fnv1a(
            format!(
                "{}\0{}\0{}\0{d}",
                norm(&self.title),
                norm(&self.artist),
                norm(&self.album)
            )
            .as_bytes(),
        )
    }

    pub fn url(&self) -> String {
        let mut u = format!(
            "{ENDPOINT}?artist_name={}&track_name={}",
            http::encode_component(&self.artist),
            http::encode_component(&self.title)
        );
        if !self.album.is_empty() {
            u.push_str(&format!(
                "&album_name={}",
                http::encode_component(&self.album)
            ));
        }
        if let Some(d) = self.duration.filter(|d| *d > 0.0) {
            u.push_str(&format!("&duration={}", d.round() as i64));
        }
        u
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct CacheEntry {
    synced: Option<String>,
    plain: Option<String>,
    instrumental: bool,
    missing: bool,
    fetched_at: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LrclibReply {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
    instrumental: bool,
}

fn cache_path(dir: &Path, track: &Track) -> PathBuf {
    dir.join(format!("{:016x}.json", track.key()))
}

pub fn cache_dir() -> PathBuf {
    nox_core::paths::cache_dir().join("lyrics")
}

fn to_reading(e: &CacheEntry, duration: Option<f64>) -> Reading<Lyrics> {
    if e.missing {
        return Reading::Unavailable(Unavailable::SourceMissing("Paroles".into()));
    }
    if e.instrumental {
        return Reading::Unavailable(Unavailable::SourceMissing("Paroles (instrumental)".into()));
    }
    if let Some(s) = e.synced.as_deref() {
        let lines = lrc::parse(s, duration);
        if !lines.is_empty() {
            return Reading::Measured(Lyrics {
                synced: true,
                lines,
                source: SOURCE.into(),
            });
        }
    }
    if let Some(p) = e.plain.as_deref() {
        let lines = lrc::plain(p);
        if !lines.is_empty() {
            return Reading::Measured(Lyrics {
                synced: false,
                lines,
                source: SOURCE.into(),
            });
        }
    }
    Reading::Unavailable(Unavailable::SourceMissing("Paroles".into()))
}

fn parse_reply(body: &[u8], now: f64) -> Option<CacheEntry> {
    let r: LrclibReply = serde_json::from_slice(body).ok()?;
    Some(CacheEntry {
        synced: r.synced_lyrics.filter(|s| !s.trim().is_empty()),
        plain: r.plain_lyrics.filter(|s| !s.trim().is_empty()),
        instrumental: r.instrumental,
        missing: false,
        fetched_at: now,
    })
}

fn read_cache(dir: &Path, track: &Track, now: f64) -> Option<CacheEntry> {
    let bytes = std::fs::read(cache_path(dir, track)).ok()?;
    if bytes.len() > lrc::MAX_BYTES * 2 {
        return None;
    }
    let e: CacheEntry = serde_json::from_slice(&bytes).ok()?;
    if e.missing && now - e.fetched_at > MISSING_TTL_SECS {
        return None;
    }
    Some(e)
}

fn write_cache(dir: &Path, track: &Track, e: &CacheEntry) {
    if let Ok(bytes) = serde_json::to_vec(e)
        && let Err(err) = nox_core::paths::write_atomic(&cache_path(dir, track), &bytes)
    {
        log::debug!("lyrics cache write failed: {err}");
    }
}

/// Cache, then (if `online`) LRCLIB. Blocking (network up to ~15 s).
pub fn lookup(track: &Track, online: bool, dir: &Path) -> Reading<Lyrics> {
    let now = nox_core::model::now();
    if track.title.trim().is_empty() || track.artist.trim().is_empty() {
        return Reading::Unavailable(Unavailable::SourceMissing("Paroles".into()));
    }
    if let Some(e) = read_cache(dir, track, now) {
        return to_reading(&e, track.duration);
    }
    if !online {
        return Reading::Unavailable(Unavailable::Disabled);
    }
    let ua = format!(
        "Nox/{} (https://github.com/MartinGaltier/Nox)",
        env!("CARGO_PKG_VERSION")
    );
    match http::get(
        &track.url(),
        &[("User-Agent", &ua), ("Accept", "application/json")],
        Duration::from_secs(10),
        lrc::MAX_BYTES * 2,
    ) {
        Ok(r) if r.status == 200 => match parse_reply(&r.body, now) {
            Some(e) => {
                write_cache(dir, track, &e);
                to_reading(&e, track.duration)
            }
            None => Reading::Unavailable(Unavailable::Error("réponse LRCLIB invalide".into())),
        },
        Ok(r) if r.status == 404 => {
            let e = CacheEntry {
                missing: true,
                fetched_at: now,
                ..Default::default()
            };
            write_cache(dir, track, &e);
            to_reading(&e, track.duration)
        }
        Ok(r) => Reading::Unavailable(Unavailable::Error(format!("LRCLIB HTTP {}", r.status))),
        Err(e) => Reading::Unavailable(Unavailable::Error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            title: "Synthetic Song".into(),
            artist: "Test & Artist".into(),
            album: "".into(),
            duration: Some(201.4),
        }
    }

    #[test]
    fn url_and_key() {
        let t = track();
        assert_eq!(
            t.url(),
            "https://lrclib.net/api/get?artist_name=Test%20%26%20Artist&track_name=Synthetic%20Song&duration=201"
        );
        let mut t2 = t.clone();
        t2.title = " synthetic song ".into();
        assert_eq!(t.key(), t2.key(), "key ignores case and padding");
    }

    #[test]
    fn reply_parsing_prefers_synced() {
        let e = parse_reply(
            br#"{"id":1,"syncedLyrics":"[00:01.00]Hello\n[00:02.00]World","plainLyrics":"Hello\nWorld","instrumental":false}"#,
            0.0,
        )
        .unwrap();
        let Reading::Measured(l) = to_reading(&e, Some(100.0)) else {
            panic!()
        };
        assert!(l.synced);
        assert_eq!(l.lines.len(), 2);
        let e = parse_reply(
            br#"{"syncedLyrics":null,"plainLyrics":"A\nB","instrumental":false}"#,
            0.0,
        )
        .unwrap();
        let Reading::Measured(l) = to_reading(&e, None) else {
            panic!()
        };
        assert!(!l.synced);
    }

    #[test]
    fn cache_offline_and_negative_entries() {
        let dir = std::env::temp_dir().join(format!("nox-lyrics-test-{}", std::process::id()));
        let t = track();
        assert_eq!(
            lookup(&t, false, &dir),
            Reading::Unavailable(Unavailable::Disabled),
            "offline without cache never invents lyrics"
        );
        write_cache(
            &dir,
            &t,
            &CacheEntry {
                synced: Some("[00:01.00]Cached".into()),
                fetched_at: 0.0,
                ..Default::default()
            },
        );
        assert!(
            matches!(lookup(&t, false, &dir), Reading::Measured(l) if l.lines[0].text == "Cached")
        );
        let raw = std::fs::read_to_string(cache_path(&dir, &t)).unwrap();
        assert!(!raw.contains("Synthetic"), "no title stored in clear");
        write_cache(
            &dir,
            &t,
            &CacheEntry {
                missing: true,
                fetched_at: 0.0,
                ..Default::default()
            },
        );
        assert!(
            read_cache(&dir, &t, MISSING_TTL_SECS + 1.0).is_none(),
            "negative entry expires"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore = "network: queries lrclib.net"]
    fn live_lrclib_lookup() {
        let dir = std::env::temp_dir().join(format!("nox-lyrics-live-{}", std::process::id()));
        let t = Track {
            title: "One More Time".into(),
            artist: "Daft Punk".into(),
            album: "Discovery".into(),
            duration: Some(320.0),
        };
        let r = lookup(&t, true, &dir);
        eprintln!("{r:?}");
        assert!(matches!(r, Reading::Measured(l) if !l.lines.is_empty()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
