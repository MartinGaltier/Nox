//! Native AppKit UI: island window, animations, settings window, menu bar.
//! Owned by Agent Interface. Public contract: [`UiContext`], [`run`].
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod alerts;
pub mod compact;
pub mod format;
pub mod geom;
pub mod geometry;
pub mod gesture;
pub mod island;
pub mod machine;
pub mod scene;
pub mod spring;
pub mod views;

use nox_core::{Hub, Settings, SettingsStore};
use std::sync::Arc;

/// Everything the UI needs from the app.
pub struct UiContext {
    pub hub: Hub,
    pub store: SettingsStore,
    pub settings: Arc<Settings>,
    /// Called (on the main thread) after the UI saved new settings; the app
    /// forwards them to the module manager.
    pub on_settings_changed: Box<dyn Fn(Arc<Settings>)>,
    /// Called once when the user quits, before the app terminates. Must stop
    /// modules and restore power state synchronously.
    pub on_quit: Box<dyn FnOnce()>,
}

/// Runs the AppKit application on the main thread. Never returns.
pub fn run(ctx: UiContext) -> ! {
    let _ = ctx;
    eprintln!("nox-ui: not implemented yet");
    std::process::exit(0)
}
