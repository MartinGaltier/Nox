//! Opt-in Core Audio process taps for Nox (owned by Agent Mesures/Audio).
//!
//! * [`meter::LevelMeter`] — live levels of the current player (B7), for the
//!   music module: start it only while live levels are enabled, the player
//!   plays and the music section is visible.
//! * Per-app volume (E9) — see docs/AUDIO.md (in progress).
//!
//! Privacy: a tap needs the "System Audio Recording" permission
//! (`NSAudioCaptureUsageDescription`), asked by macOS the first time a tap is
//! created. Nox creates a tap only for an explicit opt-in feature, reads it in
//! memory, never stores or sends audio, and destroys it as soon as the
//! feature is not shown. Taps and aggregates are private: macOS removes them
//! if Nox exits or crashes.

pub mod analysis;
pub mod meter;
pub mod routing;
mod sys;

pub use meter::{LevelMeter, MeterConfig, MeterEvent};

/// Process taps exist from macOS 14.2; the process-object properties Nox
/// relies on are stable from 14.4 (same floor as the reference app).
pub fn supported() -> bool {
    os_version() >= (14, 4)
}

fn os_version() -> (u32, u32) {
    let mut buf = [0u8; 32];
    let mut len = buf.len();
    // SAFETY: buffer and length describe `buf`; read-only sysctl.
    let r = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return (0, 0);
    }
    let s = String::from_utf8_lossy(&buf[..len.min(buf.len())]);
    let mut it = s
        .trim_end_matches('\0')
        .split('.')
        .map(|p| p.parse().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// One audio client as the HAL sees it (no tap involved).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioClient {
    pub object: u32,
    pub pid: i32,
    pub bundle_id: Option<String>,
    /// The regular app this process plays for (itself for an app).
    pub owner_pid: Option<i32>,
    pub owner_name: Option<String>,
    pub running_output: bool,
}

/// Every process holding an audio connection. Reading this needs no
/// permission and creates nothing.
pub fn audio_clients() -> Vec<AudioClient> {
    sys::hal::process_objects()
        .into_iter()
        .filter_map(|o| {
            let pid = sys::hal::process_pid(o)?;
            let owner_pid = sys::process::owning_app(pid);
            Some(AudioClient {
                object: o,
                pid,
                bundle_id: sys::hal::process_bundle_id(o),
                owner_pid,
                owner_name: owner_pid.and_then(|p| sys::process::app_identity(p).0),
                running_output: sys::hal::process_running_output(o),
            })
        })
        .collect()
}

/// The default output device: (name, uid, sample rate, output channels,
/// input channels).
pub fn default_output() -> Option<(String, String, f64, u32, u32)> {
    let d = sys::hal::default_output_device()?;
    Some((
        sys::hal::object_name(d).unwrap_or_default(),
        sys::hal::device_uid(d)?,
        sys::hal::nominal_sample_rate(d).unwrap_or(0.0),
        sys::hal::channel_count(d, sys::hal::SCOPE_OUTPUT),
        sys::hal::channel_count(d, sys::hal::SCOPE_INPUT),
    ))
}

#[doc(hidden)]
pub mod diag {
    //! Spike helpers for the `#[ignore]` hardware tests (not an API).

    use crate::analysis::Ring;
    use crate::sys::hal;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Builds a private aggregate on the default output **without any tap**
    /// and runs the meter IOProc for `dur`: validates the aggregate/IOProc
    /// machinery and measures its cost. Refuses (Err) when the default
    /// output has input channels, so no microphone access is ever started.
    /// Returns (IO cycles' frames seen, process CPU time used).
    pub fn aggregate_without_tap(dur: Duration) -> Result<(usize, Duration), String> {
        let host = hal::default_output_device().ok_or("no default output")?;
        if hal::channel_count(host, hal::SCOPE_INPUT) > 0 {
            return Err("default output has inputs: refusing (would open the microphone)".into());
        }
        let uid = hal::device_uid(host).ok_or("no uid")?;
        let agg = hal::create_aggregate("Nox Spike", &uid, None).map_err(|s| s.to_string())?;
        let ring = Arc::new(Ring::new(8192));
        let client = Box::new(crate::meter::IoClient {
            ring: ring.clone(),
            tap_channels: 2,
        });
        let cpu0 = cpu_time();
        let ptr = (&*client as *const crate::meter::IoClient)
            .cast_mut()
            .cast::<std::ffi::c_void>();
        // SAFETY: `client` outlives `io` (dropped after it below).
        let io = unsafe { hal::IoProc::start(agg.id, crate::meter::spike_io, ptr) }
            .map_err(|s| s.to_string())?;
        let t = Instant::now();
        std::thread::sleep(dur);
        drop(io);
        let used = cpu_time().saturating_sub(cpu0);
        let cycles = crate::meter::SPIKE_CYCLES.swap(0, std::sync::atomic::Ordering::Relaxed);
        drop(client);
        drop(agg);
        let _ = t;
        Ok((cycles, used))
    }

    /// Process CPU time (user + system).
    pub fn cpu_time() -> Duration {
        // SAFETY: an all-zero rusage is valid; getrusage fills it.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: valid out pointer.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        let tv = |t: libc::timeval| {
            Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64)
        };
        tv(ru.ru_utime) + tv(ru.ru_stime)
    }

    /// Cost of the analysis path alone: `n` FFT + band + smoother passes.
    pub fn analysis_cost(n: u32) -> Duration {
        let mut a = crate::analysis::Analyzer::new(48_000.0);
        let mut s = crate::analysis::Smoother::default();
        let block: Vec<f32> = (0..crate::analysis::FFT_SIZE)
            .map(|i| (i as f32 * 0.05).sin() * 0.5)
            .collect();
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(s.next(a.levels(std::hint::black_box(&block))));
        }
        t.elapsed() / n
    }
}
