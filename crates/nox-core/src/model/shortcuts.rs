use crate::reading::Reading;
use serde::{Deserialize, Serialize};

/// Built-in quick actions available from the island.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuickAction {
    LockScreen,
    Screenshot,
    ToggleDarkMode,
    ToggleDoNotDisturb,
    OpenApp(String),
    /// Run a user shortcut from Shortcuts.app by name.
    RunShortcut(String),
    EmptyTrash,
    StartScreenSaver,
    OpenUrl(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShortcutEntry {
    pub label: String,
    /// SF Symbol name.
    pub symbol: String,
    pub action: QuickAction,
}

/// System controls (E7). `Unavailable(NotSupported)` when the current
/// device does not expose the control (e.g. HDMI output volume, a mic
/// without a mute switch, an external display's brightness).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ControlsState {
    /// Default output volume 0..1.
    pub volume: Reading<f32>,
    pub muted: Reading<bool>,
    /// Default input (microphone) muted.
    pub mic_muted: Reading<bool>,
    /// Built-in display brightness 0..1.
    pub brightness: Reading<f32>,
    pub output_device: Option<String>,
    pub input_device: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShortcutsState {
    /// Names of shortcuts available in Shortcuts.app.
    pub user_shortcuts: Vec<String>,
    pub last_result: Option<Result<String, String>>,
    pub controls: ControlsState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShortcutCommand {
    Run(QuickAction),
    RefreshUserShortcuts,
    /// Re-read controls (brightness has no change notification): send when
    /// the controls are shown.
    RefreshControls,
    SetVolume(f32),
    SetMuted(bool),
    SetMicMuted(bool),
    SetBrightness(f32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutSettings {
    pub entries: Vec<ShortcutEntry>,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        let e = |label: &str, symbol: &str, action| ShortcutEntry {
            label: label.into(),
            symbol: symbol.into(),
            action,
        };
        Self {
            entries: vec![
                e("Verrouiller", "lock.fill", QuickAction::LockScreen),
                e("Capture", "camera.viewfinder", QuickAction::Screenshot),
                e(
                    "Mode sombre",
                    "circle.lefthalf.filled",
                    QuickAction::ToggleDarkMode,
                ),
                e("Économiseur", "sparkles.tv", QuickAction::StartScreenSaver),
            ],
        }
    }
}
