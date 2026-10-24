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
    /// Built-in keyboard backlight 0..1.
    pub keyboard_backlight: Reading<f32>,
    /// Current keyboard input source and the enabled ones to switch to.
    pub input_source: Option<InputSourceInfo>,
    pub input_sources: Vec<InputSourceInfo>,
    /// E8: the last volume/brightness change, for an in-island HUD. The UI
    /// shows it for ~1.5 s after `at` (the system HUD is left untouched).
    pub hud: Option<ControlHud>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSourceInfo {
    /// `com.apple.keylayout.French`…
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HudKind {
    Volume,
    Brightness,
    Microphone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlHud {
    pub kind: HudKind,
    /// 0..1 (`None` when only a mute state changed).
    pub value: Option<f32>,
    pub muted: bool,
    pub at: crate::model::Timestamp,
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
    SetKeyboardBacklight(f32),
    /// Input source id (`InputSourceInfo::id`).
    SelectInputSource(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutSettings {
    pub entries: Vec<ShortcutEntry>,
    /// E8: publish `ControlsState::hud` on volume/brightness changes.
    pub hud_in_island: bool,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        let e = |label: &str, symbol: &str, action| ShortcutEntry {
            label: label.into(),
            symbol: symbol.into(),
            action,
        };
        Self {
            hud_in_island: true,
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
