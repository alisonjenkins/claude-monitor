//! Elapsed time as a seam: login codes, sessions and the throttle read a
//! [`Ticks`] so tests move time by hand instead of sleeping.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Monotonic time since some fixed start; never goes backwards.
pub trait Ticks: Send + Sync {
    fn now(&self) -> Duration;
}

#[derive(Debug)]
pub struct MonotonicTicks(Instant);

impl MonotonicTicks {
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Default for MonotonicTicks {
    fn default() -> Self {
        Self::new()
    }
}

impl Ticks for MonotonicTicks {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// Time that only moves when [`ManualTicks::advance`] says so.
#[derive(Debug, Default)]
pub struct ManualTicks(Mutex<Duration>);

impl ManualTicks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *now = now.saturating_add(by);
    }
}

impl Ticks for ManualTicks {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}
