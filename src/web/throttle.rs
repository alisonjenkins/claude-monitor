//! A global failure limit: after `max_failures` failed logins or
//! unauthenticated requests inside `window`, further ones are refused until
//! the oldest failure ages out. Global, not per client: on loopback every
//! client looks alike, and a logged-in browser is never throttled.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::clock::Ticks;

pub const DEFAULT_MAX_FAILURES: usize = 5;
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);

pub struct Throttle {
    clock: Arc<dyn Ticks>,
    max_failures: usize,
    window: Duration,
    /// Times of the newest `max_failures` failures, oldest first.
    failures: Mutex<VecDeque<Duration>>,
}

impl Throttle {
    pub fn new(clock: Arc<dyn Ticks>, max_failures: usize, window: Duration) -> Self {
        Self {
            clock,
            max_failures,
            window,
            failures: Mutex::new(VecDeque::new()),
        }
    }

    /// `Err(wait)` while the limit is hit. Checking does not count.
    pub fn check(&self) -> Result<(), Duration> {
        let now = self.clock.now();
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        self.expire(&mut failures, now);
        if failures.len() < self.max_failures {
            return Ok(());
        }
        let oldest = failures.front().copied().unwrap_or(now);
        Err(oldest.saturating_add(self.window).saturating_sub(now))
    }

    pub fn record_failure(&self) {
        let now = self.clock.now();
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        self.expire(&mut failures, now);
        failures.push_back(now);
        while failures.len() > self.max_failures {
            failures.pop_front();
        }
    }

    fn expire(&self, failures: &mut VecDeque<Duration>, now: Duration) {
        while failures
            .front()
            .is_some_and(|at| now.saturating_sub(*at) >= self.window)
        {
            failures.pop_front();
        }
    }
}
