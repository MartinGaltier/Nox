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

struct Watcher {
    mr: MediaRemote,
    last_artwork: Mutex<Option<u64>>,
    generation: AtomicU64,
    reader: dispatch2::DispatchRetained<DispatchQueue>,
}

impl Watcher {
    fn read_and_emit(&self) {
        let reply = match self.mr.info(READ_TIMEOUT) {
            None => json!({"type": "error", "message": "metadata timeout"}),
            Some(info) => match info.filter(|i| !protocol::is_empty(i)) {
                None => {
                    *self.last_artwork.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    json!({"type": "empty"})
                }
                Some(info) => {
                    let client = self.mr.client(SIDE_TIMEOUT);
                    let pid = self.mr.pid(SIDE_TIMEOUT);
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

    fn handle(&'static self, req: Request) {
        use sys::command::*;
        let ok = match req {
            Request::Refresh => {
                self.schedule_refresh(Duration::ZERO);
                return;
            }
            Request::Play => self.mr.send(PLAY) || self.mr.send(TOGGLE_PLAY_PAUSE),
            Request::Pause => self.mr.send(PAUSE) || self.mr.send(TOGGLE_PLAY_PAUSE),
            Request::Toggle => self.mr.send(TOGGLE_PLAY_PAUSE),
            Request::Next => self.mr.send(NEXT_TRACK),
            Request::Previous => self.mr.send(PREVIOUS_TRACK),
            Request::Seek(p) => self.mr.seek(p),
        };
        emit(&json!({"type": "sent", "cmd": req.name(), "ok": ok}));
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
    let w = Watcher {
        mr,
        last_artwork: Mutex::new(None),
        generation: AtomicU64::new(0),
        reader: DispatchQueue::new("io.github.martingaltier.nox.now-playing.reader", None),
    };
    w.read_and_emit();
}

/// Long-lived: streams changes until stdin closes. Never returns.
#[unsafe(no_mangle)]
pub extern "C" fn nox_now_playing_watch() {
    let Some(mr) = load() else { return };
    let notifications = mr.register_for_notifications();
    let w: &'static Watcher = WATCHER.get_or_init(|| Watcher {
        mr,
        last_artwork: Mutex::new(None),
        generation: AtomicU64::new(0),
        reader: DispatchQueue::new("io.github.martingaltier.nox.now-playing.reader", None),
    });
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
