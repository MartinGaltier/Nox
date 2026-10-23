use super::Timestamp;
use crate::permissions::PermissionState;
use serde::{Deserialize, Serialize};

/// A screen rectangle in global (top-left origin) points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchTrigger {
    /// Alert on any visible change.
    AnyChange,
    /// Alert when the region stops changing for `settle_secs` (activity finished).
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchPhase {
    Idle,
    Selecting,
    Watching,
    Changing,
    Triggered,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchState {
    pub access: PermissionState,
    pub phase: WatchPhase,
    pub region: Option<Rect>,
    /// Owning window title / app if the region was picked on a window.
    pub target: Option<String>,
    pub trigger: WatchTrigger,
    pub last_change_at: Option<Timestamp>,
    /// 0..1 fraction of pixels changed on the last comparison.
    pub last_diff: Option<f64>,
}

impl Default for WatchState {
    fn default() -> Self {
        Self {
            access: PermissionState::NotDetermined,
            phase: WatchPhase::Idle,
            region: None,
            target: None,
            trigger: WatchTrigger::Settled,
            last_change_at: None,
            last_diff: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WatchCommand {
    BeginSelection,
    Start { region: Rect, trigger: WatchTrigger },
    Stop,
    RequestAccess,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchSettings {
    pub interval_ms: u64,
    pub settle_secs: u64,
    /// Fraction of changed pixels counted as a change.
    pub sensitivity: f64,
}

impl Default for WatchSettings {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            settle_secs: 8,
            sensitivity: 0.01,
        }
    }
}
