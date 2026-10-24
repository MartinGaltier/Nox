//! AppKit glue (main thread only). Everything here is thin: layout and
//! behaviour live in the pure modules of the crate.

pub mod alerts;
pub mod app;
pub mod camera;
pub mod canvas;
pub mod files;
#[cfg(debug_assertions)]
pub mod fixture;
pub mod haptics;
pub mod hotkey;
pub mod island_window;
pub mod login;
pub mod monitor_window;
pub mod overlay;
pub mod painter;
pub mod screens;
pub mod settings_window;
#[cfg(debug_assertions)]
pub mod snapshot;
pub mod status;
pub mod util;

use crate::UiContext;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

pub fn run(ctx: UiContext) -> ! {
    let mtm = MainThreadMarker::new().expect("nox_ui::run must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Accessory: no Dock icon, never steals focus by itself.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let _controller = app::App::install(mtm, ctx);
    app.run();
    std::process::exit(0)
}
