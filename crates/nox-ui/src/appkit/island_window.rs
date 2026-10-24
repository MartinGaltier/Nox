//! The island's non-activating panel and its custom-drawn view. All state
//! lives in the controller (`app.rs`); these classes only forward events.

use super::app;
use crate::geom::Point;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSEvent, NSPanel,
    NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect};
use std::cell::RefCell;

define_class!(
    // SAFETY: NSPanel may be subclassed; no Drop impl, no ivars.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxIslandPanel"]
    pub struct IslandPanel;

    impl IslandPanel {
        // Never key, never main: the app in front keeps the keyboard (A5).
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }
);

impl IslandPanel {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: designated NSWindow initialiser with valid arguments.
        unsafe {
            msg_send![super(this), initWithContentRect: frame,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false]
        }
    }
}

#[derive(Default)]
pub struct ViewIvars {
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    // SAFETY: NSView may be subclassed; no Drop impl; ivars are main-thread only.
    #[unsafe(super(NSView, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxIslandView"]
    #[ivars = ViewIvars]
    pub struct IslandView;

    impl IslandView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if let Some(a) = app::get() {
                a.draw_island();
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            let old = self.ivars().tracking.borrow_mut().take();
            if let Some(old) = old {
                self.removeTrackingArea(&old);
            }
            let opts = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect
                | NSTrackingAreaOptions::EnabledDuringMouseDrag;
            // SAFETY: owner is self (kept alive by the view hierarchy while the
            // area is installed); no userInfo.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    NSRect::ZERO,
                    opts,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().tracking.borrow_mut() = Some(area);
            // SAFETY: calling the superclass implementation.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, e: &NSEvent) {
            self.forward_move(e);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            self.forward_move(e);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _e: &NSEvent) {
            if let Some(a) = app::get() {
                a.pointer_moved(None);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            let p = self.local(e);
            if let Some(a) = app::get() {
                a.mouse_down(p);
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            let p = self.local(e);
            if let Some(a) = app::get() {
                a.mouse_dragged(p);
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, e: &NSEvent) {
            let p = self.local(e);
            if let Some(a) = app::get() {
                a.scroll(e, p);
            }
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // `point` is in the superview's coordinates; the view fills its
            // window, so only the flip matters.
            // SAFETY: reading the superview of a view in a live hierarchy.
            let sup = unsafe { self.superview() };
            let p = self.convertPoint_fromView(point, sup.as_deref());
            let inside = app::get().is_some_and(|a| a.shape_contains(Point::new(p.x, p.y)));
            if inside {
                // The view is retained by its window; returning +0 is the
                // hitTest: convention.
                self as *const Self as *mut NSView
            } else {
                std::ptr::null_mut()
            }
        }
    }

    unsafe impl NSObjectProtocol for IslandView {}

    unsafe impl NSDraggingDestination for IslandView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _s: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if let Some(a) = app::get() {
                a.drag_entered();
            }
            NSDragOperation::Link
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, _s: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Link
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _s: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            if let Some(a) = app::get() {
                a.drag_ended(None);
            }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, s: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let paths = super::files::file_paths(&s.draggingPasteboard());
            let ok = !paths.is_empty();
            if let Some(a) = app::get() {
                a.drag_ended(Some(paths));
            }
            ok
        }
    }
);

impl IslandView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars::default());
        // SAFETY: designated NSView initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn local(&self, e: &NSEvent) -> Point {
        let p = self.convertPoint_fromView(e.locationInWindow(), None);
        Point::new(p.x, p.y)
    }

    fn forward_move(&self, e: &NSEvent) {
        let p = self.local(e);
        if let Some(a) = app::get() {
            a.pointer_moved(Some(p));
        }
    }
}
