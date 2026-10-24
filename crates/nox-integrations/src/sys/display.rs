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

pub fn set_brightness(v: f32) -> bool {
    let (Some(d), Some(p)) = (builtin_display(), sym(c"DisplayServicesSetBrightness")) else {
        return false;
    };
    // SAFETY: `int DisplayServicesSetBrightness(CGDirectDisplayID, float)`.
    let f: extern "C" fn(u32, f32) -> i32 = unsafe { std::mem::transmute(p) };
    f(d, v.clamp(0.0, 1.0)) == 0
}
