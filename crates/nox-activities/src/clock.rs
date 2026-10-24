//! Injectable time source: a monotonic clock for exact durations and a wall
//! clock for display and persistence across restarts.

use nox_core::model::Timestamp;
use std::sync::Arc;
use std::time::Instant;

pub trait Clock: Send + Sync {
    /// Monotonic seconds since an arbitrary origin (never goes back, ignores
    /// wall-clock changes).
    fn mono(&self) -> f64;
    /// Seconds since the Unix epoch.
    fn wall(&self) -> Timestamp;
}

pub type SharedClock = Arc<dyn Clock>;

pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn shared() -> SharedClock {
        Arc::new(Self {
            origin: Instant::now(),
        })
    }
}

impl Clock for SystemClock {
    fn mono(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
    fn wall(&self) -> Timestamp {
        nox_core::model::now()
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use std::sync::Mutex;

    /// Manually advanced clock for tests.
    pub struct ManualClock {
        t: Mutex<(f64, f64)>,
    }

    impl ManualClock {
        pub fn new(wall: f64) -> Arc<Self> {
            Arc::new(Self {
                t: Mutex::new((0.0, wall)),
            })
        }
        pub fn advance(&self, secs: f64) {
            let mut t = self.t.lock().unwrap();
            t.0 += secs;
            t.1 += secs;
        }
        /// Moves only the wall clock (user changed the time).
        pub fn jump_wall(&self, secs: f64) {
            self.t.lock().unwrap().1 += secs;
        }
    }

    impl Clock for ManualClock {
        fn mono(&self) -> f64 {
            self.t.lock().unwrap().0
        }
        fn wall(&self) -> Timestamp {
            self.t.lock().unwrap().1
        }
    }
}
