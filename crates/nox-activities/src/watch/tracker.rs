// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/Notch/NotchWatchSupport.swift
// (NotchWatchTracker, fingerprint comparison), commit e916961.
//! Decides when a watched region meets its trigger, from successive
//! grey thumbnails (`SIDE`×`SIDE`, one byte per cell).

use nox_core::model::Timestamp;
use nox_core::model::watch::{WatchOutcome, WatchTrigger};

pub const SIDE: usize = 32;
/// A cell counts as changed past this grey difference (0–255), which
/// ignores compression noise, shadows and anti-aliasing shimmer.
const CELL_TOLERANCE: u8 = 24;

/// Fraction (0..1) of cells that changed between two thumbnails.
pub fn diff(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    let moved = a
        .iter()
        .zip(b)
        .filter(|(x, y)| x.abs_diff(**y) > CELL_TOLERANCE)
        .count();
    moved as f64 / a.len() as f64
}

#[derive(Debug, Clone)]
pub struct Tracker {
    pub trigger: WatchTrigger,
    /// Fraction of changed cells that counts as a change.
    pub sensitivity: f64,
    pub settle_secs: f64,
    pub confirm_secs: f64,
    baseline: Option<Vec<u8>>,
    previous: Option<Vec<u8>>,
    changed_since: Option<Timestamp>,
    pub last_change: Option<Timestamp>,
    saw_change: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Difference with the reference this trigger compares to.
    pub diff: f64,
    /// The region is moving now (for `WatchPhase::Changing`).
    pub changing: bool,
    pub outcome: Option<WatchOutcome>,
}

impl Tracker {
    pub fn new(
        trigger: WatchTrigger,
        sensitivity: f64,
        settle_secs: f64,
        confirm_secs: f64,
    ) -> Self {
        Self {
            trigger,
            sensitivity: if sensitivity.is_finite() {
                sensitivity.clamp(0.001, 0.9)
            } else {
                0.01
            },
            settle_secs: settle_secs.max(1.0),
            confirm_secs: confirm_secs.max(0.0),
            baseline: None,
            previous: None,
            changed_since: None,
            last_change: None,
            saw_change: false,
        }
    }

    pub fn observe(&mut self, thumb: Vec<u8>, at: Timestamp) -> Observation {
        let step = self.previous.as_deref().map_or(0.0, |p| diff(p, &thumb));
        let moving = self.previous.is_some() && step >= self.sensitivity;
        if moving {
            self.last_change = Some(at);
        }
        let obs = match self.trigger {
            WatchTrigger::AnyChange => {
                let Some(base) = &self.baseline else {
                    self.baseline = Some(thumb.clone());
                    self.previous = Some(thumb);
                    return Observation {
                        diff: 0.0,
                        changing: false,
                        outcome: None,
                    };
                };
                let d = diff(base, &thumb);
                if d < self.sensitivity {
                    // Back to the baseline: a blink or a redraw caught halfway.
                    self.changed_since = None;
                    Observation {
                        diff: d,
                        changing: false,
                        outcome: None,
                    }
                } else {
                    let since = *self.changed_since.get_or_insert(at);
                    Observation {
                        diff: d,
                        changing: true,
                        outcome: (at - since >= self.confirm_secs).then_some(WatchOutcome::Changed),
                    }
                }
            }
            WatchTrigger::Settled => {
                if moving {
                    self.saw_change = true;
                }
                let settled =
                    self.saw_change && self.last_change.is_some_and(|c| at - c >= self.settle_secs);
                Observation {
                    diff: step,
                    changing: moving,
                    outcome: settled.then_some(WatchOutcome::Settled),
                }
            }
        };
        self.previous = Some(thumb);
        obs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(level: u8, changed_cells: usize) -> Vec<u8> {
        let mut v = vec![level; SIDE * SIDE];
        for c in v.iter_mut().take(changed_cells) {
            *c = level.wrapping_add(100);
        }
        v
    }

    #[test]
    fn diff_ignores_noise() {
        assert_eq!(diff(&img(50, 0), &img(60, 0)), 0.0, "small grey shift");
        assert!((diff(&img(50, 0), &img(50, 512)) - 0.5).abs() < 1e-9);
        assert_eq!(diff(&[1, 2], &[1]), 1.0);
    }

    #[test]
    fn any_change_needs_confirmation() {
        let mut t = Tracker::new(WatchTrigger::AnyChange, 0.01, 8.0, 2.0);
        assert_eq!(t.observe(img(10, 0), 0.0).outcome, None);
        // A one-frame blink (caret) does not trigger.
        assert!(t.observe(img(10, 30), 1.0).changing);
        assert_eq!(t.observe(img(10, 0), 2.0).outcome, None);
        // A lasting change does, once confirmed.
        assert_eq!(t.observe(img(10, 30), 3.0).outcome, None);
        assert_eq!(t.observe(img(10, 30), 4.0).outcome, None);
        assert_eq!(
            t.observe(img(10, 30), 5.0).outcome,
            Some(WatchOutcome::Changed)
        );
        // Below sensitivity: 5 cells of 1024 < 1 %.
        let mut t = Tracker::new(WatchTrigger::AnyChange, 0.01, 8.0, 0.0);
        t.observe(img(10, 0), 0.0);
        assert_eq!(t.observe(img(10, 5), 1.0).outcome, None);
    }

    #[test]
    fn settled_waits_for_activity_then_quiet() {
        let mut t = Tracker::new(WatchTrigger::Settled, 0.01, 8.0, 0.0);
        // Quiet from the start never "settles": nothing happened.
        for s in 0..20 {
            assert_eq!(t.observe(img(10, 0), f64::from(s)).outcome, None);
        }
        // Progress moves for a while…
        for (i, s) in (20..30).enumerate() {
            let o = t.observe(img(10, 20 + i * 20), f64::from(s));
            assert!(o.changing);
            assert_eq!(o.outcome, None);
        }
        // …then holds still: settled after settle_secs.
        let last = img(10, 20 + 9 * 20);
        assert_eq!(t.observe(last.clone(), 35.0).outcome, None);
        assert_eq!(t.observe(last.clone(), 36.9).outcome, None);
        assert_eq!(t.observe(last, 37.0).outcome, Some(WatchOutcome::Settled));
    }
}
