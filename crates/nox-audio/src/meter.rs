// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchAudioLevelService.swift (NotchAudioLevelReader lifecycle: player processes, unmuted private tap, private aggregate on the default output, silence grace), commit e916961.
//! Live music levels (B7): one unmuted, private tap on the player's audio
//! processes, read through a private aggregate device on the default output.
//! The audio thread only copies samples into a lock-free ring; a meter
//! thread analyses the latest 1024 samples ~30 times a second.
//!
//! Nothing is stored or sent: the ring holds ~0.17 s of mono samples in
//! memory and is dropped with the meter.

use crate::analysis::{Analyzer, BAND_COUNT, FFT_SIZE, Ring, Smoother};
use crate::routing::{self, TapChange};
use crate::sys::hal::{self, Aggregate, IoProc, ObjectId, Tap};
use crate::sys::process;
use std::collections::{BTreeSet, HashMap};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct MeterConfig {
    pub updates_per_sec: f64,
    /// How long a tap may deliver only digital silence before the meter
    /// gives up (refused permission and a not-yet-sounding player look the
    /// same from here).
    pub silence_grace: Duration,
    /// How often the default output and the player's processes are checked.
    pub check_every: Duration,
}

impl Default for MeterConfig {
    fn default() -> Self {
        Self {
            updates_per_sec: 30.0,
            silence_grace: Duration::from_secs(8),
            check_every: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MeterEvent {
    /// Seven band levels, 0..1 (bass → treble).
    Levels([f32; BAND_COUNT]),
    /// Only silence arrived during the grace period: most likely the
    /// "System Audio Recording" permission is refused (or the player has not
    /// started sounding). The meter has stopped.
    Silent,
    /// The tap or device could not be built (or was lost for good).
    Unavailable(String),
    /// A tapped process went away (browser tab closed): the meter stopped;
    /// start a new one if the player still plays.
    ProcessesLeft,
}

/// A running meter. Dropping it stops the thread and destroys the IOProc,
/// the aggregate and the tap, in that order.
pub struct LevelMeter {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    /// Disconnected when the meter thread returns (its sender is dropped).
    done: Option<mpsc::Receiver<()>>,
}

/// Longest `stop()` waits for the thread. Building a tap can block (e.g. a
/// Bluetooth output reconnecting); quitting must never hang on it.
const STOP_WAIT: Duration = Duration::from_secs(2);

impl LevelMeter {
    /// Starts reading `pid`'s audio on a new thread and returns at once
    /// (building a tap can block while a Bluetooth device reconnects).
    ///
    /// **Creating the tap shows the system permission prompt the first
    /// time.** Call only after the user enabled live levels.
    pub fn start(
        pid: i32,
        config: MeterConfig,
        on_event: impl FnMut(MeterEvent) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("nox-audio-meter".into())
            .spawn(move || {
                let _done = done_tx;
                run(pid, config, rx, on_event)
            })?;
        Ok(Self {
            stop: Some(tx),
            thread: Some(thread),
            done: Some(done_rx),
        })
    }

    /// Stops the meter. Waits at most [`STOP_WAIT`]; a thread still stuck in
    /// tap creation is detached and exits (destroying its tap) as soon as it
    /// sees the closed stop channel.
    pub fn stop(&mut self) {
        self.stop.take();
        let finished = match self.done.take() {
            Some(done) => !matches!(done.recv_timeout(STOP_WAIT), Err(RecvTimeoutError::Timeout)),
            None => true,
        };
        if let Some(t) = self.thread.take() {
            if !finished {
                log::warn!("audio meter thread did not stop within {STOP_WAIT:?}: detached");
            } else if t.join().is_err() {
                log::error!("audio meter thread panicked");
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for LevelMeter {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Data shared with the real-time IOProc (read-only there).
pub(crate) struct IoClient {
    pub ring: Arc<Ring>,
    pub tap_channels: u32,
}

/// The IOProc: silence the outputs (the aggregate's device would otherwise
/// play stale memory), copy the tap buffer into the ring. No allocation, no
/// lock, no logging.
unsafe extern "C-unwind" fn meter_io(
    _device: ObjectId,
    _now: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    input: NonNull<objc2_core_audio_types::AudioBufferList>,
    _input_time: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    output: NonNull<objc2_core_audio_types::AudioBufferList>,
    _output_time: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the IoClient boxed by `Pipeline`, alive until the
    // IoProc is stopped and destroyed; buffer lists are valid for this call.
    unsafe {
        let c = &*(client as *const IoClient);
        for b in hal::buffers(output) {
            if !b.mData.is_null() {
                std::ptr::write_bytes(b.mData.cast::<u8>(), 0, b.mDataByteSize as usize);
            }
        }
        let ins = hal::buffers(input);
        let mut shape = [(0u32, false); 16];
        let n = ins.len().min(shape.len());
        for (s, b) in shape.iter_mut().zip(ins.iter()).take(n) {
            *s = (b.mNumberChannels, !b.mData.is_null());
        }
        if let Some(i) = routing::tap_buffer_index(&shape[..n], c.tap_channels) {
            let b = &ins[i];
            let len = b.mDataByteSize as usize / std::mem::size_of::<f32>();
            let data = std::slice::from_raw_parts(b.mData.cast::<f32>(), len);
            c.ring.write_interleaved(data, b.mNumberChannels as usize);
        }
    }
    0
}

/// Aggregate + IOProc around a tap. Field order = drop order.
struct Pipeline {
    _io: IoProc,
    _client: Box<IoClient>,
    _aggregate: Aggregate,
    host_uid: String,
    sample_rate: f64,
}

fn build_pipeline(tap: &Tap, ring: &Arc<Ring>) -> Result<Pipeline, String> {
    let host = hal::default_output_device().ok_or("aucune sortie audio par défaut")?;
    let host_uid = hal::device_uid(host).ok_or("UID de la sortie introuvable")?;
    let aggregate = hal::create_aggregate("Nox Levels", &host_uid, Some(&tap.uuid))
        .map_err(|s| format!("agrégat : {s}"))?;
    let sample_rate = hal::nominal_sample_rate(aggregate.id).unwrap_or(48_000.0);
    let client = Box::new(IoClient {
        ring: ring.clone(),
        tap_channels: tap.channels,
    });
    let ptr = (&*client as *const IoClient).cast_mut().cast::<c_void>();
    // SAFETY: `client` is boxed (stable address) and stored after `_io` in
    // Pipeline, so it outlives the IOProc; `meter_io` treats it as IoClient.
    let io = unsafe { IoProc::start(aggregate.id, meter_io, ptr) }
        .map_err(|s| format!("IOProc : {s}"))?;
    Ok(Pipeline {
        _io: io,
        _client: client,
        _aggregate: aggregate,
        host_uid,
        sample_rate,
    })
}

/// Every audio process object `pid` is responsible for (browser helpers
/// included); its own process object if none holds audio yet.
fn player_objects(pid: i32, owners: &mut HashMap<i32, Option<i32>>) -> BTreeSet<ObjectId> {
    let mut out: BTreeSet<ObjectId> = hal::process_objects()
        .into_iter()
        .filter(|&o| {
            hal::process_pid(o).is_some_and(|p| {
                p == pid || *owners.entry(p).or_insert_with(|| process::owning_app(p)) == Some(pid)
            })
        })
        .collect();
    if out.is_empty()
        && let Some(o) = hal::translate_pid(pid)
    {
        out.insert(o);
    }
    out
}

fn run(pid: i32, cfg: MeterConfig, stop: mpsc::Receiver<()>, mut emit: impl FnMut(MeterEvent)) {
    let mut owners = HashMap::new();
    let ring = Arc::new(Ring::new(8192));
    let mut tapped = player_objects(pid, &mut owners);
    let objects: Vec<ObjectId> = tapped.iter().copied().collect();
    let mut tap = match hal::create_tap(&objects, false, "Nox Levels") {
        Ok(t) => t,
        Err(s) => return emit(MeterEvent::Unavailable(format!("tap : {s}"))),
    };
    let mut pipe = match build_pipeline(&tap, &ring) {
        Ok(p) => p,
        Err(e) => return emit(MeterEvent::Unavailable(e)),
    };
    let mut analyzer = Analyzer::new(pipe.sample_rate);
    let mut smoother = Smoother::default();
    let mut block = vec![0f32; FFT_SIZE];
    let started = Instant::now();
    let mut last_check = Instant::now();
    let period = Duration::from_secs_f64(1.0 / cfg.updates_per_sec.clamp(1.0, 120.0));
    let mut last_sent: Option<[f32; BAND_COUNT]> = None;

    loop {
        match stop.recv_timeout(period) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if last_check.elapsed() >= cfg.check_every {
            last_check = Instant::now();
            // Output device or its rate changed (headphones, AirPlay…).
            let host_uid = hal::default_output_device().and_then(hal::device_uid);
            if host_uid.as_deref() != Some(pipe.host_uid.as_str()) {
                drop(pipe);
                pipe = match build_pipeline(&tap, &ring) {
                    Ok(p) => p,
                    Err(e) => return emit(MeterEvent::Unavailable(e)),
                };
                analyzer = Analyzer::new(pipe.sample_rate);
            }
            let current = player_objects(pid, &mut owners);
            match routing::tap_change(&tapped, &current) {
                TapChange::None => {}
                TapChange::Restart => return emit(MeterEvent::ProcessesLeft),
                TapChange::Rebuild => {
                    drop(pipe);
                    drop(tap);
                    tapped = current;
                    let objects: Vec<ObjectId> = tapped.iter().copied().collect();
                    tap = match hal::create_tap(&objects, false, "Nox Levels") {
                        Ok(t) => t,
                        Err(s) => return emit(MeterEvent::Unavailable(format!("tap : {s}"))),
                    };
                    pipe = match build_pipeline(&tap, &ring) {
                        Ok(p) => p,
                        Err(e) => return emit(MeterEvent::Unavailable(e)),
                    };
                    analyzer = Analyzer::new(pipe.sample_rate);
                }
            }
        }
        if !ring.has_heard(1.0) {
            if started.elapsed() >= cfg.silence_grace {
                return emit(MeterEvent::Silent);
            }
            continue;
        }
        if !ring.latest(&mut block) {
            continue;
        }
        let levels = smoother.next(analyzer.levels(&block));
        if last_sent != Some(levels) {
            last_sent = Some(levels);
            emit(MeterEvent::Levels(levels));
        }
    }
}

/// IO cycles seen by [`spike_io`] (diagnostics only).
pub(crate) static SPIKE_CYCLES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// `meter_io` plus a cycle counter, for the no-tap spike.
pub(crate) unsafe extern "C-unwind" fn spike_io(
    device: ObjectId,
    now: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    input: NonNull<objc2_core_audio_types::AudioBufferList>,
    input_time: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    output: NonNull<objc2_core_audio_types::AudioBufferList>,
    output_time: NonNull<objc2_core_audio_types::AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    SPIKE_CYCLES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: same contract as `meter_io`, forwarded unchanged.
    unsafe { meter_io(device, now, input, input_time, output, output_time, client) }
}
