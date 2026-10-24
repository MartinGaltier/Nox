//! Damped spring integrated in Rust. The display timer runs only while a
//! spring is unsettled, so the island costs nothing at rest.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
    stiffness: f64,
    damping: f64,
    /// Distance and speed under which the spring snaps to its target.
    epsilon: f64,
}

impl Spring {
    /// `response`: period of the undamped oscillation (s). `damping_fraction`:
    /// 1 = critically damped, < 1 = slight overshoot (SwiftUI semantics).
    pub fn new(value: f64, response: f64, damping_fraction: f64) -> Self {
        let response = response.max(0.01);
        let omega = 2.0 * std::f64::consts::PI / response;
        Self {
            value,
            velocity: 0.0,
            target: value,
            stiffness: omega * omega,
            damping: 2.0 * damping_fraction.max(0.0) * omega,
            epsilon: 0.01,
        }
    }

    pub fn with_epsilon(mut self, e: f64) -> Self {
        self.epsilon = e.max(1e-6);
        self
    }

    pub fn set_target(&mut self, t: f64) {
        if t.is_finite() {
            self.target = t;
        }
    }

    /// Jumps to `v` without animating.
    pub fn snap(&mut self, v: f64) {
        self.value = v;
        self.target = v;
        self.velocity = 0.0;
    }

    pub fn settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }

    /// Advances by `dt` seconds (sub-stepped for stability; long frame gaps are
    /// clamped so a stalled run loop does not explode the simulation).
    pub fn step(&mut self, dt: f64) {
        if self.settled() || !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let mut left = dt.min(0.1);
        const H: f64 = 1.0 / 480.0;
        while left > 0.0 {
            let h = left.min(H);
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if (self.value - self.target).abs() < self.epsilon
            && self.velocity.abs() < self.epsilon * 10.0
        {
            self.snap(self.target);
        }
    }
}

/// The animated island: surface size and content opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandMotion {
    pub w: Spring,
    pub h: Spring,
    /// 0 = compact content, 1 = expanded content.
    pub open: Spring,
}

impl IslandMotion {
    pub fn new(w: f64, h: f64) -> Self {
        Self {
            w: Spring::new(w, 0.42, 0.78).with_epsilon(0.2),
            h: Spring::new(h, 0.42, 0.82).with_epsilon(0.2),
            open: Spring::new(0.0, 0.32, 1.0).with_epsilon(0.004),
        }
    }
    pub fn set_target(&mut self, w: f64, h: f64, open: bool) {
        self.w.set_target(w);
        self.h.set_target(h);
        self.open.set_target(if open { 1.0 } else { 0.0 });
    }
    pub fn snap(&mut self, w: f64, h: f64, open: bool) {
        self.w.snap(w);
        self.h.snap(h);
        self.open.snap(if open { 1.0 } else { 0.0 });
    }
    pub fn step(&mut self, dt: f64) {
        self.w.step(dt);
        self.h.step(dt);
        self.open.step(dt);
    }
    pub fn settled(&self) -> bool {
        self.w.settled() && self.h.settled() && self.open.settled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_exactly_and_stops() {
        let mut s = Spring::new(0.0, 0.4, 0.8).with_epsilon(0.2);
        s.set_target(300.0);
        let mut frames = 0;
        while !s.settled() {
            s.step(1.0 / 60.0);
            frames += 1;
            assert!(frames < 240, "spring never settled");
        }
        assert_eq!(s.value, 300.0);
        assert!(frames > 10, "animates over several frames ({frames})");
        // A settled spring ignores further steps (no work at rest).
        let before = s;
        s.step(1.0);
        assert_eq!(s, before);
    }

    #[test]
    fn underdamped_overshoots_critically_damped_does_not() {
        let mut under = Spring::new(0.0, 0.4, 0.5);
        let mut crit = Spring::new(0.0, 0.4, 1.0);
        under.set_target(100.0);
        crit.set_target(100.0);
        let (mut max_u, mut max_c) = (0.0f64, 0.0f64);
        for _ in 0..120 {
            under.step(1.0 / 60.0);
            crit.step(1.0 / 60.0);
            max_u = max_u.max(under.value);
            max_c = max_c.max(crit.value);
        }
        assert!(max_u > 100.5);
        assert!(max_c <= 100.0 + 1e-6);
    }

    #[test]
    fn long_frame_gap_is_stable() {
        let mut s = Spring::new(0.0, 0.3, 0.8);
        s.set_target(50.0);
        s.step(5.0);
        assert!(s.value.is_finite() && s.value.abs() < 200.0);
        s.set_target(f64::NAN);
        assert_eq!(s.target, 50.0);
    }

    #[test]
    fn motion_settles() {
        let mut m = IslandMotion::new(185.0, 32.0);
        assert!(m.settled());
        m.set_target(520.0, 260.0, true);
        assert!(!m.settled());
        for _ in 0..600 {
            m.step(1.0 / 120.0);
        }
        assert!(m.settled());
        assert_eq!((m.w.value, m.h.value, m.open.value), (520.0, 260.0, 1.0));
    }
}
