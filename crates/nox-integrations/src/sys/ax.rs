//! Accessibility (HIServices) for reading Notification Center banners:
//! elements, attributes, actions, and an AXObserver whose run-loop source
//! lives on the main run loop. Every call uses a 0.1 s messaging timeout.

use crossbeam_channel::Sender;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSAttributedString, NSDictionary, NSNumber, NSString};
use std::ffi::c_void;

type AXUIElementRef = *mut c_void;
type AXObserverRef = *mut c_void;
type AXObserverCallback = extern "C" fn(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: *const c_void,
    refcon: *mut c_void,
);

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: *const c_void,
        value: *mut *mut c_void,
    ) -> i32;
    fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut *mut c_void) -> i32;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: *const c_void) -> i32;
    fn AXObserverCreate(
        pid: i32,
        callback: AXObserverCallback,
        observer: *mut AXObserverRef,
    ) -> i32;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        name: *const c_void,
        refcon: *mut c_void,
    ) -> i32;
    fn AXObserverRemoveNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        name: *const c_void,
    ) -> i32;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> *mut c_void;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRelease(cf: *const c_void);
    fn CFEqual(a: *const c_void, b: *const c_void) -> u8;
    static kCFRunLoopCommonModes: *const c_void;
}

const TIMEOUT: f32 = 0.1;

/// Whether Nox is trusted for Accessibility (never prompts).
pub fn is_trusted() -> bool {
    // SAFETY: no arguments, no side effects.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Shows the system "allow Accessibility" prompt (only the first time macOS
/// decides to) and returns the current trust.
pub fn prompt_trust() -> bool {
    // SAFETY: the option key is an immutable HIServices constant; the
    // dictionary is a valid CFDictionary (toll-free bridged).
    unsafe {
        let key = &*(kAXTrustedCheckOptionPrompt as *const NSString);
        let yes = NSNumber::new_bool(true);
        let d = NSDictionary::<NSString, NSNumber>::from_slices(&[key], &[&*yes]);
        AXIsProcessTrustedWithOptions(Retained::as_ptr(&d).cast()) != 0
    }
}

/// An accessibility element (a retained CF object).
#[derive(Clone)]
pub struct Element(Retained<AnyObject>);

// SAFETY: AXUIElementRef is an immutable CF handle; the AX API may be called
// from any thread (calls are IPC to the target process).
unsafe impl Send for Element {}

impl PartialEq for Element {
    fn eq(&self, other: &Self) -> bool {
        // SAFETY: both are valid CF objects.
        unsafe { CFEqual(self.raw(), other.raw()) != 0 }
    }
}

impl Element {
    fn raw(&self) -> AXUIElementRef {
        Retained::as_ptr(&self.0) as AXUIElementRef
    }

    pub fn application(pid: i32) -> Option<Self> {
        // SAFETY: Create rule: we own the returned reference.
        let e = unsafe { AXUIElementCreateApplication(pid) };
        // SAFETY: a non-null CF object, ownership transferred to Retained.
        let r = unsafe { Retained::from_raw(e as *mut AnyObject) }?;
        Some(Self(r))
    }

    fn copy(&self, attribute: &str) -> Option<Retained<AnyObject>> {
        let name = NSString::from_str(attribute);
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid element and CFString; Copy rule for `out`.
        let err = unsafe {
            AXUIElementSetMessagingTimeout(self.raw(), TIMEOUT);
            AXUIElementCopyAttributeValue(self.raw(), Retained::as_ptr(&name).cast(), &mut out)
        };
        if err != 0 || out.is_null() {
            return None;
        }
        // SAFETY: we own `out` (Copy rule).
        unsafe { Retained::from_raw(out as *mut AnyObject) }
    }

    pub fn string(&self, attribute: &str) -> Option<String> {
        let v = self.copy(attribute)?;
        if let Some(s) = v.downcast_ref::<NSString>() {
            return Some(s.to_string());
        }
        v.downcast_ref::<NSAttributedString>()
            .map(|a| a.string().to_string())
    }

    pub fn children(&self) -> Vec<Element> {
        self.elements("AXChildren")
    }

    pub fn windows(&self) -> Vec<Element> {
        self.elements("AXWindows")
    }

    fn elements(&self, attribute: &str) -> Vec<Element> {
        let Some(v) = self.copy(attribute) else {
            return vec![];
        };
        let Some(arr) = v.downcast_ref::<NSArray>() else {
            return vec![];
        };
        arr.iter().take(65).map(Element).collect()
    }

    pub fn has_focused_window(&self) -> bool {
        self.copy("AXFocusedWindow").is_some()
    }

    pub fn actions(&self) -> Vec<String> {
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid element; Copy rule for the returned CFArray.
        let err = unsafe {
            AXUIElementSetMessagingTimeout(self.raw(), TIMEOUT);
            AXUIElementCopyActionNames(self.raw(), &mut out)
        };
        if err != 0 || out.is_null() {
            return vec![];
        }
        // SAFETY: we own the CFArray of CFStrings.
        let Some(arr) = (unsafe { Retained::from_raw(out as *mut NSArray<NSString>) }) else {
            return vec![];
        };
        arr.iter().map(|s| s.to_string()).collect()
    }

    pub fn perform(&self, action: &str) -> bool {
        let name = NSString::from_str(action);
        // SAFETY: valid element and CFString.
        unsafe {
            AXUIElementSetMessagingTimeout(self.raw(), TIMEOUT);
            AXUIElementPerformAction(self.raw(), Retained::as_ptr(&name).cast()) == 0
        }
    }
}

impl crate::notifications::tree::Node for Element {
    fn attr(&self, name: &str) -> Option<String> {
        self.string(name)
    }
    fn children(&self) -> Vec<Self> {
        Element::children(self)
    }
}

extern "C" fn on_ax(_o: AXObserverRef, _e: AXUIElementRef, _n: *const c_void, refcon: *mut c_void) {
    // SAFETY: `refcon` is the Box<Sender<()>> owned by `Observer`, freed only
    // after the run-loop source is removed (no more callbacks).
    let tx = unsafe { &*(refcon as *const Sender<()>) };
    let _ = tx.try_send(());
}

pub const NOTIFICATIONS: &[&str] = &[
    "AXWindowCreated",
    "AXLayoutChanged",
    "AXUIElementDestroyed",
    "AXFocusedWindowChanged",
];

/// AXObserver on one application; its source is on the main run loop.
/// Create and drop on the main thread.
pub struct Observer {
    observer: AXObserverRef,
    app: Element,
    refcon: *mut Sender<()>,
}

// SAFETY: only created/dropped on the main thread (callers use on_main /
// run_async); moved between threads as an opaque value.
unsafe impl Send for Observer {}

impl Observer {
    pub fn new(pid: i32, tx: Sender<()>) -> Option<Self> {
        let app = Element::application(pid)?;
        let mut obs: AXObserverRef = std::ptr::null_mut();
        // SAFETY: valid callback; Create rule for `obs`.
        if unsafe { AXObserverCreate(pid, on_ax, &mut obs) } != 0 || obs.is_null() {
            return None;
        }
        let refcon = Box::into_raw(Box::new(tx));
        let mut attached = false;
        for n in NOTIFICATIONS {
            let name = NSString::from_str(n);
            // SAFETY: valid observer, element, CFString and refcon.
            if unsafe {
                AXObserverAddNotification(
                    obs,
                    app.raw(),
                    Retained::as_ptr(&name).cast(),
                    refcon.cast(),
                )
            } == 0
            {
                attached = true;
            }
        }
        // SAFETY: the run-loop source belongs to the observer (Get rule).
        unsafe {
            CFRunLoopAddSource(
                CFRunLoopGetMain(),
                AXObserverGetRunLoopSource(obs),
                kCFRunLoopCommonModes,
            )
        };
        let o = Self {
            observer: obs,
            app,
            refcon,
        };
        attached.then_some(o)
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: tear down in reverse order on the main thread; after the
        // source is removed no callback can use `refcon`.
        unsafe {
            CFRunLoopRemoveSource(
                CFRunLoopGetMain(),
                AXObserverGetRunLoopSource(self.observer),
                kCFRunLoopCommonModes,
            );
            for n in NOTIFICATIONS {
                let name = NSString::from_str(n);
                AXObserverRemoveNotification(
                    self.observer,
                    self.app.raw(),
                    Retained::as_ptr(&name).cast(),
                );
            }
            CFRelease(self.observer);
            drop(Box::from_raw(self.refcon));
        }
    }
}
