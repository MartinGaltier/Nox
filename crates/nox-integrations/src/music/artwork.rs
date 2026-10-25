// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchBrowserArtworkSupport.swift, commit e916961.

//! Online covers when the player publishes none or a tiny one (B8). Opt-in
//! (`MusicSettings::browser_artwork`, off): the title and artist go to Apple
//! (iTunes Search) and, for Firefox-family browsers, the playing YouTube
//! video's thumbnail is fetched from i.ytimg.com. The video comes from the
//! browser's own saved session file (a plain read of its profile, no new
//! permission, no browser Automation). Disk cache hashed, capped, purged with
//! the lyrics cache.

use crate::sys::{http, media};
use crate::util::{fnv1a, home};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Covers below this on their short side blur in the island.
pub const MIN_COVER_PIXELS: u32 = 300;
const MAX_ART_BYTES: usize = 4 * 1024 * 1024;
const MAX_SESSION_FILE: u64 = 32 * 1024 * 1024;
const MAX_SESSION_DECODED: usize = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 100;
const MAX_AGE: Duration = Duration::from_secs(90 * 86_400);
/// A miss is retried after a day.
const MISS_TTL: Duration = Duration::from_secs(86_400);

/// Firefox-family browsers (no or 60-px Now Playing covers) and where their
/// profiles live under Application Support.
pub const FIREFOX_PROFILES: &[(&str, &str)] = &[
    ("org.mozilla.firefox", "Firefox/Profiles"),
    ("org.mozilla.firefoxdeveloperedition", "Firefox/Profiles"),
    ("org.mozilla.nightly", "Firefox/Profiles"),
    ("app.zen-browser.zen", "zen/Profiles"),
    ("io.gitlab.librewolf-community", "librewolf/Profiles"),
    ("net.waterfox.waterfox", "Waterfox/Profiles"),
    ("one.ablaze.floorp", "Floorp/Profiles"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub bundle: String,
    pub title: String,
    pub artist: String,
}

impl Track {
    fn key(&self) -> u64 {
        fnv1a(
            format!(
                "{}\n{}\n{}",
                self.bundle,
                fold(&self.artist),
                fold(&self.title)
            )
            .as_bytes(),
        )
    }
}

pub fn cache_dir() -> PathBuf {
    nox_core::paths::cache_dir().join("artwork")
}

/// Whether a published cover is missing or too small to look sharp.
pub fn needs_cover(art: Option<&[u8]>) -> bool {
    match art.and_then(media::image_size) {
        Some((w, h)) => w.min(h) < MIN_COVER_PIXELS,
        None => true,
    }
}

/// Case-, diacritic- and width-insensitive form for matching.
pub fn fold(s: &str) -> String {
    media::fold(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `name` stands as whole words in `credit` ("keshi" in "keshi & Jay",
/// but not "mia" in "jeremias").
pub fn holds_name(name: &str, credit: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    credit.match_indices(name).any(|(i, _)| {
        let before = credit[..i].chars().next_back();
        let after = credit[i + name.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

pub fn itunes_url(title: &str, artist: &str, country: &str) -> Option<String> {
    if title.trim().is_empty() || artist.trim().is_empty() {
        return None;
    }
    let country = if country.len() == 2 && country.chars().all(|c| c.is_ascii_alphabetic()) {
        country.to_ascii_uppercase()
    } else {
        "US".into()
    };
    Some(format!(
        "https://itunes.apple.com/search?term={}&media=music&entity=song&limit=10&country={country}",
        http::encode_component(&format!("{artist} {title}"))
    ))
}

/// The 600-px cover of the result whose title and artist both match. A
/// near miss (live version, cover by someone else) is no match. Only
/// Apple's image servers are accepted.
pub fn itunes_match(json: &[u8], title: &str, artist: &str) -> Option<String> {
    let v: Value = serde_json::from_slice(json).ok()?;
    let (t, a) = (fold(title), fold(artist));
    v.get("results")?.as_array()?.iter().find_map(|r| {
        let name = fold(r.get("trackName")?.as_str()?);
        let credit = fold(r.get("artistName")?.as_str()?);
        if name != t || !(holds_name(&a, &credit) || holds_name(&credit, &a)) {
            return None;
        }
        let small = r.get("artworkUrl100")?.as_str()?;
        let cover = format!("{}600x600bb.jpg", small.strip_suffix("100x100bb.jpg")?);
        let host = cover
            .strip_prefix("https://")?
            .split('/')
            .next()?
            .to_ascii_lowercase();
        host.ends_with(".mzstatic.com").then_some(cover)
    })
}

const YOUTUBE_HOSTS: &[&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "youtu.be",
];

/// The 11-character video id of a YouTube page URL.
pub fn youtube_id(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path_q) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    if !YOUTUBE_HOSTS.contains(&host.as_str()) {
        return None;
    }
    let (path, query) = path_q.split_once('?').unwrap_or((path_q, ""));
    let path = path.split('#').next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let id = if host == "youtu.be" {
        parts.first().map(|s| s.to_string())
    } else if parts.first() == Some(&"watch") {
        query
            .split(['&', '#'])
            .find_map(|kv| kv.strip_prefix("v="))
            .map(str::to_owned)
    } else if parts.len() >= 2 && ["shorts", "live", "embed"].contains(&parts[0]) {
        Some(parts[1].to_owned())
    } else {
        None
    }?;
    (id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(id)
}

/// Full-size thumbnail first (missing for some old videos), then medium;
/// both 16:9 without letterboxing.
pub fn thumbnail_urls(id: &str) -> [String; 2] {
    [
        format!("https://i.ytimg.com/vi/{id}/maxresdefault.jpg"),
        format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"),
    ]
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub url: String,
    pub title: String,
}

/// `mozLz40\0` + u32 LE decoded size + raw LZ4 block.
pub fn decode_mozlz4(bytes: &[u8]) -> Option<Vec<u8>> {
    let body = bytes.strip_prefix(b"mozLz40\0")?;
    let size = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
    if size == 0 || size > MAX_SESSION_DECODED {
        return None;
    }
    media::lz4_decode(&body[4..], size)
}

/// The page each open tab shows now, YouTube tabs only.
pub fn youtube_tabs(session_json: &[u8]) -> Vec<Tab> {
    let Ok(v) = serde_json::from_slice::<Value>(session_json) else {
        return vec![];
    };
    let mut out = vec![];
    for w in v
        .get("windows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for t in w
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(entries) = t
                .get("entries")
                .and_then(Value::as_array)
                .filter(|e| !e.is_empty())
            else {
                continue;
            };
            let index = t
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(entries.len() as u64);
            let e = &entries[(index.max(1) as usize - 1).min(entries.len() - 1)];
            let (Some(url), Some(title)) = (
                e.get("url").and_then(Value::as_str),
                e.get("title").and_then(Value::as_str),
            ) else {
                continue;
            };
            if youtube_id(url).is_some() {
                out.push(Tab {
                    url: url.to_owned(),
                    title: title.to_owned(),
                });
            }
        }
    }
    out
}

const SITE_SUFFIXES: &[&str] = &[
    " - youtube music",
    " | youtube music",
    " - youtube",
    " | youtube",
];

/// The tab that plays this track: its page title (without the site suffix
/// and the "(3) " unread prefix) is the track title, or names both title
/// and artist.
pub fn tab_for<'a>(tabs: &'a [Tab], title: &str, artist: &str) -> Option<&'a Tab> {
    let (t, a) = (fold(title), fold(artist));
    if t.is_empty() {
        return None;
    }
    tabs.iter().find(|tab| {
        let mut page = fold(&tab.title);
        if let Some(rest) = page.strip_prefix('(')
            && let Some((n, after)) = rest.split_once(") ")
            && n.trim_end_matches('+').chars().all(|c| c.is_ascii_digit())
        {
            page = after.to_owned();
        }
        if let Some(s) = SITE_SUFFIXES.iter().find(|s| page.ends_with(*s)) {
            page.truncate(page.len() - s.len());
        }
        page == t || (holds_name(&t, &page) && !a.is_empty() && holds_name(&a, &page))
    })
}

/// The newest saved session file of a Firefox-family browser.
pub fn session_file(bundle: &str) -> Option<PathBuf> {
    let (_, folder) = FIREFOX_PROFILES.iter().find(|(b, _)| *b == bundle)?;
    let root = home().join("Library/Application Support").join(folder);
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .flat_map(|p| {
            [
                "sessionstore-backups/recovery.jsonlz4",
                "sessionstore.jsonlz4",
            ]
            .map(|f| p.path().join(f))
        })
        .filter_map(|f| {
            let m = std::fs::metadata(&f).ok()?;
            (m.is_file() && m.len() <= MAX_SESSION_FILE)
                .then_some(())
                .and_then(|_| Some((m.modified().ok()?, f)))
        })
        .max_by_key(|(t, _)| *t)
        .map(|(_, f)| f)
}

fn download_image(url: &str) -> Option<Vec<u8>> {
    let ua = format!(
        "Nox/{} (https://github.com/MartinGaltier/Nox)",
        env!("CARGO_PKG_VERSION")
    );
    let r = http::get(
        url,
        &[("User-Agent", &ua)],
        Duration::from_secs(10),
        MAX_ART_BYTES,
    )
    .ok()?;
    (r.status == 200 && media::image_size(&r.body).is_some()).then_some(r.body)
}

fn youtube_cover(track: &Track) -> Option<Vec<u8>> {
    let file = session_file(&track.bundle)?;
    let raw = std::fs::read(file).ok()?;
    let json = decode_mozlz4(&raw)?;
    let tabs = youtube_tabs(&json);
    let tab = tab_for(&tabs, &track.title, &track.artist)?;
    let id = youtube_id(&tab.url)?;
    thumbnail_urls(&id).iter().find_map(|u| download_image(u))
}

fn itunes_cover(track: &Track) -> Option<Vec<u8>> {
    let url = itunes_url(&track.title, &track.artist, &media::region())?;
    let ua = format!(
        "Nox/{} (https://github.com/MartinGaltier/Nox)",
        env!("CARGO_PKG_VERSION")
    );
    let r = http::get(
        &url,
        &[("User-Agent", &ua), ("Accept", "application/json")],
        Duration::from_secs(10),
        1 << 20,
    )
    .ok()?;
    if r.status != 200 {
        return None;
    }
    download_image(&itunes_match(&r.body, &track.title, &track.artist)?)
}

fn prune(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (i, (t, p)) in files.into_iter().enumerate() {
        if i >= MAX_ENTRIES || now.duration_since(t).is_ok_and(|a| a > MAX_AGE) {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub fn clear_cache(dir: &Path) {
    if dir.ends_with("artwork") {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Cache, then the browser's YouTube tab, then Apple's catalog. Blocking
/// (network). `None` = no reliable cover (never a guessed one).
pub fn lookup(track: &Track, dir: &Path) -> Option<Vec<u8>> {
    let key = track.key();
    let hit = dir.join(format!("{key:016x}.img"));
    let miss = dir.join(format!("{key:016x}.none"));
    if let Ok(b) = std::fs::read(&hit) {
        return Some(b);
    }
    if std::fs::metadata(&miss)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|a| a < MISS_TTL))
    {
        return None;
    }
    let found = FIREFOX_PROFILES
        .iter()
        .any(|(b, _)| *b == track.bundle)
        .then(|| youtube_cover(track))
        .flatten()
        .or_else(|| itunes_cover(track));
    let _ = match &found {
        Some(b) => nox_core::paths::write_atomic(&hit, b),
        None => nox_core::paths::write_atomic(&miss, b""),
    };
    prune(dir);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itunes_strict_matching() {
        let json = r#"{"results":[
          {"trackName":"Synthetic Song (Piano Version)","artistName":"Nobody","artworkUrl100":"https://is1-ssl.mzstatic.com/a/100x100bb.jpg"},
          {"trackName":"Synthétic   Song","artistName":"Nobody & Friend","artworkUrl100":"https://is1-ssl.mzstatic.com/b/100x100bb.jpg"},
          {"trackName":"Synthetic Song","artistName":"Nobody","artworkUrl100":"https://evil.example/c/100x100bb.jpg"}]}"#
            .as_bytes();
        assert_eq!(
            itunes_match(json, "synthetic song", "Nobody").as_deref(),
            Some("https://is1-ssl.mzstatic.com/b/600x600bb.jpg"),
            "folded title, whole-word artist, Apple host only"
        );
        assert_eq!(
            itunes_match(json, "synthetic song", "Nob"),
            None,
            "partial name is no match"
        );
        assert!(itunes_url("T", "", "FR").is_none(), "no artist, no lookup");
        assert!(itunes_url("T", "A", "fr").unwrap().ends_with("&country=FR"));
    }

    #[test]
    fn youtube_ids_and_tabs() {
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            youtube_id("https://youtu.be/dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            youtube_id("https://music.youtube.com/watch?list=x&v=AAAAAAAAAAA").as_deref(),
            Some("AAAAAAAAAAA")
        );
        assert_eq!(
            youtube_id("https://www.youtube.com/shorts/BBBBBBBBBBB").as_deref(),
            Some("BBBBBBBBBBB")
        );
        assert_eq!(
            youtube_id("https://youtube.com.evil.example/watch?v=dQw4w9WgXcQ"),
            None
        );
        assert_eq!(youtube_id("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(
            thumbnail_urls("dQw4w9WgXcQ")[0],
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg"
        );
        let tabs = vec![
            Tab {
                url: "https://www.youtube.com/watch?v=AAAAAAAAAAA".into(),
                title: "(3) Other Video - YouTube".into(),
            },
            Tab {
                url: "https://music.youtube.com/watch?v=BBBBBBBBBBB".into(),
                title: "(12) Synthetic Song - YouTube Music".into(),
            },
        ];
        assert_eq!(
            tab_for(&tabs, "Synthetic Song", "Nobody").unwrap().url,
            tabs[1].url
        );
        let t2 = vec![Tab {
            url: tabs[0].url.clone(),
            title: "Nobody - Synthetic Song (Official Video) - YouTube".into(),
        }];
        assert!(
            tab_for(&t2, "Synthetic Song", "Nobody").is_some(),
            "names title and artist"
        );
        assert!(tab_for(&t2, "Synthetic Song", "Somebody").is_none());
    }

    #[test]
    fn mozlz4_session_roundtrip() {
        let json = br#"{"windows":[{"tabs":[
            {"index":2,"entries":[{"url":"https://example.org","title":"x"},{"url":"https://www.youtube.com/watch?v=AAAAAAAAAAA","title":"Song A - YouTube"}]},
            {"entries":[{"url":"https://example.org/b","title":"not youtube"}]}]}]}"#;
        let mut file = b"mozLz40\0".to_vec();
        file.extend_from_slice(&(json.len() as u32).to_le_bytes());
        file.extend(media::lz4_encode(json).unwrap());
        let decoded = decode_mozlz4(&file).unwrap();
        assert_eq!(decoded, json.to_vec());
        let tabs = youtube_tabs(&decoded);
        assert_eq!(tabs.len(), 1, "current entry of each tab, YouTube only");
        assert_eq!(tabs[0].title, "Song A - YouTube");
        assert!(decode_mozlz4(b"notmozlz4").is_none());
    }

    #[test]
    fn cover_size_and_cache_misses() {
        assert!(needs_cover(None));
        assert!(needs_cover(Some(b"not an image")));
        let dir = std::env::temp_dir()
            .join(format!("nox-art-{}", std::process::id()))
            .join("artwork");
        std::fs::create_dir_all(&dir).unwrap();
        let t = Track {
            bundle: "x".into(),
            title: "Synthetic".into(),
            artist: "Nobody".into(),
        };
        let key = t.key();
        std::fs::write(dir.join(format!("{key:016x}.none")), b"").unwrap();
        assert_eq!(
            lookup(&t, &dir),
            None,
            "a recent miss is not retried (no network)"
        );
        std::fs::write(dir.join(format!("{key:016x}.img")), b"cached").unwrap();
        assert_eq!(lookup(&t, &dir).as_deref(), Some(&b"cached"[..]));
        clear_cache(&dir);
        assert!(!dir.exists());
    }

    #[test]
    #[ignore = "network: fetches a public YouTube thumbnail from i.ytimg.com"]
    fn live_youtube_thumbnail() {
        let b = thumbnail_urls("dQw4w9WgXcQ")
            .iter()
            .find_map(|u| download_image(u))
            .expect("thumbnail");
        let (w, h) = media::image_size(&b).unwrap();
        eprintln!("thumbnail {w}x{h}");
        assert!(w.min(h) >= MIN_COVER_PIXELS);
    }

    #[test]
    #[ignore = "network: queries itunes.apple.com for a public track"]
    fn live_itunes_cover() {
        let t = Track {
            bundle: "com.apple.Safari".into(),
            title: "One More Time".into(),
            artist: "Daft Punk".into(),
        };
        let dir = std::env::temp_dir()
            .join(format!("nox-art-live-{}", std::process::id()))
            .join("artwork");
        let b = lookup(&t, &dir).expect("cover");
        let (w, h) = media::image_size(&b).unwrap();
        eprintln!("cover {w}x{h}, {} bytes", b.len());
        assert!(w.min(h) >= MIN_COVER_PIXELS);
        clear_cache(&dir);
    }
}
