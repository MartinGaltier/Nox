//! System monitor window (detailed metrics, history graphs).

use super::app::{self, App};
use super::canvas::{CanvasIvars, CanvasView};
use super::painter::Painter;
use super::util::{Observer, ns_rect};
use crate::geom::{Color, Rect};
use crate::scene::Scene;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearanceCustomization, NSApplication, NSBackingStoreType, NSColor, NSWindow,
    NSWindowStyleMask, NSWindowWillCloseNotification,
};
use objc2_foundation::{NSNotificationCenter, NSSize, NSString};
use std::cell::RefCell;

pub struct MonitorWindow {
    window: Retained<NSWindow>,
    view: Retained<CanvasView>,
    _close: Observer,
}

impl MonitorWindow {
    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }
    #[cfg(debug_assertions)]
    pub fn with_view(&self, f: impl FnOnce(&objc2_app_kit::NSView)) {
        f(&self.view);
    }
    pub fn redraw(&self) {
        if self.is_visible() {
            self.view.setNeedsDisplay(true);
        }
    }
}

fn create(mtm: MainThreadMarker) -> MonitorWindow {
    let frame = ns_rect(Rect::new(0.0, 0.0, 900.0, 600.0));
    // SAFETY: designated initialiser with valid arguments.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is owned by MonitorWindow; never auto-released.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Moniteur système — Nox"));
    window.setContentMinSize(NSSize::new(760.0, 520.0));
    window.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
        0.09, 0.09, 0.10, 1.0,
    )));
    if let Some(dark) = objc2_app_kit::NSAppearance::appearanceNamed(
        // SAFETY: static appearance name provided by AppKit.
        unsafe { objc2_app_kit::NSAppearanceNameDarkAqua },
    ) {
        window.setAppearance(Some(&dark));
    }
    let painter = RefCell::new(Painter::default());
    let view = CanvasView::new(
        mtm,
        frame,
        CanvasIvars {
            draw: Box::new(move |bounds| {
                let Some(a) = app::get() else { return };
                let st = a.state();
                let settings = a.settings();
                let mut s = Scene::default();
                s.fill(bounds, 0.0, Color::rgba(0.09, 0.09, 0.10, 1.0));
                crate::views::monitor::render(
                    &mut s,
                    bounds.inset(16.0, 16.0),
                    st.metrics.as_ref(),
                    &settings,
                );
                painter.borrow_mut().paint(&s, &|_| None);
            }),
            mouse: None,
            key: None,
        },
    );
    window.setContentView(Some(&view));
    window.center();
    let close = Observer::for_object(
        NSNotificationCenter::defaultCenter(),
        // SAFETY: static AppKit notification name.
        unsafe { NSWindowWillCloseNotification },
        Some(&window),
        || {
            // Visibility changes after this notification returns.
            nox_core::main_thread::run_async(|| {
                if let Some(a) = app::get() {
                    a.update();
                }
            });
        },
    );
    MonitorWindow {
        window,
        view,
        _close: close,
    }
}

pub fn show(a: &App) {
    let mut w = a.monitor_window.borrow_mut();
    let m = w.get_or_insert_with(|| create(a.mtm));
    #[allow(deprecated)]
    NSApplication::sharedApplication(a.mtm).activateIgnoringOtherApps(true);
    m.window.makeKeyAndOrderFront(None);
    m.view.setNeedsDisplay(true);
    drop(w);
    a.set_metrics_visible(true);
}
