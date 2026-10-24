// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/NowPlayingAdapter/NowPlayingAdapter.swift, commit e916961.

//! Now Playing adapter.
//!
//! Since macOS 15.4 MediaRemote only answers platform binaries, so the app
//! cannot read the system Now Playing session in-process. `/usr/bin/perl` is
//! a platform binary: `packaging/resources/now-playing.pl` loads this cdylib
//! with DynaLoader and calls one of the entry points below. Nothing here is
//! linked into the app.
//!
//! `nox_now_playing_watch` is the single long-lived process the music module
//! keeps while it runs: MediaRemote change notifications (no polling) produce
//! JSON lines on stdout, commands arrive as JSON lines on stdin, and closing
//! stdin ends the process. See [`protocol`].

pub mod protocol;
mod sys;

use dispatch2::DispatchQueue;
use protocol::Request;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use sys::MediaRemote;

const NOTIFICATIONS: &[&str] = &[
    "kMRMediaRemoteNowPlayingInfoDidChangeNotification",
    "kMRMediaRemoteNowPlayingApplicationDidChangeNotification",
    "kMRMediaRemoteNowPlayingApplicationIsPlayingDidChangeNotification",
    "kMRMediaRemotePlayerNowPlayingInfoDidChangeNotification",
    "kMRMediaRemoteNowPlayingPlayerStateDidChange",
    "kMRMediaRemoteNowPlayingApplicationClientStateDidChange",
    "kMRMediaRemotePlayerIsPlayingDidChangeNotification",
];

/// Coalesces bursts of notifications into one read.
const DEBOUNCE: Duration = Duration::from_millis(120);
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const SIDE_TIMEOUT: Duration = Duration::from_millis(400);

static OUT: Mutex<()> = Mutex::new(());

fn emit(v: &Value) {
    let mut line = v.to_string();
    line.push('\n');
    let _g = OUT.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    if out
        .write_all(line.as_bytes())
        .and_then(|_| out.flush())
        .is_err()
    {
        // The app went away: nothing left to serve.
        std::process::exit(0);
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The player currently followed when it is not the system's own.
struct Selected {
    player: sys::players::Player,
    item_id: Option<String>,
    is_system: bool,
}

struct Watcher {
    mr: MediaRemote,
    last_artwork: Mutex<Option<u64>>,
    generation: AtomicU64,
    reader: dispatch2::DispatchRetained<DispatchQueue>,
    config: Mutex<protocol::Config>,
    previous: Mutex<Option<i32>>,
    selected: Mutex<Option<Selected>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Watcher {
    fn new(mr: MediaRemote) -> Self {
        Self {
            mr,
            last_artwork: Mutex::new(None),
            generation: AtomicU64::new(0),
            reader: DispatchQueue::new("io.github.martingaltier.nox.now-playing.reader", None),
            config: Mutex::new(protocol::Config {
                preferred: None,
                include_other: true,
            }),
            previous: Mutex::new(None),
            selected: Mutex::new(None),
        }
    }

    /// Reads every registered player and follows the one `choose` picks.
    /// `None` = per-player API unavailable or silent: use the system player.
    fn read_players(&self, system: Option<i32>) -> Option<Value> {
        let players = self.mr.players(SIDE_TIMEOUT)?;
        let mut sources = vec![];
        let mut reads = vec![];
        for p in &players {
            let info = self.mr.player_info(p, false, SIDE_TIMEOUT);
            let playing = self
                .mr
                .player_playing(p, SIDE_TIMEOUT)
                .or_else(|| info.as_ref().and_then(|i| i.rate).map(|r| r > 0.0));
            let bundle = p
                .client
                .parent_bundle_id
                .clone()
                .or_else(|| p.client.bundle_id.clone())
                .unwrap_or_default();
            sources.push(protocol::Source {
                pid: p.client.pid.unwrap_or(0),
                bundle,
                has_track: info.as_ref().is_some_and(|i| !protocol::is_empty(i)),
                playing: playing.unwrap_or(false),
            });
            reads.push((info, playing));
        }
        if reads.iter().all(|(i, _)| i.is_none()) {
            return None;
        }
        let listed: Vec<Value> = sources
            .iter()
            .zip(&players)
            .filter(|(s, _)| s.has_track)
            .map(|(s, p)| {
                json!({"pid": s.pid, "bundle_id": s.bundle, "name": p.client.display_name,
                       "playing": s.playing, "music": s.is_music()})
            })
            .collect();
        let cfg = lock(&self.config).clone();
        let previous = *lock(&self.previous);
        let Some(i) = protocol::choose(&sources, previous, system, &cfg) else {
            *lock(&self.previous) = None;
            *lock(&self.selected) = None;
            *lock(&self.last_artwork) = None;
            return Some(json!({"type": "empty", "sources": listed}));
        };
        let playing = reads[i].1;
        let fallback = reads.swap_remove(i).0;
        let player = players.into_iter().nth(i)?;
        let info = self
            .mr
            .player_info(&player, true, READ_TIMEOUT)
            .or(fallback)?;
        let mut last = lock(&self.last_artwork);
        let (mut v, sent) = protocol::state_reply(
            &info,
            Some(&player.client),
            player.client.pid,
            playing,
            *last,
            now(),
        );
        *last = sent.or(*last);
        drop(last);
        if let Some(m) = v.as_object_mut() {
            m.insert("sources".into(), Value::Array(listed));
            m.insert("system_pid".into(), json!(system));
        }
        *lock(&self.previous) = player.client.pid;
        *lock(&self.selected) = Some(Selected {
            is_system: player.client.pid.is_some() && player.client.pid == system,
            item_id: info.item_id.clone(),
            player,
        });
        Some(v)
    }

    fn read_and_emit(&self) {
        let system = self.mr.pid(SIDE_TIMEOUT);
        if let Some(reply) = self.read_players(system) {
            emit(&reply);
            return;
        }
        *lock(&self.selected) = None;
        let reply = match self.mr.info(READ_TIMEOUT) {
            None => json!({"type": "error", "message": "metadata timeout"}),
            Some(info) => match info.filter(|i| !protocol::is_empty(i)) {
                None => {
                    *self.last_artwork.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    json!({"type": "empty"})
                }
                Some(info) => {
                    let client = self.mr.client(SIDE_TIMEOUT);
                    let pid = system;
                    let playing = self.mr.is_playing(SIDE_TIMEOUT);
                    let mut last = self.last_artwork.lock().unwrap_or_else(|e| e.into_inner());
                    let (v, sent) =
                        protocol::state_reply(&info, client.as_ref(), pid, playing, *last, now());
                    // A reading without artwork (players send metadata first,
                    // the cover a moment later) keeps the last id: the app
                    // still holds those bytes, so they are not re-sent.
                    *last = sent.or(*last);
                    v
                }
            },
        };
        emit(&reply);
    }

    fn schedule_refresh(&'static self, delay: Duration) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
        let _ = self.reader.after(when, move || {
            if self.generation.load(Ordering::SeqCst) == generation {
                self.read_and_emit();
            }
        });
    }

    /// A command for the followed player: addressed to it directly when it
    /// is not the system's current player, else to the system player.
    fn send(&self, cmd: u32, position: Option<f64>) -> bool {
        let sel = lock(&self.selected);
        if let Some(s) = sel.as_ref().filter(|s| !s.is_system) {
            return self.mr.send_to_player(
                &s.player,
                cmd,
                s.item_id.as_deref(),
                position,
                READ_TIMEOUT,
            );
        }
        drop(sel);
        match position {
            Some(p) => self.mr.seek(p),
            None => self.mr.send(cmd),
        }
    }

    fn handle(&'static self, req: Request) {
        use sys::command::*;
        let ok = match &req {
            Request::Refresh => {
                self.schedule_refresh(Duration::ZERO);
                return;
            }
            Request::Config(c) => {
                *lock(&self.config) = c.clone();
                true
            }
            Request::Play => self.send(PLAY, None) || self.send(TOGGLE_PLAY_PAUSE, None),
            Request::Pause => self.send(PAUSE, None) || self.send(TOGGLE_PLAY_PAUSE, None),
            Request::Toggle => self.send(TOGGLE_PLAY_PAUSE, None),
            Request::Next => self.send(NEXT_TRACK, None),
            Request::Previous => self.send(PREVIOUS_TRACK, None),
            Request::Seek(p) => self.send(SEEK_TO_POSITION, Some(*p)),
        };
        // `direct`: the command was addressed to a followed player that is
        // not the system's current one (third-party players often refuse
        // those; the app may then use the player's AppleScript).
        let direct = lock(&self.selected).as_ref().is_some_and(|s| !s.is_system);
        emit(&json!({"type": "sent", "cmd": req.name(), "ok": ok, "direct": direct}));
        // Players do not always notify after a seek; read back once.
        self.schedule_refresh(Duration::from_millis(250));
    }
}

static WATCHER: OnceLock<Watcher> = OnceLock::new();

fn load() -> Option<MediaRemote> {
    match MediaRemote::load() {
        Ok(mr) => Some(mr),
        Err(e) => {
            emit(&json!({"type": "error", "message": e}));
            None
        }
    }
}

/// Prints exactly one line describing the current session and returns.
#[unsafe(no_mangle)]
pub extern "C" fn nox_now_playing_get() {
    let Some(mr) = load() else { return };
    let w = Watcher::new(mr);
    w.read_and_emit();
}

/// Long-lived: streams changes until stdin closes. Never returns.
#[unsafe(no_mangle)]
pub extern "C" fn nox_now_playing_watch() {
    let Some(mr) = load() else { return };
    let notifications = mr.register_for_notifications();
    let w: &'static Watcher = WATCHER.get_or_init(|| Watcher::new(mr));
    emit(&json!({"type": "hello", "version": protocol::VERSION, "notifications": notifications}));
    let observers = sys::observe(NOTIFICATIONS, move || w.schedule_refresh(DEBOUNCE));
    std::mem::forget(observers);

    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            match Request::parse(&line) {
                Some(req) => w.reader.exec_async(move || w.handle(req)),
                None => emit(&json!({"type": "error", "message": "bad request"})),
            }
        }
        // The app closed our stdin (stopped or exited).
        std::process::exit(0);
    });

    w.schedule_refresh(Duration::ZERO);
    dispatch2::dispatch_main();
}
