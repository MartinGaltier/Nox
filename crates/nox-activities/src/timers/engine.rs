//! Pure timer logic: countdowns, stopwatch with laps, Pomodoro cycle.
//!
//! Exact durations come from the monotonic clock; the wall clock is used only
//! for display (`observed_at`, `ends_at`) and to restore state after a restart.
//! Pomodoro semantics adapted from vorssaint-utils (GPL-3.0-or-later),
//! Services/Notch/NotchTimerSupport.swift, commit e916961.

use crate::clock::Clock;
use nox_core::model::Timestamp;
use nox_core::model::timers::{
    CountdownTimer, FocusPhase, FocusSession, Stopwatch, TimerCommand, TimerRun, TimerSettings,
    TimersState,
};
use serde::{Deserialize, Serialize};

/// Bounds keep a bad command from creating absurd timers.
const MAX_TIMER_SECS: f64 = 24.0 * 3600.0 * 7.0;
const MAX_TIMERS: usize = 32;
const MAX_LAPS: usize = 200;
/// Finished timers stay visible this long, then disappear.
const FINISHED_LINGER_SECS: f64 = 600.0;

#[derive(Debug, Clone, PartialEq)]
pub enum TimerEvent {
    Done { label: String, late: bool },
    FocusPhase { phase: FocusPhase, round: u32 },
    FocusCycleComplete { rounds: u32 },
}

#[derive(Debug, Clone)]
struct Countdown {
    id: u64,
    label: String,
    duration: f64,
    run: TimerRun,
    /// Running: monotonic deadline. Paused/finished: unused.
    deadline_mono: f64,
    /// Paused: remaining seconds.
    remaining: f64,
    /// Finished: monotonic time it rang (for lingering).
    finished_mono: f64,
}

#[derive(Debug, Clone, Default)]
struct Watch {
    running: bool,
    /// Elapsed before the current run.
    accumulated: f64,
    started_mono: f64,
    laps: Vec<f64>,
}

#[derive(Debug, Clone)]
struct Focus {
    phase: FocusPhase,
    /// 1-based work session number.
    round: u32,
    completed_work: u32,
    phase_secs: f64,
    paused: bool,
    deadline_mono: f64,
    remaining: f64,
}

pub struct TimerEngine {
    timers: Vec<Countdown>,
    stopwatch: Watch,
    focus: Option<Focus>,
    next_id: u64,
    settings: TimerSettings,
}

/// What survives a restart: wall-clock anchors, never monotonic values.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Persisted {
    pub version: u32,
    pub next_id: u64,
    pub timers: Vec<PersistedTimer>,
    pub stopwatch_running: bool,
    pub stopwatch_elapsed: f64,
    /// Wall time of the snapshot (to extend a running stopwatch).
    pub saved_at: Timestamp,
    pub laps: Vec<f64>,
    pub focus: Option<PersistedFocus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTimer {
    pub id: u64,
    pub label: String,
    pub duration: f64,
    pub run: TimerRun,
    /// Running: wall-clock end. Others: none.
    pub ends_at: Option<Timestamp>,
    pub remaining: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedFocus {
    pub phase: FocusPhase,
    pub round: u32,
    pub completed_work: u32,
    pub phase_secs: f64,
    pub paused: bool,
    pub ends_at: Option<Timestamp>,
    pub remaining: f64,
}

fn sane_secs(secs: f64) -> Option<f64> {
    (secs.is_finite() && secs > 0.0).then(|| secs.min(MAX_TIMER_SECS))
}

impl TimerEngine {
    pub fn new(settings: TimerSettings) -> Self {
        Self {
            timers: Vec::new(),
            stopwatch: Watch::default(),
            focus: None,
            next_id: 1,
            settings,
        }
    }

    pub fn set_settings(&mut self, s: TimerSettings) {
        self.settings = s;
    }

    pub fn sound(&self) -> bool {
        self.settings.sound
    }

    /// Something is counting: the worker must publish at 1 Hz.
    pub fn is_active(&self) -> bool {
        self.timers.iter().any(|t| t.run == TimerRun::Running)
            || self.stopwatch.running
            || self.focus.as_ref().is_some_and(|f| !f.paused)
    }

    fn has_lingering(&self) -> bool {
        self.timers.iter().any(|t| t.run == TimerRun::Finished)
    }

    /// Seconds until the next event (deadline or linger expiry), if any.
    pub fn next_deadline(&self, clock: &dyn Clock) -> Option<f64> {
        let now = clock.mono();
        let timers = self.timers.iter().filter_map(|t| match t.run {
            TimerRun::Running => Some(t.deadline_mono - now),
            TimerRun::Finished => Some(t.finished_mono + FINISHED_LINGER_SECS - now),
            TimerRun::Paused => None,
        });
        let focus = self
            .focus
            .as_ref()
            .filter(|f| !f.paused)
            .map(|f| f.deadline_mono - now);
        timers.chain(focus).map(|d| d.max(0.0)).reduce(f64::min)
    }

    /// Applies a command; returns true if state changed (persist + publish).
    pub fn command(&mut self, cmd: TimerCommand, clock: &dyn Clock) -> bool {
        let now = clock.mono();
        match cmd {
            TimerCommand::Start { label, secs } => {
                let Some(secs) = sane_secs(secs) else {
                    return false;
                };
                if self.timers.len() >= MAX_TIMERS {
                    // Make room by dropping the oldest finished timer, else refuse.
                    match self.timers.iter().position(|t| t.run == TimerRun::Finished) {
                        Some(i) => {
                            self.timers.remove(i);
                        }
                        None => return false,
                    }
                }
                let id = self.next_id;
                self.next_id += 1;
                let label = if label.trim().is_empty() {
                    default_label(secs)
                } else {
                    label.chars().take(60).collect()
                };
                self.timers.push(Countdown {
                    id,
                    label,
                    duration: secs,
                    run: TimerRun::Running,
                    deadline_mono: now + secs,
                    remaining: secs,
                    finished_mono: 0.0,
                });
                true
            }
            TimerCommand::Pause(id) => self.with_timer(id, |t| {
                if t.run != TimerRun::Running {
                    return false;
                }
                t.remaining = (t.deadline_mono - now).max(0.0);
                t.run = TimerRun::Paused;
                true
            }),
            TimerCommand::Resume(id) => self.with_timer(id, |t| {
                if t.run != TimerRun::Paused {
                    return false;
                }
                t.deadline_mono = now + t.remaining;
                t.run = TimerRun::Running;
                true
            }),
            TimerCommand::Cancel(id) => {
                let before = self.timers.len();
                self.timers.retain(|t| t.id != id);
                before != self.timers.len()
            }
            TimerCommand::AddTime { id, secs } => {
                if !secs.is_finite() || secs == 0.0 {
                    return false;
                }
                self.with_timer(id, |t| {
                    match t.run {
                        TimerRun::Running => {
                            let left = (t.deadline_mono - now + secs).clamp(0.0, MAX_TIMER_SECS);
                            t.deadline_mono = now + left;
                        }
                        TimerRun::Paused => {
                            t.remaining = (t.remaining + secs).clamp(0.0, MAX_TIMER_SECS)
                        }
                        TimerRun::Finished => {
                            // "+1 min" on a rung timer restarts it for that long.
                            if secs <= 0.0 {
                                return false;
                            }
                            t.run = TimerRun::Running;
                            t.deadline_mono = now + secs.min(MAX_TIMER_SECS);
                        }
                    }
                    t.duration = (t.duration + secs).clamp(1.0, MAX_TIMER_SECS);
                    true
                })
            }
            TimerCommand::Rename { id, label } => self.with_timer(id, |t| {
                t.label = label.chars().take(60).collect();
                true
            }),
            TimerCommand::ClearFinished => {
                let before = self.timers.len();
                self.timers.retain(|t| t.run != TimerRun::Finished);
                before != self.timers.len()
            }
            TimerCommand::StopwatchToggle => {
                let w = &mut self.stopwatch;
                if w.running {
                    w.accumulated += now - w.started_mono;
                    w.running = false;
                } else {
                    w.started_mono = now;
                    w.running = true;
                }
                true
            }
            TimerCommand::StopwatchLap => {
                if !self.stopwatch.running {
                    return false;
                }
                let e = self.stopwatch_elapsed(now);
                let w = &mut self.stopwatch;
                if w.laps.len() >= MAX_LAPS {
                    w.laps.remove(0);
                }
                w.laps.push(e);
                true
            }
            TimerCommand::StopwatchReset => {
                self.stopwatch = Watch::default();
                true
            }
            TimerCommand::FocusStart => {
                if self.focus.is_some() {
                    return false;
                }
                let secs = self.phase_secs(FocusPhase::Work);
                self.focus = Some(Focus {
                    phase: FocusPhase::Work,
                    round: 1,
                    completed_work: 0,
                    phase_secs: secs,
                    paused: false,
                    deadline_mono: now + secs,
                    remaining: secs,
                });
                true
            }
            TimerCommand::FocusPauseResume => {
                let Some(f) = self.focus.as_mut() else {
                    return false;
                };
                if f.paused {
                    f.deadline_mono = now + f.remaining;
                    f.paused = false;
                } else {
                    f.remaining = (f.deadline_mono - now).max(0.0);
                    f.paused = true;
                }
                true
            }
            TimerCommand::FocusSkip => {
                if self.focus.is_none() {
                    return false;
                }
                // Skipping never rings: the user chose to move on.
                let _ = self.advance_focus(now, false);
                true
            }
            TimerCommand::FocusStop => self.focus.take().is_some(),
        }
    }

    fn with_timer(&mut self, id: u64, f: impl FnOnce(&mut Countdown) -> bool) -> bool {
        self.timers.iter_mut().find(|t| t.id == id).is_some_and(f)
    }

    fn stopwatch_elapsed(&self, now: f64) -> f64 {
        let w = &self.stopwatch;
        w.accumulated + if w.running { now - w.started_mono } else { 0.0 }
    }

    fn phase_secs(&self, phase: FocusPhase) -> f64 {
        let min = match phase {
            FocusPhase::Work => self.settings.focus_work_min,
            FocusPhase::ShortBreak => self.settings.focus_short_break_min,
            FocusPhase::LongBreak => self.settings.focus_long_break_min,
        };
        f64::from(min.clamp(1, 240)) * 60.0
    }

    /// Ends the current focus phase. Returns the event to announce.
    fn advance_focus(&mut self, now: f64, auto: bool) -> Option<TimerEvent> {
        let rounds = self.settings.focus_rounds.clamp(1, 24);
        let every = self.settings.focus_long_break_every.clamp(1, 24);
        let next = {
            let f = self.focus.as_mut()?;
            match f.phase {
                FocusPhase::Work => {
                    f.completed_work += 1;
                    if f.completed_work >= rounds {
                        None
                    } else if f.completed_work.is_multiple_of(every) {
                        Some((FocusPhase::LongBreak, f.round))
                    } else {
                        Some((FocusPhase::ShortBreak, f.round))
                    }
                }
                FocusPhase::ShortBreak | FocusPhase::LongBreak => {
                    Some((FocusPhase::Work, f.completed_work + 1))
                }
            }
        };
        match next {
            None => {
                let done = self.focus.take().map(|f| f.completed_work).unwrap_or(0);
                Some(TimerEvent::FocusCycleComplete { rounds: done })
            }
            Some((phase, round)) => {
                let secs = self.phase_secs(phase);
                let pause = auto && !self.settings.focus_auto_advance;
                let f = self.focus.as_mut()?;
                f.phase = phase;
                f.round = round;
                f.phase_secs = secs;
                f.remaining = secs;
                f.paused = pause;
                f.deadline_mono = now + secs;
                Some(TimerEvent::FocusPhase { phase, round })
            }
        }
    }

    /// Fires due deadlines. Returns events; state changed iff non-empty or
    /// lingering timers expired (second value).
    pub fn tick(&mut self, clock: &dyn Clock) -> (Vec<TimerEvent>, bool) {
        let now = clock.mono();
        let mut events = Vec::new();
        for t in &mut self.timers {
            if t.run == TimerRun::Running && t.deadline_mono <= now {
                t.run = TimerRun::Finished;
                t.remaining = 0.0;
                t.finished_mono = now;
                events.push(TimerEvent::Done {
                    label: t.label.clone(),
                    late: false,
                });
            }
        }
        let before = self.timers.len();
        self.timers.retain(|t| {
            !(t.run == TimerRun::Finished && now - t.finished_mono >= FINISHED_LINGER_SECS)
        });
        let expired = before != self.timers.len();
        if self
            .focus
            .as_ref()
            .is_some_and(|f| !f.paused && f.deadline_mono <= now)
            && let Some(e) = self.advance_focus(now, true)
        {
            events.push(e);
        }
        let changed = !events.is_empty() || expired;
        (events, changed)
    }

    pub fn snapshot(&self, clock: &dyn Clock) -> TimersState {
        let now = clock.mono();
        let wall = clock.wall();
        TimersState {
            timers: self
                .timers
                .iter()
                .map(|t| {
                    let remaining = match t.run {
                        TimerRun::Running => (t.deadline_mono - now).max(0.0),
                        TimerRun::Paused => t.remaining,
                        TimerRun::Finished => 0.0,
                    };
                    CountdownTimer {
                        id: t.id,
                        label: t.label.clone(),
                        duration_secs: t.duration,
                        remaining_secs: remaining,
                        observed_at: wall,
                        run: t.run,
                        ends_at: (t.run == TimerRun::Running).then_some(wall + remaining),
                    }
                })
                .collect(),
            stopwatch: Stopwatch {
                running: self.stopwatch.running,
                elapsed_secs: self.stopwatch_elapsed(now),
                observed_at: wall,
                laps: self.stopwatch.laps.clone(),
            },
            focus: self.focus.as_ref().map(|f| FocusSession {
                phase: f.phase,
                round: f.round,
                rounds_total: self.settings.focus_rounds.clamp(1, 24),
                remaining_secs: if f.paused {
                    f.remaining
                } else {
                    (f.deadline_mono - now).max(0.0)
                },
                phase_secs: f.phase_secs,
                observed_at: wall,
                paused: f.paused,
            }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.timers.is_empty()
            && !self.stopwatch.running
            && self.stopwatch.accumulated == 0.0
            && self.stopwatch.laps.is_empty()
            && self.focus.is_none()
    }

    pub fn persist(&self, clock: &dyn Clock) -> Persisted {
        let now = clock.mono();
        let wall = clock.wall();
        Persisted {
            version: 1,
            next_id: self.next_id,
            timers: self
                .timers
                .iter()
                .map(|t| PersistedTimer {
                    id: t.id,
                    label: t.label.clone(),
                    duration: t.duration,
                    run: t.run,
                    ends_at: (t.run == TimerRun::Running)
                        .then(|| wall + (t.deadline_mono - now).max(0.0)),
                    remaining: match t.run {
                        TimerRun::Paused => t.remaining,
                        _ => 0.0,
                    },
                })
                .collect(),
            stopwatch_running: self.stopwatch.running,
            stopwatch_elapsed: self.stopwatch_elapsed(now),
            saved_at: wall,
            laps: self.stopwatch.laps.clone(),
            focus: self.focus.as_ref().map(|f| PersistedFocus {
                phase: f.phase,
                round: f.round,
                completed_work: f.completed_work,
                phase_secs: f.phase_secs,
                paused: f.paused,
                ends_at: (!f.paused).then(|| wall + (f.deadline_mono - now).max(0.0)),
                remaining: f.remaining,
            }),
        }
    }

    /// Rebuilds state saved by a previous run. Timers that ended while Nox was
    /// closed are reported as late `Done` events.
    pub fn restore(&mut self, p: Persisted, clock: &dyn Clock) -> Vec<TimerEvent> {
        let now = clock.mono();
        let wall = clock.wall();
        let mut events = Vec::new();
        self.next_id = p.next_id.max(1);
        self.timers.clear();
        for t in p.timers.into_iter().take(MAX_TIMERS) {
            let Some(duration) = sane_secs(t.duration) else {
                continue;
            };
            let mut c = Countdown {
                id: t.id,
                label: t.label,
                duration,
                run: t.run,
                deadline_mono: 0.0,
                remaining: t.remaining.clamp(0.0, MAX_TIMER_SECS),
                finished_mono: now,
            };
            if t.run == TimerRun::Running {
                let left = t.ends_at.map(|e| e - wall).unwrap_or(0.0);
                if left > 0.0 {
                    c.deadline_mono = now + left.min(MAX_TIMER_SECS);
                } else {
                    c.run = TimerRun::Finished;
                    c.remaining = 0.0;
                    // Older than the linger window: forget it silently.
                    if -left >= FINISHED_LINGER_SECS {
                        continue;
                    }
                    c.finished_mono = now + left;
                    events.push(TimerEvent::Done {
                        label: c.label.clone(),
                        late: true,
                    });
                }
            } else if t.run == TimerRun::Finished {
                continue;
            }
            self.next_id = self.next_id.max(c.id + 1);
            self.timers.push(c);
        }
        let gap = (wall - p.saved_at).max(0.0);
        self.stopwatch = Watch {
            running: p.stopwatch_running,
            accumulated: p.stopwatch_elapsed.max(0.0) + if p.stopwatch_running { gap } else { 0.0 },
            started_mono: now,
            laps: p.laps.into_iter().take(MAX_LAPS).collect(),
        };
        self.focus = p.focus.map(|f| {
            let left = match (f.paused, f.ends_at) {
                (false, Some(e)) => e - wall,
                _ => f.remaining,
            };
            Focus {
                phase: f.phase,
                round: f.round.max(1),
                completed_work: f.completed_work,
                phase_secs: f.phase_secs,
                // A phase that ended while Nox was closed waits for the user
                // instead of silently running through several phases.
                paused: f.paused || left <= 0.0,
                deadline_mono: now + left.max(0.0),
                remaining: left.max(0.0),
            }
        });
        events
    }
}

fn default_label(secs: f64) -> String {
    let s = secs.round() as u64;
    if s.is_multiple_of(3600) {
        format!("{} h", s / 3600)
    } else if s.is_multiple_of(60) {
        format!("{} min", s / 60)
    } else if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

impl TimerEngine {
    /// For the 1 Hz publisher: is there anything to show at all?
    pub fn needs_publish_loop(&self) -> bool {
        self.is_active() || self.has_lingering()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::test::ManualClock;

    fn engine() -> TimerEngine {
        TimerEngine::new(TimerSettings {
            focus_work_min: 25,
            focus_short_break_min: 5,
            focus_long_break_min: 15,
            focus_rounds: 4,
            focus_long_break_every: 2,
            ..Default::default()
        })
    }

    #[test]
    fn countdown_rings_once_at_deadline() {
        let c = ManualClock::new(1_000.0);
        let mut e = engine();
        assert!(e.command(
            TimerCommand::Start {
                label: String::new(),
                secs: 60.0
            },
            &*c
        ));
        assert_eq!(e.snapshot(&*c).timers[0].label, "1 min");
        c.advance(59.5);
        assert!(e.tick(&*c).0.is_empty());
        assert!((e.next_deadline(&*c).unwrap() - 0.5).abs() < 1e-9);
        c.advance(0.5);
        let (ev, changed) = e.tick(&*c);
        assert!(changed);
        assert_eq!(
            ev,
            vec![TimerEvent::Done {
                label: "1 min".into(),
                late: false
            }]
        );
        assert!(e.tick(&*c).0.is_empty(), "rings only once");
        assert_eq!(e.snapshot(&*c).timers[0].run, TimerRun::Finished);
        assert!(!e.is_active());
        c.advance(FINISHED_LINGER_SECS);
        assert!(e.tick(&*c).1);
        assert!(e.snapshot(&*c).timers.is_empty());
    }

    #[test]
    fn pause_resume_and_add_time() {
        let c = ManualClock::new(0.0);
        let mut e = engine();
        e.command(
            TimerCommand::Start {
                label: "Thé".into(),
                secs: 100.0,
            },
            &*c,
        );
        c.advance(30.0);
        assert!(e.command(TimerCommand::Pause(1), &*c));
        c.advance(1000.0);
        assert!(e.tick(&*c).0.is_empty(), "paused timers do not ring");
        assert_eq!(e.snapshot(&*c).timers[0].remaining_secs, 70.0);
        assert!(e.command(TimerCommand::Resume(1), &*c));
        assert!(e.command(TimerCommand::AddTime { id: 1, secs: 60.0 }, &*c));
        c.advance(129.0);
        assert!(e.tick(&*c).0.is_empty());
        c.advance(1.0);
        assert_eq!(e.tick(&*c).0.len(), 1);
        // +1 min on a finished timer restarts it.
        assert!(e.command(TimerCommand::AddTime { id: 1, secs: 60.0 }, &*c));
        assert_eq!(e.snapshot(&*c).timers[0].run, TimerRun::Running);
        assert!(!e.command(TimerCommand::Pause(42), &*c), "unknown id");
    }

    #[test]
    fn wall_clock_jump_does_not_affect_countdown() {
        let c = ManualClock::new(0.0);
        let mut e = engine();
        e.command(
            TimerCommand::Start {
                label: "x".into(),
                secs: 10.0,
            },
            &*c,
        );
        c.jump_wall(3600.0);
        assert!(e.tick(&*c).0.is_empty());
        c.advance(10.0);
        assert_eq!(e.tick(&*c).0.len(), 1);
    }

    #[test]
    fn stopwatch_laps() {
        let c = ManualClock::new(0.0);
        let mut e = engine();
        assert!(!e.command(TimerCommand::StopwatchLap, &*c), "not running");
        e.command(TimerCommand::StopwatchToggle, &*c);
        c.advance(5.0);
        e.command(TimerCommand::StopwatchLap, &*c);
        c.advance(2.5);
        e.command(TimerCommand::StopwatchToggle, &*c);
        c.advance(100.0);
        let s = e.snapshot(&*c).stopwatch;
        assert_eq!(s.elapsed_secs, 7.5);
        assert_eq!(s.laps, vec![5.0]);
        assert!(!s.running);
        e.command(TimerCommand::StopwatchReset, &*c);
        assert!(e.is_empty());
    }

    #[test]
    fn pomodoro_cycle() {
        let c = ManualClock::new(0.0);
        let mut e = engine();
        e.command(TimerCommand::FocusStart, &*c);
        let mut phases = Vec::new();
        for _ in 0..10 {
            let left = e.next_deadline(&*c).unwrap();
            c.advance(left);
            for ev in e.tick(&*c).0 {
                phases.push(ev);
            }
            if e.snapshot(&*c).focus.is_none() {
                break;
            }
        }
        use FocusPhase::*;
        assert_eq!(
            phases,
            vec![
                TimerEvent::FocusPhase {
                    phase: ShortBreak,
                    round: 1
                },
                TimerEvent::FocusPhase {
                    phase: Work,
                    round: 2
                },
                TimerEvent::FocusPhase {
                    phase: LongBreak,
                    round: 2
                },
                TimerEvent::FocusPhase {
                    phase: Work,
                    round: 3
                },
                TimerEvent::FocusPhase {
                    phase: ShortBreak,
                    round: 3
                },
                TimerEvent::FocusPhase {
                    phase: Work,
                    round: 4
                },
                TimerEvent::FocusCycleComplete { rounds: 4 },
            ]
        );
        // Durations follow the settings.
        e.command(TimerCommand::FocusStart, &*c);
        assert_eq!(e.snapshot(&*c).focus.unwrap().phase_secs, 25.0 * 60.0);
        e.command(TimerCommand::FocusSkip, &*c);
        assert_eq!(e.snapshot(&*c).focus.unwrap().phase_secs, 5.0 * 60.0);
        e.command(TimerCommand::FocusPauseResume, &*c);
        c.advance(3600.0);
        assert!(e.tick(&*c).0.is_empty(), "paused focus does not advance");
        assert!(e.command(TimerCommand::FocusStop, &*c));
        assert!(!e.is_active());
    }

    #[test]
    fn manual_advance_waits_when_auto_advance_is_off() {
        let c = ManualClock::new(0.0);
        let mut e = TimerEngine::new(TimerSettings {
            focus_auto_advance: false,
            ..Default::default()
        });
        e.command(TimerCommand::FocusStart, &*c);
        c.advance(25.0 * 60.0);
        assert_eq!(e.tick(&*c).0.len(), 1);
        let f = e.snapshot(&*c).focus.unwrap();
        assert!(f.paused);
        assert_eq!(f.phase, FocusPhase::ShortBreak);
    }

    #[test]
    fn survives_restart() {
        let c = ManualClock::new(10_000.0);
        let mut e = engine();
        e.command(
            TimerCommand::Start {
                label: "a".into(),
                secs: 300.0,
            },
            &*c,
        );
        e.command(
            TimerCommand::Start {
                label: "b".into(),
                secs: 20.0,
            },
            &*c,
        );
        e.command(
            TimerCommand::Start {
                label: "c".into(),
                secs: 50.0,
            },
            &*c,
        );
        e.command(TimerCommand::Pause(3), &*c);
        e.command(TimerCommand::StopwatchToggle, &*c);
        e.command(TimerCommand::FocusStart, &*c);
        c.advance(10.0);
        let saved = serde_json::to_string(&e.persist(&*c)).unwrap();

        // "Quit", 60 s pass, new process with a fresh monotonic clock.
        let c2 = ManualClock::new(10_070.0);
        let mut e2 = engine();
        let late = e2.restore(serde_json::from_str(&saved).unwrap(), &*c2);
        assert_eq!(
            late,
            vec![TimerEvent::Done {
                label: "b".into(),
                late: true
            }]
        );
        let s = e2.snapshot(&*c2);
        let a = s.timers.iter().find(|t| t.label == "a").unwrap();
        assert!((a.remaining_secs - 230.0).abs() < 1e-6);
        let cc = s.timers.iter().find(|t| t.label == "c").unwrap();
        assert_eq!((cc.run, cc.remaining_secs), (TimerRun::Paused, 50.0));
        assert!((s.stopwatch.elapsed_secs - 70.0).abs() < 1e-6);
        let f = s.focus.unwrap();
        assert!((f.remaining_secs - (25.0 * 60.0 - 70.0)).abs() < 1e-6);
        // New ids never collide with restored ones.
        e2.command(
            TimerCommand::Start {
                label: "d".into(),
                secs: 5.0,
            },
            &*c2,
        );
        let ids: Vec<u64> = e2.snapshot(&*c2).timers.iter().map(|t| t.id).collect();
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(ids.len(), dedup.len());
        assert!(ids.contains(&4));
    }

    #[test]
    fn rejects_bad_input() {
        let c = ManualClock::new(0.0);
        let mut e = engine();
        for secs in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert!(!e.command(
                TimerCommand::Start {
                    label: "x".into(),
                    secs
                },
                &*c
            ));
        }
        assert!(e.is_empty());
    }
}
