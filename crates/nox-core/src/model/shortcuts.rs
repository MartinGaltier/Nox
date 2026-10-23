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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShortcutsState {
    /// Names of shortcuts available in Shortcuts.app.
    pub user_shortcuts: Vec<String>,
    pub last_result: Option<Result<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShortcutCommand {
    Run(QuickAction),
    RefreshUserShortcuts,
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
