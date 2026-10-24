// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/QuickTools/QuickTogglesService.swift, commit e916961.

//! Private system switches resolved at run time (never linked): screen lock
//! (login.framework) and light/dark appearance (SkyLight). A missing symbol
//! is reported as unsupported, never faked.

use std::ffi::{CStr, c_void};

fn resolve(framework: &CStr, symbol: &CStr) -> Option<*mut c_void> {
    // SAFETY: dlopen/dlsym on system framework paths with NUL-terminated names.
    unsafe {
        let h = libc::dlopen(framework.as_ptr(), libc::RTLD_LAZY);
        if h.is_null() {
            return None;
        }
        let s = libc::dlsym(h, symbol.as_ptr());
        (!s.is_null()).then_some(s)
    }
}

const LOGIN: &CStr = c"/System/Library/PrivateFrameworks/login.framework/login";
const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";

/// Locks the session immediately. `false` if unavailable.
pub fn lock_screen() -> bool {
    let Some(p) = resolve(LOGIN, c"SACLockScreenImmediate") else {
        return false;
    };
    // SAFETY: `int SACLockScreenImmediate(void)`.
    let f: extern "C" fn() -> i32 = unsafe { std::mem::transmute(p) };
    f() == 0
}

/// Current appearance: `Some(true)` = dark.
pub fn dark_mode() -> Option<bool> {
    let p = resolve(SKYLIGHT, c"SLSGetAppearanceThemeLegacy")?;
    // SAFETY: `bool SLSGetAppearanceThemeLegacy(void)`.
    let f: extern "C" fn() -> bool = unsafe { std::mem::transmute(p) };
    Some(f())
}

/// Switches light/dark system-wide (what System Settings does).
pub fn set_dark_mode(dark: bool) -> bool {
    let Some(p) = resolve(SKYLIGHT, c"SLSSetAppearanceThemeLegacy") else {
        return false;
    };
    // SAFETY: `void SLSSetAppearanceThemeLegacy(bool)`.
    let f: extern "C" fn(bool) = unsafe { std::mem::transmute(p) };
    f(dark);
    dark_mode() == Some(dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "real machine: resolves private symbols only (does not lock or switch)"]
    fn live_symbols_resolve() {
        assert!(resolve(LOGIN, c"SACLockScreenImmediate").is_some());
        assert!(resolve(SKYLIGHT, c"SLSSetAppearanceThemeLegacy").is_some());
        eprintln!("dark mode now: {:?}", dark_mode());
    }
}
