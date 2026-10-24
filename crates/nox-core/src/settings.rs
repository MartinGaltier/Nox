//! Persistent preferences (JSON in Application Support). Missing fields take
//! defaults so older files keep loading.

use crate::model::{
    accessories::AccessorySettings, agents::AgentSettings, calendar::CalendarSettings,
    camera::CameraSettings, companion::CompanionSettings, downloads::DownloadSettings,
    metrics::MetricsSettings, music::MusicSettings, notifications::NotificationSettings,
    power::PowerSettings, shortcuts::ShortcutSettings, timers::TimerSettings, watch::WatchSettings,
};
use crate::module::ModuleId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Pages of the expanded island, in user order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Section {
    Music,
    Calendar,
    Notifications,
    Timers,
    Downloads,
    Agents,
    Watch,
    Camera,
    Files,
    Shortcuts,
    System,
    KeepAwake,
    Accessories,
}

impl Section {
    pub const DEFAULT_ORDER: [Section; 13] = [
        Section::Music,
        Section::Calendar,
        Section::Agents,
        Section::Timers,
        Section::Notifications,
        Section::Downloads,
        Section::Files,
        Section::System,
        Section::KeepAwake,
        Section::Accessories,
        Section::Watch,
        Section::Camera,
        Section::Shortcuts,
    ];
    pub fn label_fr(self) -> &'static str {
        match self {
            Section::Music => "Musique",
            Section::Calendar => "Calendrier",
            Section::Notifications => "Notifications",
            Section::Timers => "Minuteurs",
            Section::Downloads => "Téléchargements",
            Section::Agents => "Agents IA",
            Section::Watch => "Surveillance",
            Section::Camera => "Caméra",
            Section::Files => "Fichiers",
            Section::Shortcuts => "Raccourcis",
            Section::System => "Système",
            Section::KeepAwake => "Maintien éveillé",
            Section::Accessories => "Accessoires",
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Section::Music => "music.note",
            Section::Calendar => "calendar",
            Section::Notifications => "bell",
            Section::Timers => "timer",
            Section::Downloads => "arrow.down.circle",
            Section::Agents => "sparkles",
            Section::Watch => "eye",
            Section::Camera => "web.camera",
            Section::Files => "tray.full",
            Section::Shortcuts => "square.grid.2x2",
            Section::System => "gauge.with.dots.needle.50percent",
            Section::KeepAwake => "cup.and.saucer",
            Section::Accessories => "headphones",
        }
    }
    /// The module that feeds this section.
    pub fn module(self) -> ModuleId {
        match self {
            Section::Music => ModuleId::Music,
            Section::Calendar => ModuleId::Calendar,
            Section::Notifications => ModuleId::Notifications,
            Section::Timers => ModuleId::Timers,
            Section::Downloads => ModuleId::Downloads,
            Section::Agents => ModuleId::Agents,
            Section::Watch => ModuleId::Watch,
            Section::Camera => ModuleId::Camera,
            Section::Files => ModuleId::Files,
            Section::Shortcuts => ModuleId::Shortcuts,
            Section::System => ModuleId::Metrics,
            Section::KeepAwake => ModuleId::Power,
            Section::Accessories => ModuleId::Accessories,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionConfig {
    pub section: Section,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayChoice {
    /// Built-in display if present, else main display.
    Automatic,
    BuiltIn,
    Main,
}

/// Expanded island preset (A17). `Custom` uses `IslandSettings::width`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IslandSize {
    Compact,
    Spacious,
    Custom,
}

/// Shape drawn on displays without a camera housing (A6/A17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IslandSilhouette {
    /// Hangs from the top edge like a real notch.
    Notch,
    /// Floats inside the menu bar.
    Capsule,
}

/// What the closed island shows when no activity is running (A20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleContent {
    Nothing,
    Battery,
    Music,
    Agents,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IslandSettings {
    pub sections: Vec<SectionConfig>,
    pub expand_on_hover: bool,
    pub hover_delay_ms: u64,
    pub display: DisplayChoice,
    /// Hide while a fullscreen app is frontmost on the island's display.
    pub hide_in_fullscreen: bool,
    /// Draw a simulated notch on displays without one.
    pub simulate_notch: bool,
    /// Horizontal swipe changes section; vertical scroll changes volume etc.
    pub gestures: bool,
    pub show_alerts: bool,
    pub width: f64,
    /// Pointer must stay outside this long before the island closes.
    pub close_delay_ms: u64,
    /// Exclude the island from screenshots and screen sharing (A15).
    pub hide_in_captures: bool,
    /// Draw nothing at rest until the pointer reaches the island (A16).
    pub hide_until_hover: bool,
    pub size: IslandSize,
    pub silhouette: IslandSilhouette,
    pub outline: bool,
    pub translucent: bool,
    pub haptics: bool,
    pub idle_content: IdleContent,
    /// Global shortcut (⌃⌥Espace) toggles the island (A19).
    pub shortcut_enabled: bool,
}

impl Default for IslandSettings {
    fn default() -> Self {
        Self {
            sections: Section::DEFAULT_ORDER
                .iter()
                .map(|&section| SectionConfig {
                    section,
                    enabled: true,
                })
                .collect(),
            expand_on_hover: true,
            hover_delay_ms: 250,
            display: DisplayChoice::Automatic,
            hide_in_fullscreen: true,
            simulate_notch: true,
            gestures: true,
            show_alerts: true,
            width: 520.0,
            close_delay_ms: 400,
            hide_in_captures: false,
            hide_until_hover: false,
            size: IslandSize::Spacious,
            silhouette: IslandSilhouette::Capsule,
            outline: false,
            translucent: false,
            haptics: true,
            idle_content: IdleContent::Music,
            shortcut_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub launch_at_login: bool,
    /// Explicit per-module switch. Missing = enabled unless the module needs
    /// an intrusive permission (see `default_enabled`).
    pub modules: BTreeMap<ModuleId, bool>,
    pub island: IslandSettings,
    pub music: MusicSettings,
    pub calendar: CalendarSettings,
    pub notifications: NotificationSettings,
    pub accessories: AccessorySettings,
    pub downloads: DownloadSettings,
    pub shortcuts: ShortcutSettings,
    pub camera: CameraSettings,
    pub agents: AgentSettings,
    pub watch: WatchSettings,
    pub timers: TimerSettings,
    pub companion: CompanionSettings,
    pub metrics: MetricsSettings,
    pub power: PowerSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            launch_at_login: false,
            modules: BTreeMap::new(),
            island: Default::default(),
            music: Default::default(),
            calendar: Default::default(),
            notifications: Default::default(),
            accessories: Default::default(),
            downloads: Default::default(),
            shortcuts: Default::default(),
            camera: Default::default(),
            agents: Default::default(),
            watch: Default::default(),
            timers: Default::default(),
            companion: Default::default(),
            metrics: Default::default(),
            power: Default::default(),
        }
    }
}

impl Settings {
    /// Modules that need a permission prompt or screen capture start off until
    /// the user enables them (permissions are requested only when needed).
    pub fn default_enabled(id: ModuleId) -> bool {
        !matches!(
            id,
            ModuleId::Camera | ModuleId::Watch | ModuleId::Notifications
        )
    }
    pub fn module_enabled(&self, id: ModuleId) -> bool {
        self.modules
            .get(&id)
            .copied()
            .unwrap_or_else(|| Self::default_enabled(id))
    }
    pub fn set_module_enabled(&mut self, id: ModuleId, on: bool) {
        self.modules.insert(id, on);
    }
    /// Enabled sections in display order whose module is enabled.
    pub fn visible_sections(&self) -> Vec<Section> {
        self.island
            .sections
            .iter()
            .filter(|c| c.enabled && self.module_enabled(c.section.module()))
            .map(|c| c.section)
            .collect()
    }
    /// Adds sections introduced by newer versions to an older saved order.
    fn normalize(&mut self) {
        for s in Section::DEFAULT_ORDER {
            if !self.island.sections.iter().any(|c| c.section == s) {
                self.island.sections.push(SectionConfig {
                    section: s,
                    enabled: true,
                });
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.island.sections.retain(|c| seen.insert(c.section));
    }
}

/// Loads/saves settings atomically.
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn default_location() -> Self {
        Self::new(crate::paths::app_support_dir().join("settings.json"))
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    /// Unreadable/corrupt files are moved aside and defaults are used.
    pub fn load(&self) -> Settings {
        let mut s = match std::fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("settings corrupt ({e}); keeping a backup and using defaults");
                    let _ = std::fs::rename(&self.path, self.path.with_extension("corrupt.json"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        };
        s.normalize();
        s
    }
    pub fn save(&self, s: &Settings) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(&self.path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial_files() {
        let dir = std::env::temp_dir().join(format!("nox-settings-{}", std::process::id()));
        let store = SettingsStore::new(dir.join("settings.json"));
        let mut s = Settings::default();
        s.set_module_enabled(ModuleId::Agents, false);
        s.metrics.interval_ms = 5000;
        store.save(&s).unwrap();
        let back = store.load();
        assert_eq!(back, s);

        std::fs::write(store.path(), br#"{"metrics":{"interval_ms":900}}"#).unwrap();
        let partial = store.load();
        assert_eq!(partial.metrics.interval_ms, 900);
        assert_eq!(partial.island.sections.len(), Section::DEFAULT_ORDER.len());

        std::fs::write(store.path(), b"not json").unwrap();
        assert_eq!(store.load(), {
            let mut d = Settings::default();
            d.normalize();
            d
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disabled_module_hides_section() {
        let mut s = Settings::default();
        assert!(s.visible_sections().contains(&Section::Music));
        s.set_module_enabled(ModuleId::Music, false);
        assert!(!s.visible_sections().contains(&Section::Music));
        assert!(
            !s.visible_sections().contains(&Section::Camera),
            "camera off by default"
        );
    }
}
