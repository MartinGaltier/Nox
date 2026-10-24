//! Unsafe FFI for the app side, isolated here: IOPM assertions and the
//! IOPowerSources change notification on a private CFRunLoop thread.

use nox_helper::sys::{CFRelease, CFRetain, CFStringRef, CFTypeRef, cfstr};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

type CFRunLoopRef = *const c_void;
type CFRunLoopSourceRef = *const c_void;
type IoReturn = i32;

/// `kIOPMAssertionLevelOn`.
const ASSERTION_LEVEL_ON: u32 = 255;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFStringRef,
        level: u32,
        name: CFStringRef,
        id: *mut u32,
    ) -> IoReturn;
    fn IOPMAssertionRelease(id: u32) -> IoReturn;
    fn IOPSNotificationCreateRunLoopSource(
        callback: extern "C" fn(*mut c_void),
        context: *mut c_void,
    ) -> CFRunLoopSourceRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopSourceInvalidate(source: CFRunLoopSourceRef);
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after: u8) -> i32;
    fn CFRunLoopStop(rl: CFRunLoopRef);
    fn CFRunLoopWakeUp(rl: CFRunLoopRef);
}

/// Creates an IOPM assertion (`PreventUserIdleSystemSleep`, …). Returns its id.
pub fn create_assertion(assertion_type: &str, name: &str) -> Result<u32, String> {
    let t = cfstr(assertion_type).ok_or("bad assertion type")?;
    let n = cfstr(name).ok_or("bad assertion name")?;
    let mut id: u32 = 0;
    // SAFETY: both CFStrings are live for the call; `id` is a valid out pointer.
    let r =
        unsafe { IOPMAssertionCreateWithName(t.as_ptr(), ASSERTION_LEVEL_ON, n.as_ptr(), &mut id) };
    if r == 0 {
        Ok(id)
    } else {
        Err(format!("IOPMAssertionCreateWithName 0x{:x}", r as u32))
    }
}

pub fn release_assertion(id: u32) -> Result<(), String> {
    // SAFETY: releasing an id we created; an unknown id only returns an error.
    let r = unsafe { IOPMAssertionRelease(id) };
    if r == 0 {
        Ok(())
    } else {
        Err(format!("IOPMAssertionRelease 0x{:x}", r as u32))
    }
}

type Callback = Box<dyn Fn() + Send + 'static>;

extern "C" fn power_changed(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the `Box<Callback>` leaked in `watch_power_sources`;
    // it is freed only after the source has been invalidated on this thread.
    let cb = unsafe { &*(ctx as *const Callback) };
    cb();
}

struct SendPtr(CFRunLoopRef);
// SAFETY: CFRunLoopStop / CFRunLoopWakeUp are documented thread-safe; the
// pointer is retained until `stop` releases it after joining.
unsafe impl Send for SendPtr {}

/// A thread running a CFRunLoop with the IOPowerSources notification source.
/// Idle (blocked in mach_msg) between power events. Stopped and joined on
/// `stop()` / drop.
pub struct PowerSourceWatch {
    runloop: Option<SendPtr>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub fn watch_power_sources(
    on_change: impl Fn() + Send + 'static,
) -> Result<PowerSourceWatch, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Option<SendPtr>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("nox-power-sources".into())
        .spawn(move || {
            let ctx = Box::into_raw(Box::new(Box::new(on_change) as Callback)) as *mut c_void;
            // SAFETY: the callback/context contract is upheld by
            // `power_changed`; the source follows the Create rule and is
            // invalidated + released below, before the context is freed. The
            // run loop is this thread's own; we retain it for the stopper.
            unsafe {
                let source = IOPSNotificationCreateRunLoopSource(power_changed, ctx);
                if source.is_null() {
                    drop(Box::from_raw(ctx as *mut Callback));
                    let _ = tx.send(None);
                    return;
                }
                let rl = CFRunLoopGetCurrent();
                CFRunLoopAddSource(rl, source, kCFRunLoopDefaultMode);
                CFRetain(rl as CFTypeRef);
                let _ = tx.send(Some(SendPtr(rl)));
                // A stop issued before the loop runs leaves the run loop's
                // "stopped" flag set, so this returns at once in that case.
                while !stop2.load(Ordering::SeqCst) {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 3600.0, 0);
                }
                CFRunLoopRemoveSource(rl, source, kCFRunLoopDefaultMode);
                CFRunLoopSourceInvalidate(source);
                CFRelease(source as CFTypeRef);
                drop(Box::from_raw(ctx as *mut Callback));
            }
        })
        .map_err(|e| format!("spawn power-sources thread: {e}"))?;
    match rx.recv() {
        Ok(Some(rl)) => Ok(PowerSourceWatch {
            runloop: Some(rl),
            stop,
            thread: Some(thread),
        }),
        _ => {
            let _ = thread.join();
            Err("IOPSNotificationCreateRunLoopSource failed".into())
        }
    }
}

impl PowerSourceWatch {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(rl) = &self.runloop {
            // SAFETY: `rl` is retained and these calls are thread-safe.
            unsafe {
                CFRunLoopStop(rl.0);
                CFRunLoopWakeUp(rl.0);
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(rl) = self.runloop.take() {
            // SAFETY: balances the CFRetain done on the watcher thread.
            unsafe { CFRelease(rl.0 as CFTypeRef) }
        }
    }
}

impl Drop for PowerSourceWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn watcher_stops_promptly_even_right_after_start() {
        for _ in 0..20 {
            let mut w = watch_power_sources(|| {}).unwrap();
            let t = Instant::now();
            w.stop();
            assert!(
                t.elapsed() < Duration::from_millis(500),
                "{:?}",
                t.elapsed()
            );
        }
    }
}
