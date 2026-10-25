// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchMusicService.swift, commit e916961.

//! Music (B1–B10): Now Playing via the perl-hosted adapter, commands,
//! shuffle/repeat/queue via AppleScript, LRCLIB lyrics, track-change alert.
//!
//! Visualiser (B7): real levels would need a Core Audio process tap and the
//! "System Audio Recording" permission (NSAudioCaptureUsageDescription), a
//! prompt that records all system audio — too intrusive for an equaliser.
//! `levels` stays empty with `levels_measured = false`; the UI animates from
//! `now_playing.playing`.

pub mod adapter;
pub mod artwork;
pub mod automation;
pub mod lrc;
pub mod lyrics;

use crate::sys::applescript::ScriptError;
use crate::sys::{applescript, workspace};
use adapter::{Adapter, Event, Line, StateLine};
use automation::Player;
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::music::{
    Lyrics, MusicCommand, MusicSettings, MusicState, NowPlaying, PlayerInfo, QueueItem, RepeatMode,
};
use nox_core::permissions::PermissionState;
use nox_core::{
    Alert, AlertKind, AlertPriority, CancelToken, Hub, Module, ModuleContext, ModuleError,
    ModuleId, Reading, Settings, Unavailable, Worker,
};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Bounded restarts: the adapter is never relaunched in a loop.
const MAX_RESTARTS: u32 = 3;
/// Absolute cap per module start, even if each crash follows a healthy run.
const MAX_TOTAL_RESTARTS: u32 = 10;
const HEALTHY_RESET: Duration = Duration::from_secs(300);
/// Players publish "nothing" briefly between tracks; wait before clearing.
const EMPTY_GRACE: Duration = Duration::from_millis(1500);

const MUSIC_APPS: &[&str] = &[
    "com.apple.Music",
    "com.apple.iTunes",
    "com.spotify.client",
    "com.apple.podcasts",
    "com.tidal.desktop",
    "com.deezer.deezer-desktop",
    "com.amazon.music",
    "com.qobuz.QobuzDesktop",
];

pub fn is_music_app(bundle: &str) -> bool {
    MUSIC_APPS.contains(&bundle)
}

enum Msg {
    Command(MusicCommand),
    Settings(Arc<Settings>),
}

#[derive(Default)]
pub struct MusicModule {
    worker: Option<Worker>,
    tx: Option<Sender<Msg>>,
}

impl Module for MusicModule {
    fn id(&self) -> ModuleId {
        ModuleId::Music
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        let settings = ctx.settings.music.clone();
        hub.update(|s| s.music = Some(initial_state(&settings)));
        self.worker = Some(Worker::spawn("music", move |token| {
            Engine::new(hub, settings).run(rx, token)
        }));
        self.tx = Some(tx);
        Ok(())
    }

    fn stop(&mut self) {
        self.tx.take();
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn command(&mut self, cmd: nox_core::Command) {
        if let (nox_core::Command::Music(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.clone()));
        }
    }
}

fn initial_state(settings: &MusicSettings) -> MusicState {
    MusicState {
        unavailable: Some(Unavailable::Pending),
        lyrics: if settings.lyrics {
            Reading::pending()
        } else {
            Reading::Unavailable(Unavailable::Disabled)
        },
        queue: if settings.queue {
            Reading::pending()
        } else {
            Reading::Unavailable(Unavailable::Disabled)
        },
        ..Default::default()
    }
}

/// Requests for the side worker (AppleScript / Workspace calls block on the
/// main thread; LRCLIB blocks on the network). Results come back tagged
/// with the track generation they were asked for.
enum Job {
    Lyrics(u64, lyrics::Track, bool),
    PlayerState(u64, Player),
    Queue(u64, Player),
    Permission(u64, String, bool),
    SetShuffle(u64, Player, bool),
    SetRepeat(u64, Player, RepeatMode),
    Open(Option<i32>, Option<String>),
    Transport(u64, Player, String, Option<f64>),
    Artwork(u64, artwork::Track),
}

enum JobResult {
    Artwork(u64, Option<Vec<u8>>),
    Lyrics(u64, Reading<Lyrics>),
    PlayerState(u64, Result<automation::PlayerState, ScriptError>),
    Queue(u64, Reading<Vec<QueueItem>>, Option<ScriptError>),
    Permission(u64, PermissionState),
}

fn run_job(job: Job, out: &Sender<JobResult>) {
    let r = match job {
        Job::Lyrics(g, t, online) => {
            JobResult::Lyrics(g, lyrics::lookup(&t, online, &lyrics::cache_dir()))
        }
        Job::PlayerState(g, p) => JobResult::PlayerState(g, automation::read_state(p)),
        Job::Queue(g, p) => match p {
            Player::Spotify => {
                JobResult::Queue(g, Reading::Unavailable(Unavailable::NotSupported), None)
            }
            Player::Music => match automation::music_queue() {
                // Playlist order, not "Up Next": estimated.
                Ok(Some(items)) => JobResult::Queue(g, Reading::Estimated(items), None),
                Ok(None) => {
                    JobResult::Queue(g, Reading::Unavailable(Unavailable::NotSupported), None)
                }
                Err(e) => JobResult::Queue(g, Reading::Unavailable(unavailable_for(&e)), Some(e)),
            },
        },
        Job::Permission(g, bundle, ask) => {
            JobResult::Permission(g, applescript::automation_permission(&bundle, ask))
        }
        Job::SetShuffle(g, p, on) => {
            if let Err(e) = automation::set_shuffle(p, on) {
                log::info!("music: set shuffle failed: {e}");
            }
            JobResult::PlayerState(g, automation::read_state(p))
        }
        Job::SetRepeat(g, p, mode) => {
            if let Err(e) = automation::set_repeat(p, mode) {
                log::info!("music: set repeat failed: {e}");
            }
            JobResult::PlayerState(g, automation::read_state(p))
        }
        Job::Artwork(g, t) => JobResult::Artwork(g, artwork::lookup(&t, &artwork::cache_dir())),
        Job::Transport(g, p, cmd, pos) => {
            if let Err(e) = automation::transport(p, &cmd, pos) {
                log::info!("music: AppleScript {cmd} failed: {e}");
            }
            JobResult::PlayerState(g, automation::read_state(p))
        }
        Job::Open(pid, bundle) => {
            workspace::activate_app(pid, bundle.as_deref());
            return;
        }
    };
    let _ = out.send(r);
}

fn unavailable_for(e: &ScriptError) -> Unavailable {
    match e {
        ScriptError::NotAuthorized => Unavailable::PermissionDenied("Automatisation".into()),
        ScriptError::AppNotRunning => Unavailable::SourceMissing("Lecteur".into()),
        other => Unavailable::Error(other.to_string()),
    }
}

/// Identity of a track for change detection.
#[derive(Debug, Clone, PartialEq, Default)]
struct TrackId {
    bundle: String,
    title: String,
    artist: String,
    album: String,
}

struct Engine {
    hub: Hub,
    settings: MusicSettings,
    state: MusicState,
    adapter: Option<Adapter>,
    events_tx: Sender<Event>,
    events_rx: Receiver<Event>,
    restarts: u32,
    total_restarts: u32,
    launched_at: Option<Instant>,
    relaunch_at: Option<Instant>,
    empty_since: Option<Instant>,
    track: Option<TrackId>,
    /// Bumped on every track change; stale job results are dropped.
    generation: u64,
    last_line: Option<StateLine>,
    artwork_cache: ArtworkCache,
    /// Last transport command sent (for the AppleScript fallback).
    last_transport: Option<(String, Option<f64>)>,
    /// B8: when to check whether the current track needs an online cover
    /// (the player's own cover usually arrives just after the metadata).
    art_check_at: Option<Instant>,
    /// Online cover found for the current track generation.
    found_art: Option<(u64, Arc<Vec<u8>>)>,
    jobs: Option<Sender<Job>>,
    results: Receiver<JobResult>,
    results_tx: Sender<JobResult>,
    side: Option<Worker>,
}

impl Engine {
    fn new(hub: Hub, settings: MusicSettings) -> Self {
        let (events_tx, events_rx) = unbounded();
        let (results_tx, results) = unbounded();
        Self {
            hub,
            state: initial_state(&settings),
            settings,
            adapter: None,
            events_tx,
            events_rx,
            restarts: 0,
            total_restarts: 0,
            launched_at: None,
            relaunch_at: None,
            empty_since: None,
            track: None,
            generation: 0,
            last_line: None,
            artwork_cache: None,
            last_transport: None,
            art_check_at: None,
            found_art: None,
            jobs: None,
            results,
            results_tx,
            side: None,
        }
    }

    fn publish(&self) {
        let s = self.state.clone();
        self.hub.update(move |st| {
            if st.music.is_some() {
                st.music = Some(s);
            }
        });
    }

    fn launch(&mut self) {
        match adapter::locate() {
            Ok((script, lib)) => match Adapter::spawn(&script, &lib, self.events_tx.clone()) {
                Ok(a) => {
                    log::info!("music: adapter started (pid {})", a.pid());
                    self.adapter = Some(a);
                    self.launched_at = Some(Instant::now());
                    self.send_config();
                }
                Err(e) => self.fail(Unavailable::Error(format!("perl: {e}"))),
            },
            Err(e) => self.fail(Unavailable::SourceMissing(e)),
        }
    }

    /// Player selection preferences for the adapter (B9).
    fn send_config(&mut self) {
        let v = json!({
            "cmd": "config",
            "preferred": self.settings.preferred_player,
            "include_other": self.settings.include_other_players,
        });
        if let Some(a) = self.adapter.as_mut() {
            a.request(&v);
            a.request(&json!({"cmd": "refresh"}));
        }
    }

    fn fail(&mut self, why: Unavailable) {
        log::warn!("music: now playing unavailable: {why:?}");
        self.state.unavailable = Some(why);
        self.state.now_playing = None;
        self.publish();
    }

    fn submit(&mut self, job: Job) {
        if self.jobs.is_none() {
            let (tx, rx) = unbounded::<Job>();
            let out = self.results_tx.clone();
            self.side = Some(Worker::spawn("music-side", move |token: CancelToken| {
                while !token.is_cancelled() {
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(job) => run_job(job, &out),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                }
            }));
            self.jobs = Some(tx);
        }
        if let Some(tx) = &self.jobs {
            let _ = tx.send(job);
        }
    }

    fn run(mut self, rx: Receiver<Msg>, token: CancelToken) {
        self.launch();
        let tick = Duration::from_millis(250);
        while !token.is_cancelled() {
            select! {
                recv(self.events_rx) -> ev => if let Ok(ev) = ev { self.on_event(ev) },
                recv(rx) -> msg => match msg {
                    Ok(Msg::Command(c)) => self.on_command(c),
                    Ok(Msg::Settings(s)) => self.on_settings(s.music.clone()),
                    Err(_) => break,
                },
                recv(self.results) -> r => if let Ok(r) = r { self.on_result(r) },
                default(tick) => {},
            }
            self.on_tick();
        }
        if let Some(mut a) = self.adapter.take() {
            a.shutdown();
        }
        self.jobs.take();
        if let Some(mut w) = self.side.take() {
            w.stop();
        }
        self.hub.update(|s| s.music = None);
    }

    fn on_tick(&mut self) {
        if self.art_check_at.is_some_and(|t| Instant::now() >= t) {
            self.art_check_at = None;
            self.maybe_lookup_artwork();
        }
        if let Some(since) = self.empty_since
            && since.elapsed() >= EMPTY_GRACE
        {
            self.empty_since = None;
            self.clear_track();
        }
        if let Some(at) = self.relaunch_at
            && Instant::now() >= at
        {
            self.relaunch_at = None;
            self.launch();
        }
        if self.restarts > 0
            && self
                .launched_at
                .is_some_and(|t| t.elapsed() > HEALTHY_RESET)
        {
            self.restarts = 0;
        }
    }

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Exited => {
                self.adapter.take();
                self.restarts += 1;
                self.total_restarts += 1;
                if self.restarts > MAX_RESTARTS || self.total_restarts > MAX_TOTAL_RESTARTS {
                    self.fail(Unavailable::Error("adaptateur Now Playing arrêté".into()));
                } else {
                    let delay = Duration::from_secs(1 << self.restarts);
                    log::warn!(
                        "music: adapter exited, restart {} in {delay:?}",
                        self.restarts
                    );
                    self.relaunch_at = Some(Instant::now() + delay);
                }
            }
            Event::Line(Line::Hello { notifications, .. }) => {
                if !notifications {
                    log::warn!("music: MediaRemote notifications unavailable");
                }
            }
            Event::Line(Line::Empty) => {
                if self.state.unavailable.is_some() {
                    self.state.unavailable = None;
                    self.publish();
                }
                if self.state.now_playing.is_some() && self.empty_since.is_none() {
                    self.empty_since = Some(Instant::now());
                }
            }
            Event::Line(Line::State(line)) => self.on_state(*line),
            Event::Line(Line::Sent { cmd, ok, direct }) => {
                if ok {
                    return;
                }
                // MediaRemote refused a command for a followed player that is
                // not the system's current one: Music/Spotify still obey
                // their AppleScript (the user just pressed the button).
                match (direct, self.player(), self.last_transport.take()) {
                    (true, Some(p), Some((c, pos))) if c == cmd => {
                        let g = self.generation;
                        self.submit(Job::Transport(g, p, c, pos));
                    }
                    _ => log::info!("music: command {cmd} not delivered"),
                }
            }
            Event::Line(Line::Error { message }) => log::info!("music: adapter: {message}"),
        }
    }

    fn visible(&self, line: &StateLine) -> bool {
        line.title.is_some()
            && (self.settings.include_other_players
                || line.app_bundle_id().is_some_and(is_music_app))
    }

    fn on_state(&mut self, line: StateLine) {
        self.state.unavailable = None;
        if !self.visible(&line) {
            self.last_line = None;
            if self.state.now_playing.is_some() && self.empty_since.is_none() {
                self.empty_since = Some(Instant::now());
            }
            self.publish();
            return;
        }
        self.empty_since = None;
        let previous = self.state.now_playing.take();
        let np = to_now_playing(&line, previous.as_ref(), &mut self.artwork_cache);
        let id = TrackId {
            bundle: line.app_bundle_id().unwrap_or_default().to_owned(),
            title: np.title.clone(),
            artist: np.artist.clone(),
            album: np.album.clone(),
        };
        let changed = self.track.as_ref() != Some(&id);
        let playing = np.playing;
        self.state.now_playing = Some(np);
        self.state.players = line
            .sources
            .iter()
            .map(|s| PlayerInfo {
                bundle_id: s.bundle_id.clone(),
                name: s
                    .name
                    .clone()
                    .or_else(|| workspace::app_name(s.pid))
                    .unwrap_or_else(|| s.bundle_id.clone()),
            })
            .collect();
        self.last_line = Some(line);
        if changed {
            let first = self.track.is_none();
            self.track = Some(id);
            self.on_track_changed(!first && playing);
        }
        // After the track-change bookkeeping: a cover found for the previous
        // track (older generation) is never applied to this one.
        self.apply_found_art();
        self.publish();
    }

    fn player(&self) -> Option<Player> {
        self.last_line
            .as_ref()
            .and_then(|l| l.app_bundle_id())
            .and_then(Player::from_bundle)
    }

    fn maybe_lookup_artwork(&mut self) {
        let Some(np) = self.state.now_playing.as_ref() else {
            return;
        };
        if !self.settings.browser_artwork
            || !artwork::needs_cover(np.artwork.as_deref().map(|v| v.as_slice()))
        {
            return;
        }
        let t = artwork::Track {
            bundle: np
                .player
                .as_ref()
                .map(|p| p.bundle_id.clone())
                .unwrap_or_default(),
            title: np.title.clone(),
            artist: np.artist.clone(),
        };
        let g = self.generation;
        self.submit(Job::Artwork(g, t));
    }

    /// Keeps an online cover on the track it was found for, unless the
    /// player meanwhile published a sharp one.
    fn apply_found_art(&mut self) {
        let Some((g, bytes)) = self.found_art.clone() else {
            return;
        };
        if g != self.generation {
            self.found_art = None;
            return;
        }
        if let Some(np) = self.state.now_playing.as_mut()
            && artwork::needs_cover(np.artwork.as_deref().map(|v| v.as_slice()))
        {
            np.artwork_id = crate::util::fnv1a(&bytes);
            np.artwork = Some(bytes);
        }
    }

    fn on_track_changed(&mut self, alert: bool) {
        self.generation += 1;
        let g = self.generation;
        self.found_art = None;
        self.art_check_at = self
            .settings
            .browser_artwork
            .then(|| Instant::now() + Duration::from_secs(2));
        let Some(np) = self.state.now_playing.clone() else {
            return;
        };
        if alert && self.settings.track_change_alert {
            let mut a = Alert::new(AlertKind::TrackChanged, np.title.clone(), "music.note")
                .priority(AlertPriority::Low)
                .ttl(3.0);
            if !np.artist.is_empty() {
                a = a.subtitle(np.artist.clone());
            }
            self.hub.push_alert(a);
        }
        self.state.lyrics = if self.settings.lyrics {
            self.submit(Job::Lyrics(
                g,
                lyrics::Track {
                    title: np.title.clone(),
                    artist: np.artist.clone(),
                    album: np.album.clone(),
                    duration: np.duration,
                },
                self.settings.lyrics_online,
            ));
            Reading::pending()
        } else {
            Reading::Unavailable(Unavailable::Disabled)
        };
        match self.player() {
            Some(p) => {
                // Never prompts: reads the current Automation decision.
                self.submit(Job::Permission(g, p.bundle_id().into(), false));
                self.state.queue = if self.settings.queue {
                    Reading::pending()
                } else {
                    Reading::Unavailable(Unavailable::Disabled)
                };
            }
            None => {
                self.state.automation = PermissionState::NotRequired;
                self.state.queue = if self.settings.queue {
                    Reading::Unavailable(Unavailable::NotSupported)
                } else {
                    Reading::Unavailable(Unavailable::Disabled)
                };
            }
        }
    }

    fn clear_track(&mut self) {
        if self.state.now_playing.is_none() {
            return;
        }
        self.state.now_playing = None;
        self.track = None;
        self.last_line = None;
        self.generation += 1;
        self.state.lyrics = initial_state(&self.settings).lyrics;
        self.state.queue = initial_state(&self.settings).queue;
        self.publish();
    }

    /// Automation is granted: refresh shuffle/repeat (and the queue).
    fn after_permission(&mut self) {
        if let Some(p) = self.player() {
            let g = self.generation;
            self.submit(Job::PlayerState(g, p));
            if self.settings.queue {
                self.submit(Job::Queue(g, p));
            }
        }
    }

    fn on_result(&mut self, r: JobResult) {
        match r {
            JobResult::Lyrics(g, l) if g == self.generation => self.state.lyrics = l,
            JobResult::Artwork(g, Some(bytes)) if g == self.generation => {
                self.found_art = Some((g, Arc::new(bytes)));
                self.apply_found_art();
            }
            JobResult::Permission(g, p) if g == self.generation => {
                self.state.automation = p;
                match p {
                    PermissionState::Granted => self.after_permission(),
                    PermissionState::Denied if self.settings.queue => {
                        self.state.queue = Reading::Unavailable(Unavailable::PermissionDenied(
                            "Automatisation".into(),
                        ))
                    }
                    // Not asked yet: shuffle/repeat/queue wait for a user action.
                    _ if self.settings.queue => {
                        self.state.queue = Reading::Unavailable(Unavailable::PermissionDenied(
                            "Automatisation".into(),
                        ))
                    }
                    _ => {}
                }
            }
            JobResult::PlayerState(g, r) if g == self.generation => match r {
                Ok(ps) => {
                    self.state.automation = PermissionState::Granted;
                    if let Some(np) = self.state.now_playing.as_mut() {
                        np.shuffle = ps.shuffle.or(np.shuffle);
                        np.repeat = ps.repeat.or(np.repeat);
                    }
                }
                Err(ScriptError::NotAuthorized) => {
                    self.state.automation = PermissionState::Denied;
                }
                Err(e) => log::info!("music: player state: {e}"),
            },
            JobResult::Queue(g, q, err) if g == self.generation => {
                if matches!(err, Some(ScriptError::NotAuthorized)) {
                    self.state.automation = PermissionState::Denied;
                }
                self.state.queue = q;
            }
            _ => return,
        }
        self.publish();
    }

    fn request(&mut self, v: serde_json::Value) {
        if let Some(cmd) = v.get("cmd").and_then(|c| c.as_str())
            && matches!(
                cmd,
                "play" | "pause" | "toggle" | "next" | "previous" | "seek"
            )
        {
            self.last_transport =
                Some((cmd.to_owned(), v.get("position").and_then(|p| p.as_f64())));
        }
        let delivered = self.adapter.as_mut().is_some_and(|a| a.request(&v));
        if !delivered {
            log::info!("music: command dropped (adapter not running)");
        }
    }

    fn on_command(&mut self, c: MusicCommand) {
        let g = self.generation;
        match c {
            MusicCommand::PlayPause => {
                let playing = self.state.now_playing.as_ref().is_some_and(|n| n.playing);
                self.request(json!({"cmd": if playing { "pause" } else { "play" }}));
            }
            MusicCommand::Next => self.request(json!({"cmd": "next"})),
            MusicCommand::Previous => self.request(json!({"cmd": "previous"})),
            MusicCommand::Seek(p) if p.is_finite() && p >= 0.0 => {
                self.request(json!({"cmd": "seek", "position": p}))
            }
            MusicCommand::Seek(_) => {}
            // A user action: the first one may show the Automation prompt.
            MusicCommand::SetShuffle(on) => {
                if let Some(p) = self.player() {
                    self.submit(Job::SetShuffle(g, p, on));
                }
            }
            MusicCommand::SetRepeat(mode) => {
                if let Some(p) = self.player() {
                    self.submit(Job::SetRepeat(g, p, mode));
                }
            }
            MusicCommand::RequestAutomation => {
                if let Some(p) = self.player() {
                    self.submit(Job::Permission(g, p.bundle_id().into(), true));
                }
            }
            MusicCommand::ClearLyricsCache => {
                lyrics::clear_cache(&lyrics::cache_dir());
                artwork::clear_cache(&artwork::cache_dir());
            }
            // Live levels (nox-audio, B7) are wired by Agent Intégrations.
            MusicCommand::Visible(_) => {}
            MusicCommand::OpenPlayer => {
                let pid = self.last_line.as_ref().and_then(|l| l.pid);
                let bundle = self
                    .last_line
                    .as_ref()
                    .and_then(|l| l.app_bundle_id())
                    .map(str::to_owned);
                self.submit(Job::Open(pid, bundle));
            }
        }
    }

    fn on_settings(&mut self, s: MusicSettings) {
        let selection_changed = s.preferred_player != self.settings.preferred_player
            || s.include_other_players != self.settings.include_other_players;
        if self.settings.lyrics && !s.lyrics {
            lyrics::clear_cache(&lyrics::cache_dir());
        }
        if self.settings.browser_artwork && !s.browser_artwork {
            artwork::clear_cache(&artwork::cache_dir());
            self.found_art = None;
        }
        let artwork_enabled = !self.settings.browser_artwork && s.browser_artwork;
        let refetch = s.lyrics != self.settings.lyrics
            || s.lyrics_online != self.settings.lyrics_online
            || s.queue != self.settings.queue
            || s.include_other_players != self.settings.include_other_players;
        self.settings = s;
        if artwork_enabled {
            self.art_check_at = Some(Instant::now());
        }
        if selection_changed {
            self.send_config();
        }
        if refetch {
            // Re-evaluate the current track with the new settings.
            self.track = None;
            if let Some(line) = self.last_line.take() {
                self.on_state(line);
            } else {
                self.publish();
            }
        }
    }
}

/// Last artwork bytes received, by id (the adapter sends bytes only when
/// the id changes).
type ArtworkCache = Option<(u64, Arc<Vec<u8>>)>;

/// Maps an adapter line to the shared model, keeping the previous artwork
/// bytes when only their id was re-sent.
fn to_now_playing(
    line: &StateLine,
    previous: Option<&NowPlaying>,
    cache: &mut ArtworkCache,
) -> NowPlaying {
    let bundle = line.app_bundle_id().map(str::to_owned);
    let player = bundle.map(|b| PlayerInfo {
        name: line
            .player_name
            .clone()
            .or_else(|| line.pid.and_then(workspace::app_name))
            .unwrap_or_else(|| b.clone()),
        bundle_id: b,
    });
    let same_track = previous.is_some_and(|p| {
        p.title == line.title.clone().unwrap_or_default()
            && p.artist == line.artist.clone().unwrap_or_default()
    });
    let (artwork, artwork_id) = match (line.artwork_id, line.artwork.as_deref()) {
        (Some(id), Some(b64)) => match crate::util::base64_decode(b64).map(Arc::new) {
            Some(bytes) => {
                *cache = Some((id, bytes.clone()));
                (Some(bytes), id)
            }
            None => (None, 0),
        },
        (Some(id), None) => match cache.as_ref().filter(|(c, _)| *c == id) {
            Some((_, bytes)) => (Some(bytes.clone()), id),
            None => (None, 0),
        },
        // Metadata arrives before the cover: keep showing the cover of the
        // same track rather than flickering to none.
        (None, _) if same_track => (
            previous.and_then(|p| p.artwork.clone()),
            previous.map(|p| p.artwork_id).unwrap_or(0),
        ),
        (None, _) => (None, 0),
    };
    let shuffle = match line.shuffle_mode {
        Some(1) => Some(false),
        Some(2 | 3) => Some(true),
        _ => None,
    };
    let repeat = match line.repeat_mode {
        Some(1) => Some(RepeatMode::Off),
        Some(2) => Some(RepeatMode::One),
        Some(3) => Some(RepeatMode::All),
        _ => None,
    };
    let duration = line.duration.filter(|d| d.is_finite() && *d > 0.0);
    // Keep values that AppleScript already read for the same track.
    let same = previous.is_some_and(|p| {
        p.title == line.title.clone().unwrap_or_default()
            && p.artist == line.artist.clone().unwrap_or_default()
    });
    NowPlaying {
        player,
        title: line.title.clone().unwrap_or_default(),
        artist: line.artist.clone().unwrap_or_default(),
        album: line.album.clone().unwrap_or_default(),
        artwork_id,
        artwork,
        duration,
        elapsed: line.elapsed.unwrap_or(0.0),
        observed_at: line.observed_at.unwrap_or_else(nox_core::model::now),
        playing: line.playing,
        rate: line.rate,
        shuffle: shuffle.or(if same {
            previous.and_then(|p| p.shuffle)
        } else {
            None
        }),
        repeat: repeat.or(if same {
            previous.and_then(|p| p.repeat)
        } else {
            None
        }),
        can_seek: duration.is_some() && line.elapsed.is_some(),
        can_skip: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(title: &str) -> StateLine {
        StateLine {
            title: Some(title.into()),
            artist: Some("Synthetic".into()),
            duration: Some(100.0),
            elapsed: Some(10.0),
            observed_at: Some(50.0),
            playing: true,
            rate: 1.0,
            bundle_id: Some("com.spotify.client".into()),
            player_name: Some("Spotify".into()),
            artwork_id: Some(9),
            artwork: Some("Zm9v".into()),
            shuffle_mode: Some(3),
            repeat_mode: Some(2),
            ..Default::default()
        }
    }

    #[test]
    fn maps_adapter_state_and_keeps_artwork() {
        let mut cache = None;
        let np = to_now_playing(&line("A"), None, &mut cache);
        assert_eq!(
            np.artwork.as_deref().map(|v| v.as_slice()),
            Some(&b"foo"[..])
        );
        assert_eq!(np.artwork_id, 9);
        assert_eq!(np.shuffle, Some(true));
        assert_eq!(np.repeat, Some(RepeatMode::One));
        assert!((np.elapsed_now(55.0) - 15.0).abs() < 1e-9);
        // Same id without bytes: served from the cache.
        let mut l2 = line("A");
        l2.artwork = None;
        assert_eq!(
            to_now_playing(&l2, Some(&np), &mut cache).artwork,
            np.artwork
        );
        // Metadata before the cover on the same track: cover kept.
        let mut l3 = line("A");
        l3.artwork = None;
        l3.artwork_id = None;
        assert_eq!(
            to_now_playing(&l3, Some(&np), &mut cache).artwork,
            np.artwork
        );
        // New track without cover yet: none, then the same cover id is
        // restored from the cache although the adapter omits the bytes.
        let mut b = line("B");
        b.artwork = None;
        b.artwork_id = None;
        let nb = to_now_playing(&b, Some(&np), &mut cache);
        assert!(nb.artwork.is_none());
        b.artwork_id = Some(9);
        assert_eq!(
            to_now_playing(&b, Some(&nb), &mut cache).artwork,
            np.artwork
        );
        // Unknown id without bytes: nothing invented.
        b.artwork_id = Some(10);
        assert!(to_now_playing(&b, Some(&nb), &mut cache).artwork.is_none());
    }

    #[test]
    fn online_cover_only_for_its_track() {
        let hub = Hub::new();
        hub.update(|s| s.music = Some(Default::default()));
        let mut e = Engine::new(
            hub,
            MusicSettings {
                lyrics: false,
                queue: false,
                browser_artwork: true,
                ..Default::default()
            },
        );
        let mut l = line("A");
        l.artwork = None;
        l.artwork_id = None;
        e.on_state(l.clone());
        assert!(
            e.art_check_at.is_some(),
            "lookup scheduled after a track change"
        );
        let g = e.generation;
        e.on_result(JobResult::Artwork(g, Some(b"cover-A".to_vec())));
        assert_eq!(
            e.state
                .now_playing
                .as_ref()
                .unwrap()
                .artwork
                .as_deref()
                .map(|v| v.as_slice()),
            Some(&b"cover-A"[..])
        );
        // Metadata re-sent for A: the found cover stays.
        e.on_state(l.clone());
        assert!(e.state.now_playing.as_ref().unwrap().artwork.is_some());
        // Track B: A's cover must not appear, and a late result for A is dropped.
        let mut b = line("B");
        b.artwork = None;
        b.artwork_id = None;
        e.on_state(b);
        assert!(e.state.now_playing.as_ref().unwrap().artwork.is_none());
        e.on_result(JobResult::Artwork(g, Some(b"cover-A".to_vec())));
        assert!(e.state.now_playing.as_ref().unwrap().artwork.is_none());
    }

    #[test]
    fn music_app_filter() {
        assert!(is_music_app("com.apple.Music"));
        assert!(!is_music_app("com.google.Chrome"));
        let hub = Hub::new();
        let mut e = Engine::new(
            hub,
            MusicSettings {
                include_other_players: false,
                ..Default::default()
            },
        );
        let mut l = line("A");
        l.bundle_id = Some("com.google.Chrome.helper".into());
        l.parent_bundle_id = Some("com.google.Chrome".into());
        assert!(!e.visible(&l));
        e.settings.include_other_players = true;
        assert!(e.visible(&l));
    }

    #[test]
    fn track_change_bumps_generation_and_drops_stale_results() {
        let hub = Hub::new();
        hub.update(|s| s.music = Some(Default::default()));
        let mut e = Engine::new(
            hub.clone(),
            MusicSettings {
                lyrics: false,
                queue: false,
                ..Default::default()
            },
        );
        e.on_state(line("A"));
        let g = e.generation;
        e.on_state(line("A"));
        assert_eq!(e.generation, g, "same track: no change");
        e.on_state(line("B"));
        assert_eq!(e.generation, g + 1);
        assert_eq!(hub.drain_alerts().len(), 1, "one track-change alert");
        e.on_result(JobResult::Lyrics(
            g,
            Reading::Measured(Lyrics {
                synced: false,
                lines: vec![],
                source: "x".into(),
            }),
        ));
        assert_eq!(e.state.lyrics, Reading::Unavailable(Unavailable::Disabled));
        let published = hub.read(|s| s.music.as_ref().and_then(|m| m.now_playing.clone()));
        assert_eq!(published.unwrap().title, "B");
    }

    #[test]
    #[ignore = "real machine: spawns /usr/bin/perl with target/<profile>/libnox_nowplaying.dylib (cargo build -p nox-nowplaying first)"]
    fn live_adapter_starts_answers_and_stops() {
        let hub = Hub::new();
        let mut m = MusicModule::default();
        m.start(ModuleContext {
            hub: hub.clone(),
            settings: Arc::new(Settings::default()),
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && hub.read(|s| s.music.as_ref().is_some_and(|m| m.unavailable.is_some()))
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        // NOX_EXPECT_PLAYING=1: a test player is running; wait for its
        // track and its (asynchronously published) artwork.
        let expect_playing = std::env::var_os("NOX_EXPECT_PLAYING").is_some();
        if expect_playing {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline
                && hub.read(|s| {
                    s.music
                        .as_ref()
                        .is_some_and(|m| m.now_playing.as_ref().is_none_or(|n| n.artwork.is_none()))
                })
            {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let state = hub.read(|s| s.music.clone()).expect("music slot");
        eprintln!(
            "unavailable={:?} now_playing={:?}",
            state.unavailable,
            state.now_playing.as_ref().map(|n| (
                &n.title,
                &n.player,
                n.playing,
                n.duration,
                n.elapsed_now(nox_core::model::now()),
                n.artwork.as_ref().map(|a| a.len()),
            ))
        );
        assert_eq!(state.unavailable, None, "adapter answered");
        if expect_playing {
            assert!(state.now_playing.is_some(), "running player detected");
        }
        let pids = || {
            std::process::Command::new("/usr/bin/pgrep")
                // Only adapters spawned by this test process (an open Nox
                // app has its own).
                .args([
                    "-P",
                    &std::process::id().to_string(),
                    "-f",
                    "now-playing.pl",
                ])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .unwrap_or_default()
        };
        assert!(!pids().is_empty(), "adapter process running");
        m.stop();
        assert!(hub.read(|s| s.music.is_none()), "slot cleared on stop");
        assert!(pids().is_empty(), "no adapter process after stop");
    }
}
