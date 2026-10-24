// Adapted from vorssaint-utils (GPL-3.0-or-later),
// Sources/Vorssaint/Services/Notch/NotchGestureSupport.swift, commit e916961.
//! One decision per physical scroll sequence: a horizontal swipe changes
//! section, a vertical one opens/closes the island. No timer is needed: the
//! next event itself expires an old phaseless sequence.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureAction {
    Open,
    Close,
    NextSection,
    PreviousSection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Origin {
    vertical: bool,
    horizontal: bool,
    expanded: bool,
}

/// One scroll event, as read from `NSEvent`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollInput {
    /// Already oriented: positive x = fingers moved right, positive y = down.
    pub x: f64,
    pub y: f64,
    pub timestamp: f64,
    pub began: bool,
    pub ended: bool,
    pub momentum: bool,
    pub precise: bool,
    pub has_phase: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GestureTracker {
    origin: Option<Origin>,
    axis: Option<Axis>,
    pending_x: f64,
    pending_y: f64,
    distance: f64,
    fired: bool,
    coasting: bool,
    last: Option<f64>,
}

const VERTICAL_THRESHOLD: f64 = 16.0;
const HORIZONTAL_THRESHOLD: f64 = 40.0;
const SEQUENCE_GAP: f64 = 0.35;

/// Converts a raw scrolling delta into finger travel.
pub fn movement(delta: f64, precise: bool, inverted: bool) -> f64 {
    if !delta.is_finite() {
        return 0.0;
    }
    delta * if inverted { 1.0 } else { -1.0 } * if precise { 1.0 } else { 24.0 }
}

impl GestureTracker {
    pub fn handle(
        &mut self,
        e: ScrollInput,
        allow_vertical: bool,
        allow_horizontal: bool,
        expanded: bool,
    ) -> Option<GestureAction> {
        if !(e.timestamp.is_finite() && e.x.is_finite() && e.y.is_finite()) {
            *self = Self::default();
            return None;
        }
        let transition = self.coasting
            && !e.momentum
            && !e.has_phase
            && e.precise
            && self
                .last
                .is_some_and(|l| e.timestamp >= l && e.timestamp - l <= SEQUENCE_GAP);
        if e.momentum || transition {
            let ok = self.coasting
                && !self.fired
                && self.origin.is_some()
                && self
                    .last
                    .is_some_and(|l| e.timestamp >= l && e.timestamp - l <= SEQUENCE_GAP)
                && e.y.abs() >= e.x.abs() * 1.5;
            if !ok {
                *self = Self::default();
                return None;
            }
            self.last = Some(e.timestamp);
            self.distance += e.y;
            let exp = self.origin.map(|o| o.expanded).unwrap_or(expanded);
            return self.vertical_action(exp);
        }
        if e.ended {
            self.coasting =
                self.origin.is_some() && self.axis == Some(Axis::Vertical) && !self.fired;
            if self.coasting {
                self.last = Some(e.timestamp);
            } else {
                *self = Self::default();
            }
            return None;
        }
        if self.coasting {
            *self = Self::default();
        }
        if e.has_phase {
            if e.began {
                *self = Self::default();
                self.origin = Some(Origin {
                    vertical: allow_vertical,
                    horizontal: allow_horizontal,
                    expanded,
                });
            }
            self.origin?;
            if self.last.is_some_and(|l| e.timestamp < l) {
                *self = Self::default();
                return None;
            }
        } else if self.origin.is_some()
            || e.began
            || self
                .last
                .is_some_and(|l| e.timestamp < l || e.timestamp - l > SEQUENCE_GAP)
        {
            *self = Self::default();
        }
        self.last = Some(e.timestamp);
        if self.fired {
            return None;
        }
        let allow_vertical = self.origin.map(|o| o.vertical).unwrap_or(allow_vertical);
        let allow_horizontal = self
            .origin
            .map(|o| o.horizontal)
            .unwrap_or(allow_horizontal);
        let expanded = self.origin.map(|o| o.expanded).unwrap_or(expanded);
        match self.axis {
            None => {
                self.pending_x += e.x;
                self.pending_y += e.y;
                let (px, py) = (self.pending_x, self.pending_y);
                if e.precise && allow_horizontal && px.abs() >= 4.0 && px.abs() >= py.abs() * 1.5 {
                    self.axis = Some(Axis::Horizontal);
                    self.distance = px;
                } else if allow_vertical && py.abs() >= 4.0 && py.abs() >= px.abs() * 1.5 {
                    self.axis = Some(Axis::Vertical);
                    self.distance = py;
                } else {
                    return None;
                }
            }
            Some(Axis::Horizontal) => self.distance += e.x,
            Some(Axis::Vertical) => self.distance += e.y,
        }
        match self.axis {
            Some(Axis::Horizontal) if allow_horizontal => {
                if self.distance.abs() < HORIZONTAL_THRESHOLD {
                    return None;
                }
                self.fired = true;
                Some(if self.distance < 0.0 {
                    GestureAction::NextSection
                } else {
                    GestureAction::PreviousSection
                })
            }
            Some(Axis::Vertical) if allow_vertical => self.vertical_action(expanded),
            _ => None,
        }
    }

    fn vertical_action(&mut self, expanded: bool) -> Option<GestureAction> {
        if self.distance.abs() < VERTICAL_THRESHOLD {
            return None;
        }
        self.fired = true;
        if self.distance > 0.0 && !expanded {
            return Some(GestureAction::Open);
        }
        if self.distance < 0.0 && expanded {
            return Some(GestureAction::Close);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(x: f64, y: f64, t: f64, began: bool, ended: bool) -> ScrollInput {
        ScrollInput {
            x,
            y,
            timestamp: t,
            began,
            ended,
            momentum: false,
            precise: true,
            has_phase: true,
        }
    }

    #[test]
    fn horizontal_swipe_fires_once() {
        let mut g = GestureTracker::default();
        assert_eq!(
            g.handle(ev(-5.0, 0.0, 0.0, true, false), true, true, true),
            None
        );
        let mut fired = vec![];
        for i in 1..10 {
            if let Some(a) = g.handle(
                ev(-10.0, 1.0, i as f64 * 0.01, false, false),
                true,
                true,
                true,
            ) {
                fired.push(a);
            }
        }
        assert_eq!(fired, vec![GestureAction::NextSection]);
        g.handle(ev(0.0, 0.0, 0.2, false, true), true, true, true);
        g.handle(ev(6.0, 0.0, 0.3, true, false), true, true, true);
        let a = g.handle(ev(40.0, 0.0, 0.31, false, false), true, true, true);
        assert_eq!(a, Some(GestureAction::PreviousSection));
    }

    #[test]
    fn vertical_pull_opens_compact_and_pushes_close() {
        let mut g = GestureTracker::default();
        g.handle(ev(0.0, 5.0, 0.0, true, false), true, true, false);
        assert_eq!(
            g.handle(ev(0.0, 12.0, 0.01, false, false), true, true, false),
            Some(GestureAction::Open)
        );
        let mut g = GestureTracker::default();
        g.handle(ev(0.0, -5.0, 0.0, true, false), true, true, true);
        assert_eq!(
            g.handle(ev(0.0, -12.0, 0.01, false, false), true, true, true),
            Some(GestureAction::Close)
        );
    }

    #[test]
    fn blocked_vertical_is_ignored_and_wheel_needs_no_timer() {
        let mut g = GestureTracker::default();
        g.handle(ev(0.0, 5.0, 0.0, true, false), false, true, true);
        assert_eq!(
            g.handle(ev(0.0, 40.0, 0.01, false, false), false, true, true),
            None
        );
        // Phaseless wheel: a pause longer than the gap starts a new sequence.
        let mut g = GestureTracker::default();
        let wheel = |y: f64, t: f64| ScrollInput {
            x: 0.0,
            y,
            timestamp: t,
            began: false,
            ended: false,
            momentum: false,
            precise: false,
            has_phase: false,
        };
        assert_eq!(
            g.handle(wheel(24.0, 0.0), true, true, false),
            Some(GestureAction::Open)
        );
        assert_eq!(g.handle(wheel(24.0, 0.1), true, true, false), None);
        assert_eq!(
            g.handle(wheel(24.0, 1.0), true, true, false),
            Some(GestureAction::Open)
        );
    }

    #[test]
    fn movement_orientation() {
        assert_eq!(movement(2.0, true, false), -2.0);
        assert_eq!(movement(1.0, false, true), 24.0);
        assert_eq!(movement(f64::NAN, true, true), 0.0);
    }
}
