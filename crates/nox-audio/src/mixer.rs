// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Audio/AppVolumeMixer.swift (one muted private tap + private aggregate per adjusted app, apps at 100 % untouched, rows grouped by responsible app), commit e916961.
//! Per-app volume (E9), `ModuleId::Mixer`.
//!
//! An app at volume 1 and unmuted has **no tap**. For an adjusted app a
//! *muted* private tap removes its sound from the output and a private
//! aggregate on the default output re-renders the tapped stream with the
//! gain (ramped, soft-limited above 1). Fail-open:
//! * Nox exits or crashes → macOS destroys its private taps → sound returns;
//! * a tap that delivers only silence for 8 s while the app reports output
//!   (permission refused) is destroyed and the app left alone for the
//!   session (`permission = Denied`, inferred).

use crate::AudioClient;
use crate::render;
use crate::routing::{self, TapChange};
use crate::sys::hal::{self, Aggregate, IoProc, ObjectId, Tap};
use nox_core::model::mixer::{MixerApp, MixerCommand, MixerSettings, MixerState};
use nox_core::permissions::PermissionState;
use nox_core::{
    Command, Hub, Module, ModuleContext, ModuleError, ModuleId, Settings, Unavailable, Worker,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const MAX_VOLUME: f32 = 2.0;
/// A tap silent this long while its app reports output = permission refused.
pub const SILENCE_GRACE: Duration = Duration::from_secs(8);
const REFRESH_VISIBLE: Duration = Duration::from_secs(1);
const REFRESH_HIDDEN: Duration = Duration::from_secs(4);

// ---------------------------------------------------------------- planning

/// One mixer row: every audio process an app is responsible for.
#[derive(Debug, Clone, PartialEq)]
pub struct AppGroup {
    pub id: String,
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: i32,
    pub objects: BTreeSet<u32>,
    pub playing: bool,
}

/// Rolls clients up into their owning regular app. Daemons (no owning app)
/// and Nox itself are left out. Playing apps first, then by name.
pub fn group(
    clients: &[AudioClient],
    own_pid: i32,
    identity: impl Fn(i32) -> (Option<String>, Option<String>),
) -> Vec<AppGroup> {
    let mut by_pid: BTreeMap<i32, AppGroup> = BTreeMap::new();
    for c in clients {
        let Some(owner) = c.owner_pid else { continue };
        if owner == own_pid {
            continue;
        }
        let g = by_pid.entry(owner).or_insert_with(|| {
            let (name, bundle) = identity(owner);
            let bundle = bundle.or_else(|| (c.pid == owner).then(|| c.bundle_id.clone()).flatten());
            AppGroup {
                id: bundle.clone().unwrap_or_else(|| format!("pid:{owner}")),
                name: name
                    .or_else(|| c.owner_name.clone())
                    .unwrap_or_else(|| format!("pid {owner}")),
                bundle_id: bundle,
                pid: owner,
                objects: BTreeSet::new(),
                playing: false,
            }
        });
        g.objects.insert(c.object);
        g.playing |= c.running_output;
    }
    let mut v: Vec<AppGroup> = by_pid.into_values().collect();
    v.sort_by(|a, b| {
        b.playing
            .cmp(&a.playing)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v
}

/// (volume, muted) for an app: a live command wins, then the saved setting
/// (bundle id only), else unchanged.
pub fn target(
    id: &str,
    bundle_id: Option<&str>,
    settings: &MixerSettings,
    session: &HashMap<String, (f32, bool)>,
) -> (f32, bool) {
    if let Some(&(v, m)) = session.get(id) {
        return (v.clamp(0.0, MAX_VOLUME), m);
    }
    let Some(b) = bundle_id else {
        return (1.0, false);
    };
    let v = settings
        .volumes
        .get(b)
        .copied()
        .unwrap_or(1.0)
        .clamp(0.0, MAX_VOLUME);
    (v, settings.muted.iter().any(|m| m == b))
}

pub fn needs_engine(volume: f32, muted: bool) -> bool {
    muted || (volume - 1.0).abs() > 0.005
}

pub fn effective_gain(volume: f32, muted: bool) -> f32 {
    if muted {
        0.0
    } else {
        volume.clamp(0.0, MAX_VOLUME)
    }
}

// ------------------------------------------------------------------ engine

/// Shared with the real-time IOProc.
struct MixClient {
    tap_channels: u32,
    /// Target gain (f32 bits), set by the module.
    target: AtomicU32,
    /// Gain reached at the end of the last block (IOProc only).
    current: AtomicU32,
    heard: AtomicBool,
    cycles: AtomicU64,
}

unsafe extern "C-unwind" fn mix_io(
    _device: ObjectId,
    _now: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    input: NonNull<objc2_core_audio_types::AudioBufferList>,
    _input_time: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    output: NonNull<objc2_core_audio_types::AudioBufferList>,
    _output_time: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the MixClient boxed by `Engine`, alive until the
    // IoProc is stopped and destroyed; buffer lists are valid for this call.
    unsafe {
        let c = &*(client as *const MixClient);
        c.cycles.fetch_add(1, Ordering::Relaxed);
        let ins = hal::buffers(input);
        let mut shape = [(0u32, false); 16];
        let n = ins.len().min(shape.len());
        for (s, b) in shape.iter_mut().zip(ins.iter()).take(n) {
            *s = (b.mNumberChannels, !b.mData.is_null());
        }
        let tap: &[f32] = match routing::tap_buffer_index(&shape[..n], c.tap_channels) {
            Some(i) => {
                let b = &ins[i];
                std::slice::from_raw_parts(b.mData.cast::<f32>(), b.mDataByteSize as usize / 4)
            }
            None => &[],
        };
        let tap_ch = c.tap_channels as usize;
        if !c.heard.load(Ordering::Relaxed)
            && tap
                .iter()
                .any(|v| v.abs() > crate::analysis::HEARD_THRESHOLD)
        {
            c.heard.store(true, Ordering::Relaxed);
        }
        let from = f32::from_bits(c.current.load(Ordering::Relaxed));
        let to = f32::from_bits(c.target.load(Ordering::Relaxed));
        let mut first = 0usize;
        for b in hal::buffers(output) {
            let ch = b.mNumberChannels as usize;
            if b.mData.is_null() || ch == 0 {
                continue;
            }
            let out =
                std::slice::from_raw_parts_mut(b.mData.cast::<f32>(), b.mDataByteSize as usize / 4);
            render::render(tap, tap_ch, out, ch, first, from, to);
            first += ch;
        }
        c.current.store(to.to_bits(), Ordering::Relaxed);
    }
    0
}

/// Muted tap + aggregate + IOProc for one app. Field order = drop order:
/// IOProc first, then its client, the aggregate, and the tap last (the
/// app's sound returns when the tap goes).
struct Engine {
    _io: IoProc,
    client: Box<MixClient>,
    _aggregate: Aggregate,
    _tap: Tap,
    objects: BTreeSet<u32>,
    host_uid: String,
    started: Instant,
}

impl Engine {
    fn build(objects: &BTreeSet<u32>, gain: f32) -> Result<Self, String> {
        let list: Vec<ObjectId> = objects.iter().copied().collect();
        let tap = hal::create_tap(&list, true, "Nox Mixer").map_err(|s| format!("tap : {s}"))?;
        let host = hal::default_output_device().ok_or("aucune sortie par défaut")?;
        let host_uid = hal::device_uid(host).ok_or("UID de sortie introuvable")?;
        let aggregate = hal::create_aggregate("Nox Mixer", &host_uid, Some(&tap.uuid))
            .map_err(|s| format!("agrégat : {s}"))?;
        let client = Box::new(MixClient {
            tap_channels: tap.channels,
            target: AtomicU32::new(gain.to_bits()),
            current: AtomicU32::new(gain.to_bits()),
            heard: AtomicBool::new(false),
            cycles: AtomicU64::new(0),
        });
        let ptr = (&*client as *const MixClient).cast_mut().cast::<c_void>();
        // SAFETY: the boxed client is declared after `_io` in Engine, so it is
        // dropped after the IOProc stops; `mix_io` reads it as MixClient.
        let io = unsafe { IoProc::start(aggregate.id, mix_io, ptr) }
            .map_err(|s| format!("IOProc : {s}"))?;
        Ok(Self {
            _io: io,
            client,
            _aggregate: aggregate,
            _tap: tap,
            objects: objects.clone(),
            host_uid,
            started: Instant::now(),
        })
    }

    fn set_gain(&self, g: f32) {
        self.client.target.store(g.to_bits(), Ordering::Relaxed);
    }

    fn heard(&self) -> bool {
        self.client.heard.load(Ordering::Relaxed)
    }
}

// ------------------------------------------------------------------ module

enum Msg {
    Command(MixerCommand),
    Settings(MixerSettings),
}

#[derive(Default)]
pub struct MixerModule {
    worker: Option<Worker>,
    tx: Option<mpsc::Sender<Msg>>,
    hub: Option<Hub>,
}

pub fn module() -> Box<dyn Module> {
    Box::new(MixerModule::default())
}

impl Module for MixerModule {
    fn id(&self) -> ModuleId {
        ModuleId::Mixer
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        if self.worker.is_some() {
            return Ok(());
        }
        if !crate::supported() {
            ctx.hub.update(|s| {
                s.mixer = Some(MixerState {
                    unavailable: Some(Unavailable::NotSupported),
                    ..Default::default()
                })
            });
            self.hub = Some(ctx.hub);
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        let hub = ctx.hub.clone();
        let settings = ctx.settings.mixer.clone();
        hub.update(|s| s.mixer = Some(MixerState::default()));
        self.worker = Some(Worker::spawn("mixer", move |token| {
            run(&hub, settings, &rx, &token)
        }));
        self.tx = Some(tx);
        self.hub = Some(ctx.hub);
        Ok(())
    }

    fn stop(&mut self) {
        self.tx.take();
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        if let Some(h) = self.hub.take() {
            h.update(|s| s.mixer = None);
        }
    }

    fn command(&mut self, cmd: Command) {
        if let (Command::Mixer(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
            if let Some(w) = &self.worker {
                w.token().nudge();
            }
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.mixer.clone()));
            if let Some(w) = &self.worker {
                w.token().nudge();
            }
        }
    }
}

impl Drop for MixerModule {
    fn drop(&mut self) {
        self.stop();
    }
}

struct State {
    settings: MixerSettings,
    session: HashMap<String, (f32, bool)>,
    engines: HashMap<String, Engine>,
    /// Apps whose tap stayed silent (permission refused): left alone until
    /// the user changes them again.
    refused: HashSet<String>,
    errors: HashMap<String, String>,
    permission: PermissionState,
    visible: bool,
    known: Vec<String>,
}

fn apply(st: &mut State, msg: Msg) {
    match msg {
        Msg::Settings(s) => st.settings = s,
        Msg::Command(c) => match c {
            MixerCommand::SetVolume { app, volume } => {
                let muted = st.session.get(&app).map(|e| e.1).unwrap_or(false);
                st.refused.remove(&app);
                st.session
                    .insert(app, (volume.clamp(0.0, MAX_VOLUME), muted));
            }
            MixerCommand::SetMuted { app, muted } => {
                let v = st.session.get(&app).map(|e| e.0).unwrap_or(1.0);
                st.refused.remove(&app);
                st.session.insert(app, (v, muted));
            }
            MixerCommand::Reset { app } => {
                st.session.insert(app, (1.0, false));
            }
            MixerCommand::ResetAll => {
                for id in st.known.clone() {
                    st.session.insert(id, (1.0, false));
                }
            }
            MixerCommand::Visible(v) => st.visible = v,
        },
    }
}

fn run(
    hub: &Hub,
    settings: MixerSettings,
    rx: &mpsc::Receiver<Msg>,
    token: &nox_core::CancelToken,
) {
    let own = std::process::id() as i32;
    let mut st = State {
        settings,
        session: HashMap::new(),
        engines: HashMap::new(),
        refused: HashSet::new(),
        errors: HashMap::new(),
        permission: PermissionState::NotDetermined,
        visible: false,
        known: vec![],
    };
    let mut last_published: Option<Vec<MixerApp>> = None;
    loop {
        while let Ok(m) = rx.try_recv() {
            apply(&mut st, m);
        }
        let clients = crate::audio_clients();
        let groups = group(&clients, own, crate::sys::process::app_identity);
        st.known = groups.iter().map(|g| g.id.clone()).collect();
        let host_uid = hal::default_output_device().and_then(hal::device_uid);
        reconcile(&mut st, &groups, host_uid.as_deref());
        let apps: Vec<MixerApp> = groups
            .iter()
            .map(|g| {
                let (volume, muted) =
                    target(&g.id, g.bundle_id.as_deref(), &st.settings, &st.session);
                MixerApp {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    bundle_id: g.bundle_id.clone(),
                    pid: g.pid,
                    playing: g.playing,
                    volume,
                    muted,
                    engaged: st.engines.contains_key(&g.id),
                }
            })
            .collect();
        if last_published.as_ref() != Some(&apps) {
            last_published = Some(apps.clone());
            let state = MixerState {
                apps,
                permission: st.permission,
                unavailable: None,
            };
            hub.update(|s| s.mixer = Some(state));
        }
        let wait = if st.visible || !st.engines.is_empty() {
            REFRESH_VISIBLE
        } else {
            REFRESH_HIDDEN
        };
        if token.sleep(wait) {
            break;
        }
    }
    // Engines dropped here: every tap destroyed, every app audible again.
    st.engines.clear();
}

fn reconcile(st: &mut State, groups: &[AppGroup], host_uid: Option<&str>) {
    let live: HashSet<&str> = groups.iter().map(|g| g.id.as_str()).collect();
    st.engines.retain(|id, _| live.contains(id.as_str()));
    for g in groups {
        let (volume, muted) = target(&g.id, g.bundle_id.as_deref(), &st.settings, &st.session);
        let want = needs_engine(volume, muted) && !st.refused.contains(&g.id);
        let gain = effective_gain(volume, muted);
        if !want {
            st.engines.remove(&g.id);
            continue;
        }
        let rebuild = match st.engines.get(&g.id) {
            None => true,
            Some(e) => {
                let device_moved = host_uid.is_some_and(|h| h != e.host_uid);
                let objects = routing::tap_change(&e.objects, &g.objects) != TapChange::None;
                if !device_moved && !objects {
                    e.set_gain(gain);
                    // Fail-open: a tap that never hears while the app plays.
                    if g.playing && !e.heard() && e.started.elapsed() >= SILENCE_GRACE {
                        log::warn!("mixer: tap for {} stayed silent; permission refused?", g.id);
                        st.engines.remove(&g.id);
                        st.refused.insert(g.id.clone());
                        st.permission = PermissionState::Denied;
                        continue;
                    }
                    if e.heard() {
                        st.permission = PermissionState::Granted;
                    }
                    false
                } else {
                    true
                }
            }
        };
        if rebuild {
            // Drop the old engine first so two taps never hold the app.
            st.engines.remove(&g.id);
            match Engine::build(&g.objects, gain) {
                Ok(e) => {
                    st.errors.remove(&g.id);
                    st.engines.insert(g.id.clone(), e);
                }
                Err(err) => {
                    if st.errors.insert(g.id.clone(), err.clone()).as_deref() != Some(err.as_str())
                    {
                        log::warn!("mixer: {}: {err}", g.id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(object: u32, pid: i32, owner: Option<i32>, playing: bool) -> AudioClient {
        AudioClient {
            object,
            pid,
            bundle_id: None,
            owner_pid: owner,
            owner_name: None,
            running_output: playing,
        }
    }

    fn ident(pid: i32) -> (Option<String>, Option<String>) {
        match pid {
            731 => (
                Some("Google Chrome".into()),
                Some("com.google.Chrome".into()),
            ),
            44766 => (Some("Musique".into()), Some("com.apple.Music".into())),
            _ => (None, None),
        }
    }

    #[test]
    fn groups_helpers_under_their_app_and_skips_daemons_and_self() {
        let clients = vec![
            client(112, 731, Some(731), false),
            client(118, 14814, Some(731), true),
            client(123, 44766, Some(44766), false),
            client(96, 388, None, true),       // daemon
            client(200, 999, Some(999), true), // Nox itself
        ];
        let g = group(&clients, 999, ident);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].id, "com.google.Chrome", "playing first");
        assert!(g[0].playing);
        assert_eq!(g[0].objects, BTreeSet::from([112, 118]));
        assert_eq!(g[1].name, "Musique");
    }

    #[test]
    fn unknown_identity_gets_a_session_id() {
        let g = group(&[client(1, 50, Some(50), false)], 0, |_| (None, None));
        assert_eq!(g[0].id, "pid:50");
        assert_eq!(g[0].bundle_id, None);
    }

    #[test]
    fn targets_and_engines() {
        let mut s = MixerSettings::default();
        s.volumes.insert("com.apple.Music".into(), 0.5);
        s.muted.push("com.spotify.client".into());
        let mut session = HashMap::new();
        assert_eq!(
            target("com.apple.Music", Some("com.apple.Music"), &s, &session),
            (0.5, false)
        );
        assert_eq!(
            target(
                "com.spotify.client",
                Some("com.spotify.client"),
                &s,
                &session
            ),
            (1.0, true)
        );
        assert_eq!(target("pid:5", None, &s, &session), (1.0, false));
        session.insert("com.apple.Music".into(), (3.0, false));
        assert_eq!(
            target("com.apple.Music", Some("com.apple.Music"), &s, &session).0,
            MAX_VOLUME
        );
        assert!(!needs_engine(1.0, false));
        assert!(!needs_engine(1.004, false));
        assert!(needs_engine(0.8, false));
        assert!(needs_engine(1.0, true));
        assert_eq!(effective_gain(1.5, true), 0.0);
    }

    #[test]
    fn commands_update_the_session() {
        let mut st = State {
            settings: MixerSettings::default(),
            session: HashMap::new(),
            engines: HashMap::new(),
            refused: HashSet::from(["a".to_owned()]),
            errors: HashMap::new(),
            permission: PermissionState::NotDetermined,
            visible: false,
            known: vec!["a".into(), "b".into()],
        };
        apply(
            &mut st,
            Msg::Command(MixerCommand::SetVolume {
                app: "a".into(),
                volume: 0.3,
            }),
        );
        assert!(
            !st.refused.contains("a"),
            "explicit change retries a refused app"
        );
        apply(
            &mut st,
            Msg::Command(MixerCommand::SetMuted {
                app: "a".into(),
                muted: true,
            }),
        );
        assert_eq!(st.session["a"], (0.3, true));
        apply(&mut st, Msg::Command(MixerCommand::ResetAll));
        assert_eq!(st.session["b"], (1.0, false));
        apply(&mut st, Msg::Command(MixerCommand::Visible(true)));
        assert!(st.visible);
    }
}
