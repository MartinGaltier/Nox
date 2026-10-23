//! The common module interface.
//!
//! Lifecycle: `start` is called when the module becomes enabled, `stop` when it
//! is disabled or the app quits. After `stop` returns the module MUST have no
//! running threads, timers, observers, or open handles (no collection while
//! disabled). `start` may be called again later.

use crate::command::Command;
use crate::hub::Hub;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ModuleId {
    Music,
    Calendar,
    Notifications,
    Accessories,
    Downloads,
    Files,
    Shortcuts,
    Camera,
    Agents,
    Watch,
    Timers,
    Companion,
    Metrics,
    Power,
}

impl ModuleId {
    pub const ALL: [ModuleId; 14] = [
        Self::Music,
        Self::Calendar,
        Self::Notifications,
        Self::Accessories,
        Self::Downloads,
        Self::Files,
        Self::Shortcuts,
        Self::Camera,
        Self::Agents,
        Self::Watch,
        Self::Timers,
        Self::Companion,
        Self::Metrics,
        Self::Power,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Music => "music",
            Self::Calendar => "calendar",
            Self::Notifications => "notifications",
            Self::Accessories => "accessories",
            Self::Downloads => "downloads",
            Self::Files => "files",
            Self::Shortcuts => "shortcuts",
            Self::Camera => "camera",
            Self::Agents => "agents",
            Self::Watch => "watch",
            Self::Timers => "timers",
            Self::Companion => "companion",
            Self::Metrics => "metrics",
            Self::Power => "power",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("permission required: {0}")]
    Permission(String),
    #[error("not supported: {0}")]
    NotSupported(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Handed to modules on start.
#[derive(Clone)]
pub struct ModuleContext {
    pub hub: Hub,
    /// Settings at start time; updates arrive via `Module::settings_changed`.
    pub settings: Arc<Settings>,
}

pub trait Module: Send {
    fn id(&self) -> ModuleId;

    /// Begin collecting. Spawn owned `Worker`s; never block for long here.
    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError>;

    /// Stop all collection and release resources. Must be idempotent. Should
    /// also clear the module's slot in the hub (`None`).
    fn stop(&mut self);

    /// Handle a command routed to this module. Must not block for long.
    fn command(&mut self, _cmd: Command) {}

    /// New settings while running (interval changes, toggles inside module).
    fn settings_changed(&mut self, _settings: &Arc<Settings>) {}
}
