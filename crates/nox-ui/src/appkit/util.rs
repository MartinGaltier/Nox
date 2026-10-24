//! Small AppKit helpers: closure-backed action targets, timers, colours,
//! notification observers.

use crate::geom::{Color, Rect};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSColor;
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSOperationQueue,
    NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSTimer,
};
use std::ptr::NonNull;

pub struct TargetIvars {
    action: Box<dyn Fn(&AnyObject)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class has no Drop
    // impl and its only ivar is a Rust closure called on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxActionTarget"]
    #[ivars = TargetIvars]
    pub struct ActionTarget;

    impl ActionTarget {
        #[unsafe(method(fire:))]
        fn fire(&self, sender: &AnyObject) {
            (self.ivars().action)(sender);
        }
    }

    unsafe impl NSObjectProtocol for ActionTarget {}
);

impl ActionTarget {
    pub fn new(mtm: MainThreadMarker, f: impl Fn(&AnyObject) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            action: Box::new(f),
        });
        // SAFETY: plain NSObject initialiser on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }

    pub fn selector() -> objc2::runtime::Sel {
        sel!(fire:)
    }
}

/// A running NSTimer; invalidated on drop.
pub struct Timer(Retained<NSTimer>);

impl Timer {
    /// Fires once after `secs` (or repeatedly) on the main run loop, also
    /// while menus track or windows resize (common modes).
    pub fn new(secs: f64, repeats: bool, f: impl Fn() + 'static) -> Self {
        // The timer is attached to the current run loop: main thread only.
        assert!(
            MainThreadMarker::new().is_some(),
            "Timer::new must be called on the main thread"
        );
        let block = RcBlock::new(move |_t: NonNull<NSTimer>| f());
        // SAFETY: the block is 'static and called on the main thread by the
        // run loop; the timer is invalidated before the closure can dangle.
        let t = unsafe {
            NSTimer::timerWithTimeInterval_repeats_block(secs.max(0.001), repeats, &block)
        };
        if repeats {
            t.setTolerance(secs * 0.1);
        }
        // SAFETY: adding a valid timer to the current (main) run loop;
        // NSRunLoopCommonModes is a static Foundation string.
        unsafe {
            NSRunLoop::currentRunLoop().addTimer_forMode(&t, NSRunLoopCommonModes);
        }
        Self(t)
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.0.invalidate();
    }
}

/// Observer token; removed from its centre on drop.
pub struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Observer {
    pub fn new(
        center: Retained<NSNotificationCenter>,
        name: &NSNotificationName,
        f: impl Fn() + 'static,
    ) -> Self {
        Self::for_object(center, name, None, f)
    }

    pub fn for_object(
        center: Retained<NSNotificationCenter>,
        name: &NSNotificationName,
        object: Option<&AnyObject>,
        f: impl Fn() + 'static,
    ) -> Self {
        let block = RcBlock::new(move |_n: NonNull<NSNotification>| f());
        let queue = NSOperationQueue::mainQueue();
        // SAFETY: the main operation queue runs the (non-Send) closure on the
        // main thread whatever thread posts the notification.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                object,
                Some(&queue),
                &block,
            )
        };
        Self { center, token }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: the token was returned by this centre.
        unsafe { self.center.removeObserver(self.token.as_ref()) };
    }
}

pub fn ns_color(c: Color) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c.r, c.g, c.b, c.a)
}

pub fn ns_rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

pub fn rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// Monotonic seconds (for the state machine / animations).
pub fn mono() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}
