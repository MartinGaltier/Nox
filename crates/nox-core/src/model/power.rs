//! Keep-awake state. Three distinct modes — do not conflate idle-sleep
//! prevention with lid-close sleep prevention.
//!
//! IOPM assertions (`System`, `SystemAndDisplay`) only prevent *idle* sleep.
//! They do not keep a MacBook awake when its lid closes without an external
//! display: that needs `pmset disablesleep 1`, which only root can set, hence
//! the privileged helper used by `ClosedLid` (see docs/SLEEP_RECOVERY.md).

use super::Timestamp;
use crate::reading::Reading;
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
    pub const ALL: [KeepAwakeMode; 3] = [Self::System, Self::SystemAndDisplay, Self::ClosedLid];

    pub fn label_fr(self) -> &'static str {
        match self {
            Self::System => "Mac actif (écran peut s’éteindre)",
            Self::SystemAndDisplay => "Mac et écran actifs",
            Self::ClosedLid => "Mac actif capot fermé",
        }
    }
    /// SF Symbol for the mode indicator.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::System => "cup.and.saucer",
            Self::SystemAndDisplay => "sun.max",
            Self::ClosedLid => "laptopcomputer",
        }
    }
    /// Whether this mode needs the privileged helper.
    pub fn needs_helper(self) -> bool {
        matches!(self, Self::ClosedLid)
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
            Self::Minutes(m) if m % 60 == 0 => format!("{} h", m / 60),
            Self::Minutes(m) => format!("{} h {:02}", m / 60, m % 60),
            Self::Unlimited => "Illimité".into(),
        }
    }
    /// `None` for unlimited. `Minutes(0)` is treated as unlimited.
    pub fn as_secs(self) -> Option<u64> {
        match self {
            Self::Minutes(0) | Self::Unlimited => None,
            Self::Minutes(m) => Some(u64::from(m) * 60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperStatus {
    NotInstalled,
    Installed,
    /// Installed but a different version than this app expects.
    Outdated,
    /// Installed (files present) but the socket does not answer.
    Unreachable,
    /// The helper answers but refuses this client (app moved since the helper
    /// was installed, or another user's session). Reinstall from this app.
    ClientRejected,
}

impl HelperStatus {
    pub fn label_fr(self) -> &'static str {
        match self {
            Self::NotInstalled => "Assistant non installé",
            Self::Installed => "Assistant installé",
            Self::Outdated => "Assistant à mettre à jour",
            Self::Unreachable => "Assistant injoignable",
            Self::ClientRejected => "Assistant installé pour une autre copie de Nox",
        }
    }
    /// Whether the closed-lid mode can be started right now.
    pub fn usable(self) -> bool {
        matches!(self, Self::Installed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    User,
    Expired,
    /// `only_on_ac` and the Mac switched to battery.
    OnBattery,
    /// On battery at or below `stop_below_percent`.
    BatteryThreshold,
    /// The helper restored lid sleep because of heavy thermal pressure.
    Thermal,
    /// The helper became unreachable or revoked the closed-lid lease.
    HelperLost,
    Error,
    AppQuit,
}

impl StopReason {
    pub fn label_fr(self) -> &'static str {
        match self {
            Self::User => "Arrêté",
            Self::Expired => "Durée écoulée",
            Self::OnBattery => "Passage sur batterie",
            Self::BatteryThreshold => "Seuil de batterie atteint",
            Self::Thermal => "Température élevée",
            Self::HelperLost => "Assistant capot fermé interrompu",
            Self::Error => "Erreur",
            Self::AppQuit => "Nox quitté",
        }
    }
    /// Stops the user did not ask for (they get an island alert).
    pub fn is_automatic(self) -> bool {
        !matches!(self, Self::User | Self::AppQuit)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeepAwakeSession {
    pub mode: KeepAwakeMode,
    pub duration: KeepAwakeDuration,
    pub started_at: Timestamp,
    /// `None` = unlimited.
    pub ends_at: Option<Timestamp>,
}

impl KeepAwakeSession {
    /// Seconds left at `now` (clamped at 0), `None` when unlimited.
    pub fn remaining_secs(&self, now: Timestamp) -> Option<f64> {
        self.ends_at.map(|e| (e - now).max(0.0))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerState {
    pub active: Option<KeepAwakeSession>,
    pub helper: HelperStatus,
    /// An install/uninstall of the helper is in progress (admin prompt shown).
    pub helper_busy: bool,
    /// Lid-close sleep currently disabled system-wide (`SleepDisabled`, read
    /// from the IOPMrootDomain registry entry — no privileges needed). It can
    /// be set by Nox's helper or by the user (`sudo pmset disablesleep 1`).
    pub lid_sleep_disabled: Reading<bool>,
    pub last_stop: Option<StopReason>,
    pub last_error: Option<String>,
    /// One-off information for the user, e.g. what launch recovery repaired.
    pub notice: Option<String>,
}

impl Default for PowerState {
    fn default() -> Self {
        Self {
            active: None,
            helper: HelperStatus::NotInstalled,
            helper_busy: false,
            lid_sleep_disabled: Reading::pending(),
            last_stop: None,
            last_error: None,
            notice: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PowerCommand {
    /// Starts (or replaces) a session.
    Start {
        mode: KeepAwakeMode,
        duration: KeepAwakeDuration,
    },
    Stop,
    /// Stops if active, else starts with `PowerSettings` defaults (menu bar,
    /// shortcut).
    Toggle,
    /// Explicit user action only: shows the macOS administrator prompt once.
    InstallHelper,
    UninstallHelper,
    /// Re-reads helper status and `SleepDisabled`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(KeepAwakeDuration::Minutes(15).as_secs(), Some(900));
        assert_eq!(KeepAwakeDuration::Unlimited.as_secs(), None);
        assert_eq!(KeepAwakeDuration::Minutes(0).as_secs(), None);
        assert_eq!(KeepAwakeDuration::Minutes(120).label_fr(), "2 h");
        assert_eq!(KeepAwakeDuration::Minutes(90).label_fr(), "1 h 30");
    }

    #[test]
    fn remaining_is_clamped() {
        let s = KeepAwakeSession {
            mode: KeepAwakeMode::System,
            duration: KeepAwakeDuration::Minutes(15),
            started_at: 100.0,
            ends_at: Some(1000.0),
        };
        assert_eq!(s.remaining_secs(400.0), Some(600.0));
        assert_eq!(s.remaining_secs(2000.0), Some(0.0));
    }
}
