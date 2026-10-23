//! Keep-awake state. Three distinct modes — do not conflate idle-sleep
//! prevention with lid-close sleep prevention.

use super::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeepAwakeMode {
    /// PreventUserIdleSystemSleep: Mac stays awake, display may sleep.
    System,
    /// + PreventUserIdleDisplaySleep: Mac and display stay awake.
    SystemAndDisplay,
    /// System assertion + privileged helper sets lid-close sleep disabled.
    ClosedLid,
}

impl KeepAwakeMode {
    pub fn label_fr(self) -> &'static str {
        match self {
            Self::System => "Mac actif (écran peut s’éteindre)",
            Self::SystemAndDisplay => "Mac et écran actifs",
            Self::ClosedLid => "Mac actif capot fermé",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeepAwakeDuration {
    Minutes(u32),
    Unlimited,
}

impl KeepAwakeDuration {
    pub const PRESETS: [KeepAwakeDuration; 5] = [
        Self::Minutes(15),
        Self::Minutes(30),
        Self::Minutes(60),
        Self::Minutes(120),
        Self::Unlimited,
    ];
    pub fn label_fr(self) -> String {
        match self {
            Self::Minutes(m) if m < 60 => format!("{m} min"),
            Self::Minutes(m) => format!("{} h", m / 60),
            Self::Unlimited => "Illimité".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperStatus {
    NotInstalled,
    Installed,
    /// Installed but a different version than this app expects.
    Outdated,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    User,
    Expired,
    OnBattery,
    BatteryThreshold,
    Error,
    AppQuit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeepAwakeSession {
    pub mode: KeepAwakeMode,
    pub started_at: Timestamp,
    pub ends_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerState {
    pub active: Option<KeepAwakeSession>,
    pub helper: HelperStatus,
    /// Lid-close sleep currently disabled system-wide (read from pmset).
    pub lid_sleep_disabled: Option<bool>,
    pub last_stop: Option<StopReason>,
    pub last_error: Option<String>,
}

impl Default for PowerState {
    fn default() -> Self {
        Self {
            active: None,
            helper: HelperStatus::NotInstalled,
            lid_sleep_disabled: None,
            last_stop: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PowerCommand {
    Start {
        mode: KeepAwakeMode,
        duration: KeepAwakeDuration,
    },
    Stop,
    InstallHelper,
    UninstallHelper,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerSettings {
    pub default_mode: KeepAwakeMode,
    pub default_duration: KeepAwakeDuration,
    pub only_on_ac: bool,
    /// Stop when battery drops to this percent (0 = off).
    pub stop_below_percent: u8,
}

impl Default for PowerSettings {
    fn default() -> Self {
        Self {
            default_mode: KeepAwakeMode::System,
            default_duration: KeepAwakeDuration::Minutes(60),
            only_on_ac: false,
            stop_below_percent: 20,
        }
    }
}
