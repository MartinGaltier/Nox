//! Bridges to the AppKit main thread. Modules run on worker threads; anything
//! that touches AppKit / NSAppleScript / AVCapture preview must go through here.

use dispatch2::DispatchQueue;

/// Schedules `f` on the main queue without waiting.
pub fn run_async(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

/// Runs `f` on the main thread and waits for its result. Must not be called
/// from the main thread while it is blocked on the caller (deadlock).
pub fn run_sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    dispatch2::run_on_main(|_| f())
}
