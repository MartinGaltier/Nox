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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stopwatch {
    pub running: bool,
    pub elapsed_secs: f64,
    pub observed_at: Timestamp,
    pub laps: Vec<f64>,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimersState {
    pub timers: Vec<CountdownTimer>,
    pub stopwatch: Stopwatch,
    pub focus: Option<FocusSession>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TimerCommand {
    Start { label: String, secs: f64 },
    Pause(u64),
    Resume(u64),
    Cancel(u64),
    AddTime { id: u64, secs: f64 },
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
    pub focus_rounds: u32,
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
            sound: true,
        }
    }
}
