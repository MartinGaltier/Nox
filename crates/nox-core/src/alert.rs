//! Transient island alerts ("live activities"): a download finished, an agent
//! completed, a timer rang, low battery, accessory connected, watched region
//! changed, calendar event soon, notification received.

use crate::model::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AlertPriority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertKind {
    DownloadFinished,
    TrackChanged,
    AgentFinished,
    AgentNeedsInput,
    AgentLimit,
    TimerDone,
    FocusPhase,
    LowBattery,
    PowerSource,
    AccessoryConnected,
    AccessoryLowBattery,
    WatchTriggered,
    CalendarSoon,
    Notification,
    KeepAwake,
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub id: u64,
    pub kind: AlertKind,
    pub priority: AlertPriority,
    pub title: String,
    pub subtitle: Option<String>,
    /// SF Symbol name.
    pub symbol: String,
    pub created_at: Timestamp,
    /// How long the island shows it before collapsing back.
    pub ttl_secs: f64,
    /// Optional 0..1 progress ring.
    pub progress: Option<f64>,
}

impl Alert {
    pub fn new(kind: AlertKind, title: impl Into<String>, symbol: impl Into<String>) -> Self {
        Self {
            id: 0,
            kind,
            priority: AlertPriority::Normal,
            title: title.into(),
            subtitle: None,
            symbol: symbol.into(),
            created_at: crate::model::now(),
            ttl_secs: 4.0,
            progress: None,
        }
    }
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }
    pub fn priority(mut self, p: AlertPriority) -> Self {
        self.priority = p;
        self
    }
    pub fn ttl(mut self, secs: f64) -> Self {
        self.ttl_secs = secs;
        self
    }
}
