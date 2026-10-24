use super::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimerRun {
    Running,
    Paused,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountdownTimer {
    pub id: u64,
    pub label: String,
    pub duration_secs: f64,
    /// Remaining at `observed_at`.
    pub remaining_secs: f64,
    pub observed_at: Timestamp,
    pub run: TimerRun,
    /// Wall-clock end while running (display only; exactness uses a monotonic clock).
    #[serde(default)]
    pub ends_at: Option<Timestamp>,
}

impl CountdownTimer {
    /// Remaining seconds at `now`, extrapolated while running.
    pub fn remaining_now(&self, now: Timestamp) -> f64 {
        match self.run {
            TimerRun::Running => (self.remaining_secs - (now - self.observed_at).max(0.0)).max(0.0),
            _ => self.remaining_secs,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stopwatch {
    pub running: bool,
    pub elapsed_secs: f64,
    pub observed_at: Timestamp,
    pub laps: Vec<f64>,
}

impl Stopwatch {
    pub fn elapsed_now(&self, now: Timestamp) -> f64 {
        if self.running {
            self.elapsed_secs + (now - self.observed_at).max(0.0)
        } else {
            self.elapsed_secs
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusPhase {
    Work,
    ShortBreak,
    LongBreak,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FocusSession {
    pub phase: FocusPhase,
    pub round: u32,
    pub rounds_total: u32,
    pub remaining_secs: f64,
    pub phase_secs: f64,
    pub observed_at: Timestamp,
    pub paused: bool,
}

impl FocusSession {
    pub fn remaining_now(&self, now: Timestamp) -> f64 {
        if self.paused {
            self.remaining_secs
        } else {
            (self.remaining_secs - (now - self.observed_at).max(0.0)).max(0.0)
        }
    }
}

impl FocusPhase {
    pub fn label_fr(self) -> &'static str {
        match self {
            FocusPhase::Work => "Concentration",
            FocusPhase::ShortBreak => "Pause courte",
            FocusPhase::LongBreak => "Pause longue",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimersState {
    pub timers: Vec<CountdownTimer>,
    pub stopwatch: Stopwatch,
    pub focus: Option<FocusSession>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TimerCommand {
    Start {
        label: String,
        secs: f64,
    },
    Pause(u64),
    Resume(u64),
    Cancel(u64),
    AddTime {
        id: u64,
        secs: f64,
    },
    /// Rename a timer.
    Rename {
        id: u64,
        label: String,
    },
    /// Remove finished timers.
    ClearFinished,
    StopwatchToggle,
    StopwatchLap,
    StopwatchReset,
    FocusStart,
    FocusPauseResume,
    FocusSkip,
    FocusStop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimerSettings {
    pub presets_secs: Vec<u64>,
    pub focus_work_min: u32,
    pub focus_short_break_min: u32,
    pub focus_long_break_min: u32,
    /// Work sessions in a full cycle.
    pub focus_rounds: u32,
    /// A long break comes after this many work sessions.
    pub focus_long_break_every: u32,
    /// Start the next phase automatically.
    pub focus_auto_advance: bool,
    pub sound: bool,
}

impl Default for TimerSettings {
    fn default() -> Self {
        Self {
            presets_secs: vec![60, 300, 600, 1500, 3600],
            focus_work_min: 25,
            focus_short_break_min: 5,
            focus_long_break_min: 15,
            focus_rounds: 4,
            focus_long_break_every: 4,
            focus_auto_advance: true,
            sound: true,
        }
    }
}
