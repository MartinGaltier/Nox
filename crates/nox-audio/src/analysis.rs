// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchAudioLevelSupport.swift and NotchAudioLevelService.swift (band edges, dB floor, smoother, ring), commit e916961.
//! Pure signal path from tap samples to bar heights: a lock-free mono ring
//! written by the real-time IOProc, a 1024-point FFT, seven log-spaced bands
//! on a −60 dB floor, and a rise-fast / fall-slow smoother.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

pub const BAND_COUNT: usize = 7;
/// Band edges in hertz, about a third of an octave apart from bass to air.
pub const BAND_EDGES_HZ: [f64; BAND_COUNT + 1] =
    [50.0, 120.0, 250.0, 500.0, 1000.0, 2000.0, 4500.0, 12_000.0];
/// Levels below this many dB under full scale read as silence.
pub const FLOOR_DB: f64 = 60.0;
pub const FFT_SIZE: usize = 1024;
/// A sample above this (after compensation) proves the tap delivers sound.
pub const HEARD_THRESHOLD: f32 = 0.001;

/// Mono samples from the audio thread, most recent last.
///
/// The writer (IOProc) never blocks and never allocates: it stores `f32`
/// bits into atomics and then publishes the new head. A reader racing a
/// write may see a few samples from the next block, which is harmless for a
/// meter.
pub struct Ring {
    samples: Box<[AtomicU32]>,
    head: AtomicUsize,
    written: AtomicUsize,
    peak: AtomicU32,
    heard: AtomicBool,
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.next_power_of_two().max(FFT_SIZE);
        Self {
            samples: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            written: AtomicUsize::new(0),
            peak: AtomicU32::new(0),
            heard: AtomicBool::new(false),
        }
    }

    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    /// Writes interleaved frames, mixed down to mono. Real-time safe.
    pub fn write_interleaved(&self, data: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let mask = self.samples.len() - 1;
        let mut head = self.head.load(Ordering::Relaxed);
        let scale = 1.0 / channels as f32;
        let mut loudest = 0f32;
        let frames = data.len() / channels;
        for frame in data.chunks_exact(channels) {
            let mono = frame.iter().sum::<f32>() * scale;
            let mono = if mono.is_finite() { mono } else { 0.0 };
            self.samples[head & mask].store(mono.to_bits(), Ordering::Relaxed);
            head = head.wrapping_add(1);
            loudest = loudest.max(mono.abs());
        }
        self.head.store(head, Ordering::Release);
        self.written.fetch_add(frames, Ordering::Relaxed);
        // Positive f32 bit patterns order like the values themselves.
        self.peak.fetch_max(loudest.to_bits(), Ordering::Relaxed);
    }

    /// Whether any sample so far, scaled by `gain`, carried real sound. Once
    /// true it stays true (a later lower gain must not undo the proof).
    pub fn has_heard(&self, gain: f32) -> bool {
        if self.heard.load(Ordering::Relaxed) {
            return true;
        }
        let peak = f32::from_bits(self.peak.load(Ordering::Relaxed));
        if peak * gain > HEARD_THRESHOLD {
            self.heard.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    pub fn frames_written(&self) -> usize {
        self.written.load(Ordering::Relaxed)
    }

    /// The most recent `out.len()` samples, oldest first; false until that
    /// many have arrived.
    pub fn latest(&self, out: &mut [f32]) -> bool {
        let n = out.len();
        if n == 0 || n > self.samples.len() || self.frames_written() < n {
            return false;
        }
        let mask = self.samples.len() - 1;
        let head = self.head.load(Ordering::Acquire);
        let start = head.wrapping_sub(n);
        for (i, o) in out.iter_mut().enumerate() {
            *o = f32::from_bits(self.samples[start.wrapping_add(i) & mask].load(Ordering::Relaxed));
        }
        true
    }
}

/// In-place iterative radix-2 FFT (size fixed at construction).
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
}

impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let (cos, sin) = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .unzip();
        Self { n, cos, sin, rev }
    }

    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        assert!(re.len() == n && im.len() == n);
        for i in 0..n {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }
}

/// The FFT bins of each band for `sample_rate` and `size`. Every band keeps
/// at least one bin; bin 0 (DC) belongs to none.
pub fn band_ranges(sample_rate: f64, size: usize) -> [std::ops::Range<usize>; BAND_COUNT] {
    let usable = (size / 2).max(2);
    let bin_width = if sample_rate > 0.0 && size > 0 {
        sample_rate / size as f64
    } else {
        1.0
    };
    std::array::from_fn(|b| {
        let lower = ((BAND_EDGES_HZ[b] / bin_width).floor() as usize).clamp(1, usable - 1);
        let upper = ((BAND_EDGES_HZ[b + 1] / bin_width).floor() as usize).clamp(lower + 1, usable);
        lower..upper
    })
}

/// Mean magnitude of each band on a dB scale with a floor, 0..1.
/// Magnitudes are relative to full scale (a full-scale sine reads 1).
pub fn band_levels(magnitudes: &[f32], bands: &[std::ops::Range<usize>]) -> [f32; BAND_COUNT] {
    std::array::from_fn(|b| {
        let Some(band) = bands.get(b) else { return 0.0 };
        let lo = band.start.min(magnitudes.len());
        let hi = band.end.min(magnitudes.len());
        if lo >= hi {
            return 0.0;
        }
        let mean = magnitudes[lo..hi]
            .iter()
            .map(|&m| f64::from(m))
            .sum::<f64>()
            / (hi - lo) as f64;
        if mean <= 0.0 {
            return 0.0;
        }
        (((20.0 * mean.log10()) + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0) as f32
    })
}

/// Hann-windowed magnitude spectrum of one block.
pub struct Analyzer {
    fft: Fft,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    mags: Vec<f32>,
    pub bands: [std::ops::Range<usize>; BAND_COUNT],
}

impl Analyzer {
    pub fn new(sample_rate: f64) -> Self {
        let n = FFT_SIZE;
        let window = (0..n)
            .map(|i| {
                (0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())) as f32
            })
            .collect();
        Self {
            fft: Fft::new(n),
            window,
            re: vec![0.0; n],
            im: vec![0.0; n],
            mags: vec![0.0; n / 2],
            bands: band_ranges(sample_rate, n),
        }
    }

    /// Magnitudes of the first `n/2` bins. Hann coherent gain is 0.5 and a
    /// real sine splits its energy over ±f, hence the 4/n scale.
    pub fn magnitudes(&mut self, samples: &[f32]) -> &[f32] {
        let n = FFT_SIZE;
        if samples.len() != n {
            return &self.mags;
        }
        for ((re, im), (&x, &w)) in self
            .re
            .iter_mut()
            .zip(self.im.iter_mut())
            .zip(samples.iter().zip(&self.window))
        {
            *re = x * w;
            *im = 0.0;
        }
        self.fft.forward(&mut self.re, &mut self.im);
        let scale = 4.0 / n as f32;
        for (k, m) in self.mags.iter_mut().enumerate() {
            *m = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * scale;
        }
        &self.mags
    }

    pub fn levels(&mut self, samples: &[f32]) -> [f32; BAND_COUNT] {
        let bands = self.bands.clone();
        band_levels(self.magnitudes(samples), &bands)
    }
}

/// Bars rise fast and fall slowly; the loudest band sets the scale (slowly
/// relaxing) so a quiet track still fills the meter.
pub struct Smoother {
    levels: [f32; BAND_COUNT],
    ceiling: f32,
}

impl Default for Smoother {
    fn default() -> Self {
        Self {
            levels: [0.0; BAND_COUNT],
            ceiling: 0.05,
        }
    }
}

impl Smoother {
    pub const ATTACK: f32 = 0.55;
    pub const DECAY: f32 = 0.12;
    pub const CEILING_DECAY: f32 = 0.995;
    pub const PEAK_TARGET: f32 = 0.92;

    pub fn next(&mut self, raw: [f32; BAND_COUNT]) -> [f32; BAND_COUNT] {
        let loudest = raw.iter().copied().fold(0.0, f32::max);
        self.ceiling = loudest.max(self.ceiling * Self::CEILING_DECAY).max(0.05);
        let gain = Self::PEAK_TARGET / self.ceiling;
        for (level, &r) in self.levels.iter_mut().zip(&raw) {
            let target = (r * gain).clamp(0.0, 1.0);
            let rate = if target > *level {
                Self::ATTACK
            } else {
                Self::DECAY
            };
            *level += (target - *level) * rate;
        }
        self.levels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: f64, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin() as f32)
            .collect()
    }

    #[test]
    fn fft_matches_naive_dft() {
        let n = 16;
        let x: Vec<f32> = (0..n).map(|i| ((i * 7 % 5) as f32) - 2.0).collect();
        let (mut re, mut im) = (x.clone(), vec![0.0; n]);
        Fft::new(n).forward(&mut re, &mut im);
        for k in 0..n {
            let (mut r, mut i) = (0f64, 0f64);
            for (t, &v) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                r += f64::from(v) * a.cos();
                i += f64::from(v) * a.sin();
            }
            assert!((f64::from(re[k]) - r).abs() < 1e-3, "re[{k}]");
            assert!((f64::from(im[k]) - i).abs() < 1e-3, "im[{k}]");
        }
    }

    #[test]
    fn full_scale_sine_reads_near_0_db_in_its_band() {
        let rate = 48_000.0;
        let mut a = Analyzer::new(rate);
        // 750 Hz sits in band 3 (500–1000 Hz); bin-centred to avoid leakage.
        let f = (16.0 * rate / FFT_SIZE as f64).round();
        let levels = a.levels(&sine(f, rate, 1.0, FFT_SIZE));
        let peak_band = levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak_band, 3, "{levels:?}");
        let k = 16;
        let m = a.magnitudes(&sine(f, rate, 1.0, FFT_SIZE))[k];
        assert!((m - 1.0).abs() < 0.05, "bin magnitude {m}");
    }

    #[test]
    fn silence_is_zero_and_quiet_is_low() {
        let mut a = Analyzer::new(44_100.0);
        assert_eq!(a.levels(&vec![0.0; FFT_SIZE]), [0.0; BAND_COUNT]);
        let quiet = a.levels(&sine(1000.0, 44_100.0, 0.001, FFT_SIZE));
        assert!(quiet.iter().all(|&l| l < 0.5), "{quiet:?}");
    }

    #[test]
    fn band_ranges_cover_edges_and_keep_one_bin() {
        let r = band_ranges(48_000.0, FFT_SIZE);
        assert_eq!(r[0], 1..2); // 50 Hz → bin 1, 120 Hz → bin 2
        assert!(r.iter().all(|b| b.end > b.start && b.start >= 1));
        assert!(r[BAND_COUNT - 1].end <= FFT_SIZE / 2);
        let tiny = band_ranges(8_000.0, 16);
        assert!(tiny.iter().all(|b| b.end > b.start && b.end <= 8));
    }

    #[test]
    fn ring_mixes_down_and_returns_latest() {
        let r = Ring::new(8);
        assert_eq!(r.capacity(), FFT_SIZE);
        assert!(!r.has_heard(1.0));
        r.write_interleaved(&[0.5, -0.5, 1.0, 0.0], 2); // two stereo frames
        assert_eq!(r.frames_written(), 2);
        let mut out = [0.0; 2];
        assert!(r.latest(&mut out));
        assert_eq!(out, [0.0, 0.5]);
        assert!(r.has_heard(1.0));
        let mut big = vec![0.0; 3];
        assert!(!r.latest(&mut big), "not enough samples yet");
        r.write_interleaved(&[f32::NAN], 1);
        let mut one = [9.0];
        assert!(r.latest(&mut one));
        assert_eq!(one, [0.0], "non-finite input is dropped to 0");
    }

    #[test]
    fn heard_needs_gain_for_quiet_taps() {
        let r = Ring::new(FFT_SIZE);
        r.write_interleaved(&[0.0008], 1);
        assert!(!r.has_heard(1.0));
        assert!(r.has_heard(2.0));
        assert!(r.has_heard(0.5), "stays heard");
    }

    #[test]
    fn smoother_rises_fast_falls_slow_and_autoscales() {
        let mut s = Smoother::default();
        let up = s.next([0.5; BAND_COUNT]);
        assert!(up[0] > 0.4, "{up:?}");
        let down = s.next([0.0; BAND_COUNT]);
        assert!(down[0] > up[0] * 0.8, "slow decay {down:?}");
        let mut q = Smoother::default();
        let mut v = [0.0; BAND_COUNT];
        for _ in 0..30 {
            v = q.next([0.1; BAND_COUNT]);
        }
        assert!(v[0] > 0.85, "quiet track fills the meter: {v:?}");
    }
}
