//! Brings the app hosting an agent session (its terminal or editor) forward:
//! walks up the session's process ancestors to the first regular GUI app.

use objc2_app_kit::{
    NSApplicationActivationOptions, NSApplicationActivationPolicy, NSRunningApplication,
};

fn parent(pid: i32) -> Option<i32> {
    // SAFETY: `info` is a zeroed plain C struct of the size we pass;
    // proc_pidinfo writes at most that many bytes and returns the count.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size);
        (n == size && info.pbi_ppid > 1).then_some(info.pbi_ppid as i32)
    }
}

/// Ancestors of `pid`, nearest first (bounded).
pub fn ancestors(pid: i32) -> Vec<i32> {
    let mut v = vec![pid];
    let mut cur = pid;
    while v.len() < 32
        && let Some(p) = parent(cur)
    {
        v.push(p);
        cur = p;
    }
    v
}

/// Activates the nearest GUI ancestor of `pid` on the main thread.
pub fn focus(pid: i32) {
    let chain = ancestors(pid);
    nox_core::main_thread::run_async(move || {
        for p in chain {
            let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(p) else {
                continue;
            };
            if app.activationPolicy() == NSApplicationActivationPolicy::Regular {
                let _ = app.activateWithOptions(NSApplicationActivationOptions::empty());
                return;
            }
        }
        log::info!("agents: no GUI app found for session process");
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn own_ancestry_is_walkable() {
        let me = std::process::id() as i32;
        let a = super::ancestors(me);
        assert_eq!(a[0], me);
        assert!(a.len() >= 2, "the test runner has a parent");
    }
}
