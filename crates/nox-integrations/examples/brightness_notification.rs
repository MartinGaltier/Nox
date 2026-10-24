//! Real-machine check that DisplayServices brightness notifications reach
//! the main run loop (E8). Nudges the built-in display by +0.03 for one
//! second, then restores the exact previous value.
//!   cargo run -p nox-integrations --example brightness_notification

use nox_integrations::sys::display;
use std::ffi::c_void;
use std::time::Duration;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after: u8) -> i32;
    static kCFRunLoopDefaultMode: *const c_void;
}

fn main() {
    let Some(before) = display::brightness() else {
        println!("brightness unavailable");
        return;
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    let Some(obs) = display::BrightnessObserver::new(tx) else {
        println!("brightness notifications unavailable (symbol missing or refused)");
        return;
    };
    let target = if before > 0.5 {
        before - 0.03
    } else {
        before + 0.03
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        display::set_brightness(target);
        std::thread::sleep(Duration::from_secs(1));
        display::set_brightness(before);
    });
    // SAFETY: runs this thread's (main) run loop for 2.5 s.
    unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 2.5, 0) };
    let n = rx.try_iter().count();
    drop(obs);
    let after = display::brightness().unwrap_or(-1.0);
    println!(
        "notifications received: {n}; restored: {}",
        (after - before).abs() < 0.01
    );
}
