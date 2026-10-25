// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/ResponsibleProcess.swift (responsible pid + parent chain), commit e916961.
//! Which app a helper process plays for: `responsibility_get_pid_responsible_for_pid`
//! (exported by libsystem, resolved at run time) and the BSD parent chain.

use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
use std::ffi::c_void;
use std::sync::OnceLock;

type ResponsibleFn = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;

fn responsible_fn() -> Option<ResponsibleFn> {
    static F: OnceLock<Option<usize>> = OnceLock::new();
    let addr = *F.get_or_init(|| {
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name; a null
        // result means the symbol is absent.
        let p = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"responsibility_get_pid_responsible_for_pid".as_ptr(),
            )
        };
        (!p.is_null()).then_some(p as usize)
    });
    // SAFETY: the symbol has the C signature pid_t (*)(pid_t).
    addr.map(|a| unsafe { std::mem::transmute::<*mut c_void, ResponsibleFn>(a as *mut c_void) })
}

/// The process the system holds responsible for `pid` (itself if unknown).
pub fn responsible(pid: i32) -> i32 {
    match responsible_fn() {
        Some(f) => {
            // SAFETY: plain query; any pid value is accepted.
            let r = unsafe { f(pid) };
            if r > 0 { r } else { pid }
        }
        None => pid,
    }
}

pub fn parent(pid: i32) -> i32 {
    // SAFETY: an all-zero proc_bsdinfo is valid; size passed.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: valid buffer of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if n == size { info.pbi_ppid as i32 } else { 0 }
}

pub fn is_regular_app(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .is_some_and(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular)
}

/// App name and bundle id of a running app.
pub fn app_identity(pid: i32) -> (Option<String>, Option<String>) {
    match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        Some(a) => (
            a.localizedName().map(|s| s.to_string()),
            a.bundleIdentifier().map(|s| s.to_string()),
        ),
        None => (None, None),
    }
}

/// The regular app `pid` plays for: the responsible process, else the first
/// regular-app ancestor (browser audio helpers answer for themselves).
pub fn owning_app(pid: i32) -> Option<i32> {
    crate::routing::owning_app(pid, responsible, is_regular_app, parent)
}
