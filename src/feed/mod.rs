//! Change feed over the `events` table.
//!
//! A source only says "something may have changed"; it carries no payload and
//! its wakes coalesce. On every fired wake the feed re-reads the rows after its
//! cursor, so the rows (not the wake) say exactly what changed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::store::events::{EventRow, EventsError};
use crate::store::Store;

pub mod claude;
pub mod copilot;
pub mod files;
pub mod fs;
pub mod merge;
pub mod proc_exit;
pub mod registry;
pub mod safety_net;

pub use claude::{ClaudeProjectsSource, ClaudeSourceError};
pub use copilot::{CopilotSessionsSource, CopilotSourceError};
pub use files::{BoundFilesSource, FilesSourceError};
pub use fs::{FsSourceError, FsWatcherSource};
pub use merge::MergedSource;
pub use proc_exit::{ProcExitError, ProcExitHandle, ProcExitSource};
pub use registry::{ClaudeRegistrySource, RegistrySourceError};
pub use safety_net::{SafetyNetSource, SAFETY_NET_INTERVAL};

/// Start-anchored: a fire comes this long after the first wake of a group. A
/// trailing debounce starves under continuous writes (first fire after 1.1 to
/// 3.3 s in the R8 spike); this gave p99 about 50 ms.
pub const THROTTLE_WINDOW: Duration = Duration::from_millis(50);

/// Rows read per query; a full batch keeps the feed pending so a backlog drains
/// without waiting for another wake.
pub const BATCH_LIMIT: u32 = 500;

/// A blocking-capable, payload-free change signal.
pub trait EventSource {
    /// Waits up to `timeout` for a wake and swallows any that queued behind it.
    /// `Duration::ZERO` polls without blocking.
    fn wait(&mut self, timeout: Duration) -> bool;
}

pub trait Monotonic {
    fn now(&self) -> Instant;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemMonotonic;

impl Monotonic for SystemMonotonic {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Groups wakes into one fire per window, anchored at the first wake.
#[derive(Debug, Clone)]
pub struct Throttle {
    window: Duration,
    deadline: Option<Instant>,
}

impl Throttle {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            deadline: None,
        }
    }

    /// Arms the window if idle; a wake inside an armed window joins it.
    pub fn wake(&mut self, now: Instant) {
        if self.deadline.is_none() {
            self.deadline = Some(now.checked_add(self.window).unwrap_or(now));
        }
    }

    /// True once when the armed window has elapsed, which re-arms for the next group.
    pub fn poll(&mut self, now: Instant) -> bool {
        match self.deadline {
            Some(deadline) if now >= deadline => {
                self.deadline = None;
                true
            }
            _ => false,
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

#[derive(Debug)]
pub struct ChangeFeed<S: EventSource, C: Monotonic = SystemMonotonic> {
    store: Store,
    source: S,
    clock: C,
    throttle: Throttle,
    cursor: i64,
    pending: bool,
}

impl<S: EventSource, C: Monotonic> ChangeFeed<S, C> {
    /// Starts at the current end of the table: only later events are delivered.
    pub fn new(store: Store, source: S, clock: C) -> Result<Self, EventsError> {
        let cursor = store.last_seq()?;
        Ok(Self {
            store,
            source,
            clock,
            throttle: Throttle::new(THROTTLE_WINDOW),
            cursor,
            pending: false,
        })
    }

    /// Replays from after `seq` (a `Last-Event-ID`); the next `poll` reads at once.
    pub fn resume_from(&mut self, seq: i64) {
        self.cursor = seq;
        self.pending = true;
    }

    pub fn cursor(&self) -> i64 {
        self.cursor
    }

    /// When the next window closes, for a caller that sleeps between polls.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.throttle.deadline()
    }

    /// Blocks on the source for up to `timeout`, then polls. A stub until the
    /// implementing commit.
    pub fn wait_poll(&mut self, timeout: Duration) -> Result<Vec<EventRow>, EventsError> {
        let _ = timeout;
        Ok(Vec::new())
    }

    /// Non-blocking. Rows after the cursor once a window has fired (or a replay
    /// or backlog is pending), oldest first; empty otherwise.
    pub fn poll(&mut self) -> Result<Vec<EventRow>, EventsError> {
        let now = self.clock.now();
        if self.source.wait(Duration::ZERO) {
            self.throttle.wake(now);
        }
        if !(self.throttle.poll(now) || self.pending) {
            return Ok(Vec::new());
        }
        let rows = self.store.events_since(self.cursor, BATCH_LIMIT)?;
        if let Some(last) = rows.last() {
            self.cursor = last.seq;
        }
        self.pending = u32::try_from(rows.len()).is_ok_and(|n| n >= BATCH_LIMIT);
        Ok(rows)
    }
}

#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<Mutex<Instant>>);

#[doc(hidden)]
impl ManualClock {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *now = now.checked_add(by).unwrap_or(*now);
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Monotonic for ManualClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug, Default)]
struct FakeState {
    woken: Mutex<bool>,
    woke: Condvar,
    closed: AtomicBool,
}

/// Test source: the paired [`FakeHandle`] injects wakes and sees the drop.
#[doc(hidden)]
#[derive(Debug)]
pub struct FakeSource(Arc<FakeState>);

#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct FakeHandle(Arc<FakeState>);

impl FakeSource {
    pub fn new() -> (Self, FakeHandle) {
        let state = Arc::new(FakeState::default());
        (Self(Arc::clone(&state)), FakeHandle(state))
    }
}

impl FakeHandle {
    pub fn wake(&self) {
        *self.0.woken.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.woke.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.0.closed.load(Ordering::SeqCst)
    }
}

impl EventSource for FakeSource {
    /// Blocks like a real source, so a forwarding thread does not spin on it.
    fn wait(&mut self, timeout: Duration) -> bool {
        let woken = self.0.woken.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut woken, _) = self
            .0
            .woke
            .wait_timeout_while(woken, timeout, |woken| !*woken)
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *woken)
    }
}

impl Drop for FakeSource {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    // Offsets from a fresh Instant cannot overflow.
    #![allow(clippy::arithmetic_side_effects)]

    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn idle_throttle_never_fires() {
        let mut throttle = Throttle::new(THROTTLE_WINDOW);
        let t0 = Instant::now();

        assert!(!throttle.poll(t0 + ms(500)));
        assert_eq!(throttle.deadline(), None);
    }

    #[test]
    fn fires_one_window_after_the_first_wake() {
        let mut throttle = Throttle::new(THROTTLE_WINDOW);
        let t0 = Instant::now();

        throttle.wake(t0);

        assert!(!throttle.poll(t0 + ms(49)));
        assert!(throttle.poll(t0 + ms(50)));
        assert!(!throttle.poll(t0 + ms(51)), "one fire per group");
    }

    #[test]
    fn wakes_inside_the_window_do_not_move_the_deadline() {
        let mut throttle = Throttle::new(THROTTLE_WINDOW);
        let t0 = Instant::now();

        throttle.wake(t0);
        throttle.wake(t0 + ms(30));
        throttle.wake(t0 + ms(49));

        assert_eq!(throttle.deadline(), Some(t0 + ms(50)));
        assert!(throttle.poll(t0 + ms(50)));
    }

    #[test]
    fn a_wake_after_a_fire_arms_a_new_window() {
        let mut throttle = Throttle::new(THROTTLE_WINDOW);
        let t0 = Instant::now();
        throttle.wake(t0);
        assert!(throttle.poll(t0 + ms(50)));

        throttle.wake(t0 + ms(52));

        assert_eq!(throttle.deadline(), Some(t0 + ms(102)));
        assert!(throttle.poll(t0 + ms(102)));
    }

    #[test]
    fn continuous_wakes_fire_every_window() {
        let mut throttle = Throttle::new(THROTTLE_WINDOW);
        let t0 = Instant::now();
        let mut fires = 0;

        for step in 0..250u32 {
            let now = t0 + ms(2) * step;
            throttle.wake(now);
            if throttle.poll(now) {
                fires += 1;
            }
        }

        assert!(fires >= 8, "{fires}");
    }
}
