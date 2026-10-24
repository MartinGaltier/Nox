//! All `unsafe` / Objective-C interop of the crate lives under `sys`.
//! Public functions here are safe; each unsafe block carries a SAFETY note.

pub mod applescript;
pub mod audio;
pub mod avcapture;
pub mod ax;
pub mod display;
pub mod eventkit;
pub mod fsevents;
pub mod http;
pub mod peripherals;
pub mod progress;
pub mod system;
pub mod workspace;

use std::time::Duration;

/// Default bound for main-thread calls (AppleScript events time out at 3 s).
pub const MAIN_TIMEOUT: Duration = Duration::from_secs(4);

/// Runs `f` on the main thread and waits at most `timeout` (never deadlocks
/// when the main thread is blocked joining modules at quit). Thin alias of
/// `nox_core::main_thread::run_with_timeout`.
pub fn on_main<R: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    nox_core::main_thread::run_with_timeout(timeout, f)
}
