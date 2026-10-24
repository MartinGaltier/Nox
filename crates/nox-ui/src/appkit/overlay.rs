//! Region selection overlay for the watch module (D10): a transparent
//! window over the screen under the pointer; drag a rectangle, Escape cancels.
//! The selected region is sent as `WatchCommand::Start` in global points
//! with a top-left origin.

use super::app::{self, App};
use super::canvas::{CanvasIvars, CanvasView, Mouse};
use super::painter::Painter;
use super::util::{ns_rect, rect};
use crate::geom::{Color, Point, Rect};
use crate::scene::{Align, Scene, TextStyle, Weight};
use nox_core::Command;
use nox_core::model::watch::{Rect as WatchRect, WatchCommand, WatchTrigger};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSCursor, NSEvent, NSScreen, NSWindow,
    NSWindowStyleMask,
};
use std::cell::Cell;
use std::rc::Rc;

objc2::define_class!(
    // SAFETY: NSWindow subclass without ivars or Drop.
    #[unsafe(super(NSWindow, objc2_app_kit::NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxOverlayWindow"]
    pub struct OverlayWindow;

    impl OverlayWindow {
        // Borderless windows refuse key status by default; the overlay needs
        // it for Escape.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }
    }
);

pub struct SelectionOverlay {
    window: Retained<OverlayWindow>,
}

/// Converts a selection in the overlay view (flipped, screen-sized) to
/// global top-left-origin points.
pub fn to_global(sel: Rect, screen: Rect, primary_height: f64) -> WatchRect {
    // Global AppKit y-up → CG/top-left: y' = primary_h - y - h.
    let x = screen.x + sel.x;
    let y_up = screen.y + (screen.h - sel.y - sel.h);
    WatchRect {
        x,
        y: primary_height - y_up - sel.h,
        w: sel.w,
        h: sel.h,
    }
}

pub fn normalized(a: Point, b: Point) -> Rect {
    Rect::new(
        a.x.min(b.x),
        a.y.min(b.y),
        (a.x - b.x).abs(),
        (a.y - b.y).abs(),
    )
}

fn finish(region: Option<WatchRect>, trigger: WatchTrigger) {
    if let Some(a) = app::get() {
        if let Some(o) = a.overlay.borrow_mut().take() {
            o.window.orderOut(None);
        }
        match region {
            Some(r) => {
                a.set_capture_exclusion(true);
                a.hub
                    .send(Command::Watch(WatchCommand::Start { region: r, trigger }));
            }
            None => a.hub.send(Command::Watch(WatchCommand::Stop)),
        }
        a.update();
    }
}

pub fn begin(a: &App, trigger: WatchTrigger) {
    let mtm = a.mtm;
    let screens = NSScreen::screens(mtm);
    let mouse = NSEvent::mouseLocation();
    let screen = screens
        .iter()
        .find(|s| {
            let f = rect(s.frame());
            mouse.x >= f.x && mouse.x <= f.max_x() && mouse.y >= f.y && mouse.y <= f.max_y()
        })
        .or_else(|| screens.firstObject());
    let Some(screen) = screen else { return };
    let primary_h = screens
        .firstObject()
        .map(|s| s.frame().size.height)
        .unwrap_or(0.0);
    let sf = rect(screen.frame());
    // SAFETY: designated initialiser.
    let window: Retained<OverlayWindow> = unsafe {
        objc2::msg_send![super(OverlayWindow::alloc(mtm).set_ivars(())),
            initWithContentRect: ns_rect(sf),
            styleMask: NSWindowStyleMask::Borderless,
            backing: NSBackingStoreType::Buffered,
            defer: false]
    };
    // SAFETY: owned by SelectionOverlay.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setLevel(objc2_app_kit::NSScreenSaverWindowLevel);
    window.setIgnoresMouseEvents(false);

    let start: Rc<Cell<Option<Point>>> = Rc::new(Cell::new(None));
    let current: Rc<Cell<Option<Point>>> = Rc::new(Cell::new(None));
    let painter = Rc::new(std::cell::RefCell::new(Painter::default()));
    let (s1, c1) = (start.clone(), current.clone());
    let (s2, c2) = (start.clone(), current.clone());
    let view_frame = ns_rect(Rect::new(0.0, 0.0, sf.w, sf.h));
    let view_cell: Rc<std::cell::RefCell<Option<Retained<CanvasView>>>> = Rc::default();
    let vc = view_cell.clone();
    let view = CanvasView::new(
        mtm,
        view_frame,
        CanvasIvars {
            draw: Box::new(move |bounds| {
                let mut s = Scene::default();
                s.fill(bounds, 0.0, Color::gray(0.0, 0.35));
                let hint = Rect::new(bounds.mid_x() - 260.0, 80.0, 520.0, 44.0);
                s.fill(hint, 14.0, Color::gray(0.0, 0.75));
                s.text(
                    hint.inset(12.0, 13.0),
                    "Tracez la zone à surveiller · Échap pour annuler",
                    TextStyle::new(14.0)
                        .weight(Weight::Semibold)
                        .align(Align::Center),
                );
                if let (Some(a), Some(b)) = (s1.get(), c1.get()) {
                    let r = normalized(a, b);
                    s.fill(r, 2.0, Color::ACCENT.with_alpha(0.18));
                    s.stroke(r, 2.0, Color::ACCENT, 2.0);
                    s.text(
                        Rect::new(r.x, r.max_y() + 6.0, 200.0, 16.0),
                        format!("{} × {} pt", r.w.round(), r.h.round()),
                        TextStyle::new(11.0).mono(),
                    );
                }
                painter.borrow_mut().paint(&s, &|_| None);
            }),
            mouse: Some(Box::new(move |m| {
                match m {
                    Mouse::Down(p) => {
                        s2.set(Some(p));
                        c2.set(Some(p));
                    }
                    Mouse::Dragged(p) => c2.set(Some(p)),
                    Mouse::Up(p) => {
                        c2.set(Some(p));
                        if let Some(a) = s2.get() {
                            let r = normalized(a, p);
                            let region =
                                (r.w >= 8.0 && r.h >= 8.0).then(|| to_global(r, sf, primary_h));
                            let t = trigger;
                            nox_core::main_thread::run_async(move || finish(region, t));
                        }
                    }
                }
                if let Some(v) = vc.borrow().as_ref() {
                    v.setNeedsDisplay(true);
                }
            })),
            key: Some(Box::new(move |code| {
                if code == 53 {
                    nox_core::main_thread::run_async(move || finish(None, trigger));
                }
            })),
        },
    );
    *view_cell.borrow_mut() = Some(view.clone());
    window.setContentView(Some(&view));
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&view));
    NSCursor::crosshairCursor().set();
    *a.overlay.borrow_mut() = Some(SelectionOverlay { window });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_maps_to_global_top_left_points() {
        // Primary 1470×956 at origin; selection 100×50 at (10, 20) from top.
        let r = to_global(
            Rect::new(10.0, 20.0, 100.0, 50.0),
            Rect::new(0.0, 0.0, 1470.0, 956.0),
            956.0,
        );
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 20.0, 100.0, 50.0));
        // Secondary display above-right of the primary (AppKit y-up origin 956).
        let r = to_global(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(1470.0, 956.0, 1920.0, 1080.0),
            956.0,
        );
        assert_eq!((r.x, r.y), (1470.0, -1080.0));
        assert_eq!(
            normalized(Point::new(5.0, 9.0), Point::new(1.0, 2.0)),
            Rect::new(1.0, 2.0, 4.0, 7.0)
        );
    }
}
