//! A worker sleep that a command can cut short.
//!
//! `nox_core::CancelToken::nudge` wakes the condvar but `sleep` then waits
//! again until its original deadline, so it cannot shorten a long idle sleep.

use nox_core::{CancelToken, Worker};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct Wake {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl Wake {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ends the current (or next) `sleep` early.
    pub fn wake(&self) {
        let (m, cv) = &*self.inner;
        *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }

    /// Sleeps up to `dur`, until woken, or until `token` is cancelled (the
    /// canceller must also call `wake`, see [`stop_worker`]). Returns `true`
    /// when the worker should exit.
    pub fn sleep(&self, token: &CancelToken, dur: Duration) -> bool {
        let (m, cv) = &*self.inner;
        let deadline = Instant::now().checked_add(dur);
        let mut woken = m.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if token.is_cancelled() {
                return true;
            }
            if *woken {
                *woken = false;
                return false;
            }
            let wait = match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return false;
                    }
                    d - now
                }
                None => Duration::from_secs(3600),
            };
            woken = match cv.wait_timeout(woken, wait) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
        }
    }
}

/// Cancels, wakes and joins a worker that sleeps on `wake`.
pub fn stop_worker(worker: &mut Option<Worker>, wake: &Wake) {
    if let Some(mut w) = worker.take() {
        w.token().cancel();
        wake.wake();
        w.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_cuts_a_long_sleep() {
        let wake = Wake::new();
        let w2 = wake.clone();
        let token = CancelToken::new();
        let start = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            w2.wake();
        });
        assert!(!wake.sleep(&token, Duration::from_secs(60)));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn stop_joins_idle_worker() {
        let wake = Wake::new();
        let w2 = wake.clone();
        let mut worker = Some(Worker::spawn("wake-test", move |t| {
            while !w2.sleep(&t, Duration::from_secs(3600)) {}
        }));
        let start = Instant::now();
        stop_worker(&mut worker, &wake);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
