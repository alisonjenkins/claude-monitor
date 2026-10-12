//! Safety-net source (stub; the timer implementation follows).

use std::time::{Duration, Instant};

use super::{EventSource, Monotonic};

pub const SAFETY_NET_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct SafetyNetSource<C: Monotonic> {
    _interval: Duration,
    _clock: C,
}

impl<C: Monotonic> SafetyNetSource<C> {
    pub fn new(interval: Duration, clock: C) -> Self {
        Self {
            _interval: interval,
            _clock: clock,
        }
    }

    pub fn due(&mut self, _now: Instant) -> bool {
        false
    }
}

impl<C: Monotonic> EventSource for SafetyNetSource<C> {
    fn wait(&mut self, _timeout: Duration) -> bool {
        false
    }
}
