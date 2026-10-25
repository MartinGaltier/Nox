//! Nox entry point: loads settings, runs crash recovery, starts the module
//! manager, then hands the main thread to the AppKit UI.

use nox_core::{Hub, Module, SettingsStore, manager::ModuleManager};
use std::sync::{Arc, Mutex};

mod logging;

fn main() {
    logging::init();
    log::info!(
        "Nox {} starting (pid {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );

    // Repair anything a previous crash left behind before starting modules.
    nox_power::startup_recovery();

    let store = SettingsStore::default_location();
    let settings = Arc::new(store.load());
    let hub = Hub::new();

    let mut modules: Vec<Box<dyn Module>> = Vec::new();
    modules.push(nox_metrics::module());
    modules.push(nox_power::module());
    modules.extend(nox_integrations::modules());
    modules.extend(nox_activities::modules());
    modules.push(nox_audio::mixer_module());

    let (manager, _running) = ModuleManager::spawn(hub.clone(), modules, settings.clone());
    let manager = Arc::new(Mutex::new(Some(manager)));

    let m_settings = manager.clone();
    let m_quit = manager;
    nox_ui::run(nox_ui::UiContext {
        hub,
        store,
        settings,
        on_settings_changed: Box::new(move |s| {
            if let Some(m) = m_settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                m.apply_settings(s);
            }
        }),
        on_quit: Box::new(move || {
            log::info!("quit requested: stopping modules");
            if let Some(mut m) = m_quit.lock().unwrap_or_else(|e| e.into_inner()).take() {
                m.shutdown();
            }
        }),
    });
}
