//! DEBUG-ONLY hover driver for measurements: `NOX_UI_HOVER=<cycles>,<holdMs>,<awayMs>`
//! enters and leaves the island that many times, through the same
//! `pointer_moved` entry point a real hover uses, so the animation, window
//! and drawing paths all run as usual. (Synthetic mouse events need the
//! Accessibility permission, which the measuring terminal may not have.)

#![cfg(any(debug_assertions, feature = "hover-probe"))]

use super::app::{self, App};
use super::util::{Timer, mono};
use std::cell::RefCell;

thread_local! {
    static RUN: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

pub fn schedule(_app: &App) {
    let Ok(spec) = std::env::var("NOX_UI_HOVER") else {
        return;
    };
    let v: Vec<f64> = spec
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let [cycles, hold, away] = v[..] else {
        log::warn!("NOX_UI_HOVER expects <cycles>,<holdMs>,<awayMs>");
        return;
    };
    // (inside?, duration) steps: enter, hold, leave, rest.
    let steps: Vec<(bool, f64)> = (0..cycles as usize)
        .flat_map(|_| [(true, hold / 1000.0), (false, away / 1000.0)])
        .collect();
    // Waits 2 s for the app to settle at rest before the first move.
    let state = RefCell::new((0usize, mono() + 2.0));
    let timer = Timer::new(0.005, true, move || {
        let now = mono();
        let (step, since) = *state.borrow();
        if now < since {
            return;
        }
        let Some(&(inside, dur)) = steps.get(step) else {
            RUN.with(|r| r.borrow_mut().take());
            if let Some(a) = app::get() {
                a.debug_hover(false);
            }
            log::info!("hover probe finished");
            return;
        };
        if now - since < dur {
            return;
        }
        *state.borrow_mut() = (step + 1, now);
        if let Some(a) = app::get() {
            a.debug_hover(inside);
        }
    });
    RUN.with(|r| *r.borrow_mut() = Some(timer));
}
