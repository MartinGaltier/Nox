//! Built-in display brightness through DisplayServices (private framework,
//! resolved at run time). External displays (DDC) are not supported.

use std::ffi::{CStr, c_void};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
}

const DISPLAY_SERVICES: &CStr =
    c"/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";

fn sym(name: &CStr) -> Option<*mut c_void> {
    // SAFETY: dlopen/dlsym with NUL-terminated names.
    unsafe {
        let h = libc::dlopen(DISPLAY_SERVICES.as_ptr(), libc::RTLD_LAZY);
        if h.is_null() {
            return None;
        }
        let s = libc::dlsym(h, name.as_ptr());
        (!s.is_null()).then_some(s)
    }
}

fn builtin_display() -> Option<u32> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: buffer of 16 entries, `n` receives the count.
    if unsafe { CGGetOnlineDisplayList(16, ids.as_mut_ptr(), &mut n) } != 0 {
        return None;
    }
    // SAFETY: plain query on display ids returned above.
    ids[..n as usize]
        .iter()
        .copied()
        .find(|&d| unsafe { CGDisplayIsBuiltin(d) } != 0)
}

pub fn brightness() -> Option<f32> {
    let d = builtin_display()?;
    let p = sym(c"DisplayServicesGetBrightness")?;
    // SAFETY: `int DisplayServicesGetBrightness(CGDirectDisplayID, float *)`.
    let f: extern "C" fn(u32, *mut f32) -> i32 = unsafe { std::mem::transmute(p) };
    let mut v = 0f32;
    (f(d, &mut v) == 0 && v.is_finite()).then_some(v.clamp(0.0, 1.0))
}

type NotificationCallback = extern "C" fn(
    center: *mut c_void,
    observer: *mut c_void,
    name: *const c_void,
    object: *const c_void,
    user_info: *const c_void,
);

extern "C" fn on_brightness(
    _c: *mut c_void,
    observer: *mut c_void,
    _n: *const c_void,
    _o: *const c_void,
    _u: *const c_void,
) {
    // SAFETY: `observer` is the Box<Sender<()>> owned by `BrightnessObserver`,
    // freed only after unregistering.
    let tx = unsafe { &*(observer as *const crossbeam_channel::Sender<()>) };
    let _ = tx.try_send(());
}

/// Brightness change notifications for the built-in display
/// (`DisplayServicesRegisterForBrightnessChangeNotifications`, private).
/// Create and drop on the main thread (delivered on its run loop).
pub struct BrightnessObserver {
    display: u32,
    observer: *mut crossbeam_channel::Sender<()>,
}

// SAFETY: created/dropped on the main thread only (callers use on_main /
// run_async); moved between threads as an opaque value.
unsafe impl Send for BrightnessObserver {}

impl BrightnessObserver {
    pub fn new(tx: crossbeam_channel::Sender<()>) -> Option<Self> {
        let display = builtin_display()?;
        let p = sym(c"DisplayServicesRegisterForBrightnessChangeNotifications")?;
        sym(c"DisplayServicesUnregisterForBrightnessChangeNotifications")?;
        // SAFETY: `int (CGDirectDisplayID, const void *observer, CFNotificationCallback)`.
        let f: extern "C" fn(u32, *const c_void, NotificationCallback) -> i32 =
            unsafe { std::mem::transmute(p) };
        let observer = Box::into_raw(Box::new(tx));
        if f(display, observer.cast(), on_brightness) != 0 {
            // SAFETY: never registered: reclaim the box.
            drop(unsafe { Box::from_raw(observer) });
            return None;
        }
        Some(Self { display, observer })
    }
}

impl Drop for BrightnessObserver {
    fn drop(&mut self) {
        if let Some(p) = sym(c"DisplayServicesUnregisterForBrightnessChangeNotifications") {
            // SAFETY: `int (CGDirectDisplayID, const void *observer)`.
            let f: extern "C" fn(u32, *const c_void) -> i32 = unsafe { std::mem::transmute(p) };
            f(self.display, self.observer.cast());
        }
        // SAFETY: unregistered above; no callback can use it any more.
        drop(unsafe { Box::from_raw(self.observer) });
    }
}

pub fn set_brightness(v: f32) -> bool {
    let (Some(d), Some(p)) = (builtin_display(), sym(c"DisplayServicesSetBrightness")) else {
        return false;
    };
    // SAFETY: `int DisplayServicesSetBrightness(CGDirectDisplayID, float)`.
    let f: extern "C" fn(u32, f32) -> i32 = unsafe { std::mem::transmute(p) };
    f(d, v.clamp(0.0, 1.0)) == 0
}
