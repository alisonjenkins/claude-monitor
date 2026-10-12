//! Safety-net source: one wake per interval even when nothing else happens, so
//! a dropped event is repaired by the next re-read.

use std::time::{Duration, Instant};

use super::{EventSource, Monotonic};

/// How often a viewer re-reads without being told to (research D12).
pub const SAFETY_NET_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct SafetyNetSource<C: Monotonic> {
    interval: Duration,
    clock: C,
    next_due: Instant,
}

impl<C: Monotonic> SafetyNetSource<C> {
    /// The first fire is one `interval` after construction.
    pub fn new(interval: Duration, clock: C) -> Self {
        let now = clock.now();
        Self {
            interval,
            clock,
            next_due: now.checked_add(interval).unwrap_or(now),
        }
    }

    /// True once when `now` has reached the next fire time, which moves the
    /// next fire to one interval after `now`: a long gap fires once, it does
    /// not catch up.
    pub fn due(&mut self, now: Instant) -> bool {
        if now < self.next_due {
            return false;
        }
        self.next_due = now.checked_add(self.interval).unwrap_or(now);
        true
    }
}

impl<C: Monotonic> EventSource for SafetyNetSource<C> {
    /// Sleeps once, for the shorter of `timeout` and the time left to the next
    /// fire; a timer, not a polling loop.
    fn wait(&mut self, timeout: Duration) -> bool {
        let now = self.clock.now();
        if self.due(now) {
            return true;
        }
        let until_due = self.next_due.saturating_duration_since(now);
        if timeout < until_due {
            std::thread::sleep(timeout);
            return false;
        }
        std::thread::sleep(until_due);
        let at = self.clock.now().max(self.next_due);
        self.due(at)
    }
}
