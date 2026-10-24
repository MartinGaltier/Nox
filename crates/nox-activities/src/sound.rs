//! System alert sounds (`/System/Library/Sounds`), played on the main thread.

use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::NSString;
use std::cell::RefCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// A countdown rang.
    TimerDone,
    /// Pomodoro phase change.
    Phase,
    /// A watched region triggered.
    Watch,
}

impl Tone {
    fn name(self) -> &'static str {
        match self {
            Tone::TimerDone => "Glass",
            Tone::Phase => "Hero",
            Tone::Watch => "Ping",
        }
    }
}

thread_local! {
    // Keeps the playing sound alive until the next one (main thread only).
    static CURRENT: RefCell<Option<Retained<NSSound>>> = const { RefCell::new(None) };
}

/// Plays `tone` asynchronously on the main thread. Never blocks the caller.
pub fn play(tone: Tone) {
    nox_core::main_thread::run_async(move || {
        let name = NSString::from_str(tone.name());
        match NSSound::soundNamed(&name) {
            Some(s) => {
                s.stop();
                if !s.play() {
                    log::warn!("NSSound {} did not start", tone.name());
                }
                CURRENT.with(|c| *c.borrow_mut() = Some(s));
            }
            None => log::warn!("system sound {} not found", tone.name()),
        }
    });
}
