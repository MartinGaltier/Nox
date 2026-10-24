//! All `unsafe` / Objective-C interop of the crate lives under `sys`.
//! Public functions here are safe; each unsafe block carries a SAFETY note.

pub mod applescript;
pub mod avcapture;
pub mod eventkit;
pub mod fsevents;
pub mod http;
pub mod progress;
pub mod system;
pub mod workspace;

use std::time::Duration;

/// Default bound for main-thread calls (AppleScript events time out at 3 s).
pub const MAIN_TIMEOUT: Duration = Duration::from_secs(4);

/// Runs `f` on the main thread and waits at most `timeout` for its result.
/// Unlike `main_thread::run_sync` this cannot deadlock when the main thread
/// is itself blocked (e.g. joining modules at quit): the caller gives up and
/// returns `None`. Runs inline when already on the main thread.
pub fn on_main<R: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    if objc2::MainThreadMarker::new().is_some() {
        return Some(f());
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    nox_core::main_thread::run_async(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(timeout).ok()
}
