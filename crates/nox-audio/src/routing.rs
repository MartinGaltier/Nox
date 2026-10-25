// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Audio/MixerRoutingSupport.swift (owningRegularAppPid), Services/Notch/NotchAudioLevelSupport.swift (tapChange), Services/Audio/MixerRender.swift (tapBufferIndex), commit e916961.
//! Pure decisions shared by the meter and the mixer.

use std::collections::BTreeSet;

/// Walks from the responsible process up the parent chain (bounded) to the
/// first regular app; then the same from `pid` itself.
pub fn owning_app(
    pid: i32,
    responsible: impl Fn(i32) -> i32,
    is_app: impl Fn(i32) -> bool,
    parent: impl Fn(i32) -> i32,
) -> Option<i32> {
    let walk = |start: i32| {
        let mut p = start;
        for _ in 0..16 {
            if p <= 1 {
                return None;
            }
            if is_app(p) {
                return Some(p);
            }
            p = parent(p);
        }
        None
    };
    let r = responsible(pid);
    walk(r).or_else(|| if r != pid { walk(pid) } else { None })
}

/// What a change in the player's audio processes asks of its tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapChange {
    None,
    /// Only new processes: rebuild around all of them, keep what was heard.
    Rebuild,
    /// A tapped process left: start over (its buffers will never arrive).
    Restart,
}

pub fn tap_change(tapped: &BTreeSet<u32>, current: &BTreeSet<u32>) -> TapChange {
    if tapped == current {
        TapChange::None
    } else if !tapped.is_subset(current) {
        TapChange::Restart
    } else {
        TapChange::Rebuild
    }
}

/// Which input buffer of an aggregate carries the tap: the last one with the
/// tap's channel count; a lone buffer otherwise. Never guesses among several
/// (that could be the device's own microphone).
pub fn tap_buffer_index(channels: &[(u32, bool)], tap_channels: u32) -> Option<usize> {
    let mut lone = None;
    for (i, &(ch, has_data)) in channels.iter().enumerate().rev() {
        if !has_data || ch == 0 {
            continue;
        }
        if ch == tap_channels {
            return Some(i);
        }
        if channels.len() == 1 {
            lone = Some(i);
        }
    }
    lone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_through_responsibility_then_parents() {
        // 500 = Safari (app); 600 = WebKit.GPU, responsible = 500.
        let app = |p| p == 500 || p == 900;
        assert_eq!(
            owning_app(600, |p| if p == 600 { 500 } else { p }, app, |_| 1),
            Some(500)
        );
        // Chrome helper 700 answers for itself; parent chain 700 → 650 → 900 (Chrome).
        let parent = |p| match p {
            700 => 650,
            650 => 900,
            _ => 1,
        };
        assert_eq!(owning_app(700, |p| p, app, parent), Some(900));
        // Daemon with no app ancestor.
        assert_eq!(owning_app(42, |p| p, app, |_| 1), None);
        // Cycle protection.
        assert_eq!(owning_app(10, |p| p, |_| false, |p| p), None);
    }

    #[test]
    fn tap_changes() {
        let s = |v: &[u32]| v.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(tap_change(&s(&[1, 2]), &s(&[2, 1])), TapChange::None);
        assert_eq!(tap_change(&s(&[1]), &s(&[1, 3])), TapChange::Rebuild);
        assert_eq!(tap_change(&s(&[1, 2]), &s(&[1, 3])), TapChange::Restart);
    }

    #[test]
    fn picks_tap_buffer() {
        // Mic (1 ch) first, tap (2 ch) after it.
        assert_eq!(tap_buffer_index(&[(1, true), (2, true)], 2), Some(1));
        assert_eq!(tap_buffer_index(&[(4, true)], 2), Some(0), "lone buffer");
        assert_eq!(
            tap_buffer_index(&[(1, true), (4, true)], 2),
            None,
            "never guess"
        );
        assert_eq!(tap_buffer_index(&[(2, false)], 2), None);
    }
}
