// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Audio/MixerRender.swift (channel mapping, silence the tail), commit e916961.
//! Real-time render of a tapped app onto the output with a gain (E9).
//! Pure functions on slices: no allocation, no lock, no syscall.

/// Which tap channel feeds output channel `out` (None = silent channel).
pub fn source_channel(out: usize, tap_channels: usize) -> Option<usize> {
    match tap_channels {
        0 => None,
        // A mono source belongs in both front channels.
        1 if out < 2 => Some(0),
        1 => None,
        n if out < n => Some(out),
        _ => None,
    }
}

/// Soft limiter for boosts above unity: linear up to 0.8, then a smooth
/// knee that never exceeds ±1 (avoids hard-clipping distortion).
pub fn limit(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let over = (a - KNEE) / (1.0 - KNEE);
        let y = KNEE + (1.0 - KNEE) * (over / (1.0 + over));
        y.copysign(x)
    }
}

/// Renders interleaved `tap` (tap_ch channels) into interleaved `out`
/// (out_ch channels, which are device channels `first..first+out_ch`: an
/// output with one buffer per channel passes each buffer's offset), ramping the gain linearly from `from` to `to` across
/// the block (no zipper noise on slider moves). Unwritten frames are
/// silenced. Returns frames written.
pub fn render(
    tap: &[f32],
    tap_ch: usize,
    out: &mut [f32],
    out_ch: usize,
    first: usize,
    from: f32,
    to: f32,
) -> usize {
    if out_ch == 0 {
        return 0;
    }
    let out_frames = out.len() / out_ch;
    let tap_frames = tap.len().checked_div(tap_ch).unwrap_or(0);
    let frames = out_frames.min(tap_frames);
    let step = if frames > 1 {
        (to - from) / (frames - 1) as f32
    } else {
        0.0
    };
    let boost = from > 1.0 || to > 1.0;
    for f in 0..frames {
        let g = if frames > 1 {
            from + step * f as f32
        } else {
            to
        };
        for c in 0..out_ch {
            let v = match source_channel(first + c, tap_ch) {
                Some(s) => tap[f * tap_ch + s] * g,
                None => 0.0,
            };
            out[f * out_ch + c] = if boost { limit(v) } else { v };
        }
    }
    for v in &mut out[frames * out_ch..] {
        *v = 0.0;
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_mapping() {
        assert_eq!(source_channel(0, 2), Some(0));
        assert_eq!(source_channel(1, 2), Some(1));
        assert_eq!(source_channel(2, 2), None);
        assert_eq!(source_channel(1, 1), Some(0), "mono to both fronts");
        assert_eq!(source_channel(2, 1), None);
        assert_eq!(source_channel(0, 0), None);
    }

    #[test]
    fn renders_with_gain_and_silences_tail() {
        let tap = [0.5, -0.5, 0.25, -0.25];
        let mut out = [9.0; 8]; // 2 frames of 4 channels
        assert_eq!(render(&tap, 2, &mut out, 4, 0, 0.5, 0.5), 2);
        assert_eq!(out, [0.25, -0.25, 0.0, 0.0, 0.125, -0.125, 0.0, 0.0]);
        let mut longer = [9.0; 6]; // 3 stereo frames, tap has 2
        render(&tap, 2, &mut longer, 2, 0, 1.0, 1.0);
        assert_eq!(&longer[4..], &[0.0, 0.0]);
    }

    #[test]
    fn per_channel_buffers_use_their_offset() {
        let tap = [0.1, 0.2];
        let mut right = [0.0; 1];
        render(&tap, 2, &mut right, 1, 1, 1.0, 1.0);
        assert_eq!(right, [0.2]);
        let mut third = [5.0; 1];
        render(&tap, 2, &mut third, 1, 2, 1.0, 1.0);
        assert_eq!(third, [0.0]);
    }

    #[test]
    fn gain_ramps_across_the_block() {
        let tap = [1.0; 4]; // 4 mono frames
        let mut out = [0.0; 4];
        render(&tap, 1, &mut out, 1, 0, 0.0, 0.75);
        assert_eq!(out, [0.0, 0.25, 0.5, 0.75]);
    }

    #[test]
    fn mute_is_silence_and_boost_is_limited() {
        let tap = [0.9, -0.9];
        let mut out = [1.0; 2];
        render(&tap, 2, &mut out, 2, 0, 0.0, 0.0);
        assert_eq!(out, [0.0, -0.0]);
        render(&tap, 2, &mut out, 2, 0, 2.0, 2.0);
        assert!(
            out.iter().all(|v| v.abs() < 1.0 && v.abs() > 0.8),
            "{out:?}"
        );
        assert_eq!(limit(0.5), 0.5);
        assert!(limit(100.0) < 1.0 && limit(-100.0) > -1.0);
    }
}
