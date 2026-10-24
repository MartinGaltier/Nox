//! Bridges to the AppKit main thread. Modules run on worker threads; anything
//! that touches AppKit / NSAppleScript / AVCapture preview must go through here.

use dispatch2::DispatchQueue;

/// Schedules `f` on the main queue without waiting.
pub fn run_async(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

/// Runs `f` on the main thread and waits for its result.
///
/// DEADLOCK RISK: the app's quit path runs on the main thread and joins every
/// module worker (`ModuleManager::shutdown`). A worker blocked here at that
/// moment never returns. Workers should prefer [`run_with_timeout`].
pub fn run_sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    dispatch2::run_on_main(|_| f())
}

/// Runs `f` on the main thread and waits at most `timeout` for the result.
/// Runs inline when already on the main thread. Returns `None` on timeout (the
/// closure may still run later; it must not rely on the caller being alive).
pub fn run_with_timeout<R: Send + 'static>(
    timeout: std::time::Duration,
    f: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    if is_main_thread() {
        return Some(f());
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    DispatchQueue::main().exec_async(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(timeout).ok()
}

/// True on the process main thread.
pub fn is_main_thread() -> bool {
    // SAFETY: pthread_main_np has no preconditions.
    unsafe { pthread_main_np() != 0 }
}

unsafe extern "C" {
    fn pthread_main_np() -> std::ffi::c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_out_without_a_running_main_queue() {
        // The test harness never drains the main queue: must return None.
        let r = std::thread::spawn(|| run_with_timeout(std::time::Duration::from_millis(50), || 1))
            .join()
            .unwrap();
        assert_eq!(r, None);
    }
}
