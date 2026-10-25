//! Permission states shared between modules and the UI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    Calendars,
    Camera,
    Accessibility,
    /// Automation (Apple Events) towards a given app — tracked per target.
    Automation,
    FullDiskAccess,
    ScreenRecording,
    Bluetooth,
    Notifications,
    /// System audio recording (Core Audio process taps: live levels, mixer).
    SystemAudio,
    /// Administrator approval to install the closed-lid helper (one time).
    AdminHelperInstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PermissionState {
    #[default]
    NotDetermined,
    Granted,
    Denied,
    Restricted,
    /// Not needed by the current configuration.
    NotRequired,
}

impl Permission {
    pub fn label_fr(self) -> &'static str {
        match self {
            Permission::Calendars => "Calendriers",
            Permission::Camera => "Caméra",
            Permission::Accessibility => "Accessibilité",
            Permission::Automation => "Automatisation",
            Permission::FullDiskAccess => "Accès complet au disque",
            Permission::ScreenRecording => "Enregistrement de l’écran",
            Permission::Bluetooth => "Bluetooth",
            Permission::Notifications => "Notifications",
            Permission::SystemAudio => "Enregistrement audio du système",
            Permission::AdminHelperInstall => "Assistant administrateur (capot fermé)",
        }
    }
    /// `x-apple.systempreferences:` URL that opens the relevant privacy pane.
    pub fn settings_url(self) -> &'static str {
        match self {
            Permission::Calendars => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars"
            }
            Permission::Camera => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
            }
            Permission::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Permission::Automation => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"
            }
            Permission::FullDiskAccess => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
            }
            Permission::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            Permission::Bluetooth => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Bluetooth"
            }
            Permission::Notifications => {
                "x-apple.systempreferences:com.apple.Notifications-Settings.extension"
            }
            // Anchor not verified on macOS 27 (no tap was ever created here).
            Permission::SystemAudio => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture"
            }
            Permission::AdminHelperInstall => {
                "x-apple.systempreferences:com.apple.LoginItems-Settings.extension"
            }
        }
    }
}
