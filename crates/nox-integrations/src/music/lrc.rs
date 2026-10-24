// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchLyricsSupport.swift, commit e916961.

//! LRC parsing and line synchronisation.

use nox_core::model::music::LyricLine;

pub const MAX_BYTES: usize = 512 * 1024;
pub const MAX_LINES: usize = 5_000;

/// `mm:ss`, `mm:ss.xx`, `mm:ss.xxx` or `mm:ss:xx` → seconds.
fn timestamp(tag: &str) -> Option<f64> {
    let (min, rest) = tag.split_once(':')?;
    let min: u32 = min.trim().parse().ok()?;
    let (sec, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    if sec.is_empty() || sec.len() > 2 || !sec.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sec: u32 = sec.parse().ok()?;
    if sec >= 60 {
        return None;
    }
    let frac = match frac {
        Some(f) if !f.is_empty() && f.len() <= 3 && f.bytes().all(|b| b.is_ascii_digit()) => {
            f.parse::<f64>().ok()? / 10f64.powi(f.len() as i32)
        }
        Some(_) => return None,
        None => 0.0,
    };
    Some(f64::from(min) * 60.0 + f64::from(sec) + frac)
}

/// Parses synced LRC text. Lines past `duration` are dropped; `[offset:±ms]`
/// is honoured; a line with several tags is repeated at each time.
/// Returns an empty vector when the text has no timed line.
pub fn parse(src: &str, duration: Option<f64>) -> Vec<LyricLine> {
    if src.len() > MAX_BYTES {
        return vec![];
    }
    let mut offset = 0.0;
    let mut entries: Vec<(f64, usize, String)> = Vec::new();
    for raw in src.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower
            .strip_prefix("[offset:")
            .and_then(|r| r.strip_suffix(']'))
        {
            if let Ok(ms) = v.trim().parse::<f64>()
                && ms.is_finite()
                && ms.abs() <= 60_000.0
            {
                offset = ms / 1000.0;
            }
            continue;
        }
        let mut text = line;
        let mut times = Vec::new();
        while let Some(rest) = text.strip_prefix('[') {
            let Some(end) = rest.find(']') else { break };
            let Some(t) = timestamp(&rest[..end]) else {
                break;
            };
            times.push(t);
            text = &rest[end + 1..];
        }
        let words = text.trim();
        for t in times {
            if entries.len() >= MAX_LINES {
                return vec![];
            }
            entries.push((t, entries.len(), words.to_owned()));
        }
    }
    // A positive LRC offset means lyrics are shown earlier.
    let mut out: Vec<(f64, usize, String)> = entries
        .into_iter()
        .map(|(t, i, s)| ((t - offset).max(0.0), i, s))
        .filter(|(t, _, _)| duration.is_none_or(|d| d <= 0.0 || *t <= d))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    out.into_iter()
        .map(|(t, _, text)| LyricLine {
            time: Some(t),
            text,
        })
        .collect()
}

/// Unsynced lyrics: one line per non-empty text line.
pub fn plain(src: &str) -> Vec<LyricLine> {
    src.lines()
        .take(MAX_LINES)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| LyricLine {
            time: None,
            text: l.to_owned(),
        })
        .collect()
}

/// Index of the line being sung at `t` (last line starting at or before `t`).
pub fn active_index(lines: &[LyricLine], t: f64) -> Option<usize> {
    let i = lines.partition_point(|l| l.time.is_some_and(|lt| lt <= t));
    i.checked_sub(1).filter(|&i| lines[i].time.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp("01:02.50"), Some(62.5));
        assert_eq!(timestamp("00:05.123"), Some(5.123));
        assert_eq!(timestamp("00:05:10"), Some(5.1));
        assert_eq!(timestamp("3:07"), Some(187.0));
        assert_eq!(timestamp("ar:Artist"), None);
        assert_eq!(timestamp("00:75.00"), None);
    }

    #[test]
    fn parses_synthetic_lrc() {
        let src = "[ti:Synthetic]\n[ar:Nobody]\n[00:01.00]First\n\
                   [00:05.50][00:20.00]Chorus\n[00:03.00]Second\n[00:10.00]\n[09:00.00]Past the end";
        let lines = parse(src, Some(180.0));
        let times: Vec<f64> = lines.iter().map(|l| l.time.unwrap()).collect();
        assert_eq!(times, vec![1.0, 3.0, 5.5, 10.0, 20.0]);
        assert_eq!(lines[2].text, "Chorus");
        assert_eq!(lines[3].text, "", "empty timed line ends a verse");
        assert_eq!(active_index(&lines, 0.5), None);
        assert_eq!(active_index(&lines, 3.2), Some(1));
        assert_eq!(active_index(&lines, 100.0), Some(4));
    }

    #[test]
    fn honours_offset_and_rejects_untimed() {
        let lines = parse("[offset:+500]\n[00:02.00]A", None);
        assert_eq!(lines[0].time, Some(1.5));
        assert!(parse("just words\nno tags", None).is_empty());
        assert_eq!(plain("a\n\n b ").len(), 2);
    }
}
