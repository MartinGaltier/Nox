// Hover suppression adapted from vorssaint-utils (GPL-3.0-or-later),
// Sources/Vorssaint/Services/Notch/NotchSupport.swift (NotchHoverState), commit e916961.
//! Compact/expanded state machine: hover with delay, click, close delay.
//! Time is passed in (monotonic seconds) so the logic is testable.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Compact,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MachineConfig {
    pub expand_on_hover: bool,
    pub hover_delay: f64,
    pub close_delay: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Machine {
    pub mode: Mode,
    cfg: MachineConfig,
    inside: bool,
    open_at: Option<f64>,
    close_at: Option<f64>,
    /// Closed by the user while the pointer was inside: hovering must leave
    /// first before it can reopen the island.
    suppressed: bool,
    /// Forced by an environment override (debug) or kept open while a drag
    /// or a menu interacts with the island.
    held: bool,
}

impl Machine {
    pub fn new(cfg: MachineConfig) -> Self {
        Self {
            mode: Mode::Compact,
            cfg,
            inside: false,
            open_at: None,
            close_at: None,
            suppressed: false,
            held: false,
        }
    }

    pub fn configure(&mut self, cfg: MachineConfig) {
        self.cfg = cfg;
        if !cfg.expand_on_hover {
            self.open_at = None;
        }
    }

    pub fn inside(&self) -> bool {
        self.inside
    }

    /// Pointer entered/left the island's shape.
    pub fn pointer(&mut self, inside: bool, now: f64) {
        if inside == self.inside {
            return;
        }
        self.inside = inside;
        if inside {
            self.close_at = None;
            if self.mode == Mode::Compact && self.cfg.expand_on_hover && !self.suppressed {
                self.open_at = Some(now + self.cfg.hover_delay.max(0.0));
            }
        } else {
            self.suppressed = false;
            self.open_at = None;
            if self.mode == Mode::Expanded && !self.held {
                self.close_at = Some(now + self.cfg.close_delay.max(0.0));
            }
        }
    }

    /// A click on the island toggles it (a click while closing cancels the close).
    pub fn click(&mut self, now: f64) {
        match self.mode {
            Mode::Compact => self.open(),
            Mode::Expanded if self.close_at.is_some() => self.close_at = None,
            Mode::Expanded => self.close(now),
        }
    }

    pub fn open(&mut self) {
        self.mode = Mode::Expanded;
        self.open_at = None;
        self.close_at = None;
        self.suppressed = false;
    }

    pub fn close(&mut self, _now: f64) {
        self.mode = Mode::Compact;
        self.open_at = None;
        self.close_at = None;
        self.suppressed = self.inside;
    }

    /// Keeps the island open regardless of the pointer (drag in progress).
    pub fn hold(&mut self, held: bool, now: f64) {
        self.held = held;
        if !held && !self.inside && self.mode == Mode::Expanded {
            self.close_at = Some(now + self.cfg.close_delay.max(0.0));
        } else if held {
            self.close_at = None;
        }
    }

    /// Applies elapsed deadlines. Returns true if the mode changed.
    pub fn tick(&mut self, now: f64) -> bool {
        let before = self.mode;
        if let Some(t) = self.open_at
            && now >= t
        {
            self.open_at = None;
            if self.inside {
                self.open();
            }
        }
        if let Some(t) = self.close_at
            && now >= t
        {
            self.close_at = None;
            if !self.inside && !self.held {
                self.mode = Mode::Compact;
            }
        }
        self.mode != before
    }

    /// Next instant at which `tick` may change something (one-shot timer).
    pub fn deadline(&self) -> Option<f64> {
        match (self.open_at, self.close_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MachineConfig {
        MachineConfig {
            expand_on_hover: true,
            hover_delay: 0.25,
            close_delay: 0.4,
        }
    }

    #[test]
    fn hover_opens_after_delay_and_closes_after_leaving() {
        let mut m = Machine::new(cfg());
        m.pointer(true, 10.0);
        assert_eq!(m.deadline(), Some(10.25));
        assert!(!m.tick(10.1));
        assert_eq!(m.mode, Mode::Compact);
        assert!(m.tick(10.25));
        assert_eq!(m.mode, Mode::Expanded);
        assert_eq!(m.deadline(), None, "no timer while resting open");
        m.pointer(false, 11.0);
        assert_eq!(m.deadline(), Some(11.4));
        assert!(!m.tick(11.2));
        assert!(m.tick(11.4));
        assert_eq!(m.mode, Mode::Compact);
        assert_eq!(m.deadline(), None);
    }

    #[test]
    fn brief_pass_does_not_open() {
        let mut m = Machine::new(cfg());
        m.pointer(true, 0.0);
        m.pointer(false, 0.1);
        assert!(!m.tick(1.0));
        assert_eq!(m.mode, Mode::Compact);
    }

    #[test]
    fn returning_cancels_close() {
        let mut m = Machine::new(cfg());
        m.open();
        m.pointer(true, 0.0);
        m.pointer(false, 1.0);
        m.pointer(true, 1.2);
        assert!(!m.tick(2.0));
        assert_eq!(m.mode, Mode::Expanded);
    }

    #[test]
    fn click_toggles_and_suppresses_hover_until_leave() {
        let mut m = Machine::new(MachineConfig {
            expand_on_hover: false,
            ..cfg()
        });
        m.pointer(true, 0.0);
        assert_eq!(m.deadline(), None, "hover disabled");
        m.click(0.5);
        assert_eq!(m.mode, Mode::Expanded);
        m.click(0.6);
        assert_eq!(m.mode, Mode::Compact);
        m.configure(cfg());
        // Still inside after a manual close: hovering does not reopen.
        m.pointer(false, 0.7);
        m.pointer(true, 0.8);
        assert!(m.tick(1.1));
        assert_eq!(m.mode, Mode::Expanded);
    }

    #[test]
    fn manual_close_inside_is_not_reopened_by_hover() {
        let mut m = Machine::new(cfg());
        m.pointer(true, 0.0);
        m.tick(0.3);
        m.close(1.0);
        assert!(!m.tick(5.0));
        assert_eq!(m.mode, Mode::Compact);
    }

    #[test]
    fn hold_keeps_open_during_drag() {
        let mut m = Machine::new(cfg());
        m.open();
        m.hold(true, 0.0);
        m.pointer(true, 0.0);
        m.pointer(false, 0.1);
        assert!(!m.tick(5.0));
        m.hold(false, 5.0);
        assert!(m.tick(5.4));
        assert_eq!(m.mode, Mode::Compact);
    }
}
