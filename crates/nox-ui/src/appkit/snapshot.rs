//! DEBUG-ONLY visual verification without Screen Recording permission:
//! `NOX_UI_SNAPSHOT=<dir>` renders the island view (and optionally the
//! monitor/settings windows, `NOX_UI_OPEN=monitor,settings`) into PNG files
//! with `cacheDisplayInRect:`, plus a text file with the real geometry.

#![cfg(debug_assertions)]

use super::app::{self, App};
use super::util::Timer;
use objc2_app_kit::{NSBitmapImageFileType, NSView};
use objc2_foundation::NSDictionary;
use std::cell::RefCell;

thread_local! {
    static KEEP: RefCell<Vec<Timer>> = const { RefCell::new(vec![]) };
}

pub fn write_view(view: &NSView, path: &str) {
    let bounds = view.bounds();
    let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
        log::warn!("snapshot: no bitmap rep");
        return;
    };
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    // SAFETY: empty properties dictionary of the expected type.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    };
    match data {
        Some(d) => match std::fs::write(path, d.to_vec()) {
            Ok(()) => log::info!("snapshot written: {path}"),
            Err(e) => log::warn!("snapshot {path}: {e}"),
        },
        None => log::warn!("snapshot: PNG encoding failed"),
    }
}

pub fn schedule(_app: &App) {
    let Ok(dir) = std::env::var("NOX_UI_SNAPSHOT") else {
        return;
    };
    let tag = std::env::var("NOX_UI_FORCE")
        .unwrap_or_else(|_| "rest".into())
        .replace(':', "-");
    let open = std::env::var("NOX_UI_OPEN").unwrap_or_default();
    let d2 = dir.clone();
    let t1 = Timer::new(2.0, false, move || {
        let Some(a) = app::get() else { return };
        a.debug_dump(&format!("{d2}/geometry-{tag}.txt"));
        a.with_view(|v| write_view(v, &format!("{d2}/island-{tag}.png")));
        if open.contains("monitor") {
            super::monitor_window::show(&a);
        }
        if open.contains("settings") {
            super::settings_window::show(&a);
        }
    });
    let t2 = Timer::new(3.5, false, move || {
        let Some(a) = app::get() else { return };
        if let Some(m) = a.monitor_window.borrow().as_ref() {
            m.with_view(|v| write_view(v, &format!("{dir}/monitor.png")));
        }
        if let Some(s) = a.settings_window.borrow().as_ref() {
            let _ = std::fs::write(format!("{dir}/settings-layout.txt"), s.layout_report());
        }
    });
    KEEP.with(|k| k.borrow_mut().extend([t1, t2]));
}
