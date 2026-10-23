//! Cooperative cancellation and owned background workers.
//!
//! Modules must never leave threads running after `stop()`. `Worker` joins its
//! thread on `stop()` and on drop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    lock: Mutex<()>,
    cv: Condvar,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        let _g = self.inner.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.cv.notify_all();
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }
    /// Sleeps up to `dur`; returns `true` if cancelled (caller should exit).
    pub fn sleep(&self, dur: Duration) -> bool {
        let deadline = Instant::now() + dur;
        let mut g = self.inner.lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if self.is_cancelled() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            g = match self.inner.cv.wait_timeout(g, deadline - now) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
        }
    }
    /// Wakes sleepers without cancelling (used to apply a new interval now).
    pub fn nudge(&self) {
        let _g = self.inner.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.cv.notify_all();
    }
}

/// A named background thread with its own cancel token.
pub struct Worker {
    token: CancelToken,
    handle: Option<JoinHandle<()>>,
    name: String,
}

impl Worker {
    pub fn spawn(name: &str, f: impl FnOnce(CancelToken) + Send + 'static) -> Self {
        let token = CancelToken::new();
        let t = token.clone();
        let handle = std::thread::Builder::new()
            .name(format!("nox-{name}"))
            .spawn(move || f(t))
            .expect("spawn worker thread");
        Self {
            token,
            handle: Some(handle),
            name: name.to_owned(),
        }
    }

    /// Runs `tick` every `interval()` until cancelled. `interval` is re-read each
    /// loop so settings changes apply without restarting.
    pub fn periodic(
        name: &str,
        interval: impl Fn() -> Duration + Send + 'static,
        mut tick: impl FnMut(&CancelToken) + Send + 'static,
    ) -> Self {
        Self::spawn(name, move |token| {
            while !token.is_cancelled() {
                tick(&token);
                if token.sleep(interval()) {
                    break;
                }
            }
        })
    }

    pub fn token(&self) -> &CancelToken {
        &self.token
    }

    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }

    pub fn stop(&mut self) {
        self.token.cancel();
        if let Some(h) = self.handle.take()
            && h.join().is_err()
        {
            log::error!("worker {} panicked", self.name);
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn periodic_worker_stops_and_joins() {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let mut w = Worker::periodic(
            "t",
            || Duration::from_millis(5),
            move |_| {
                n2.fetch_add(1, Ordering::SeqCst);
            },
        );
        std::thread::sleep(Duration::from_millis(40));
        w.stop();
        let after = n.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        assert!(after > 0);
        assert_eq!(after, n.load(Ordering::SeqCst), "no ticks after stop");
        assert!(!w.is_running());
    }

    #[test]
    fn sleep_returns_early_on_cancel() {
        let t = CancelToken::new();
        let t2 = t.clone();
        let start = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            t2.cancel();
        });
        assert!(t.sleep(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
