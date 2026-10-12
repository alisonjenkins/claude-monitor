// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use claude_monitor::feed::{EventSource, FakeHandle, FakeSource, MergedSource};

const REAL_OS_WAKE_TIMEOUT: Duration = Duration::from_secs(2);
const QUIET: Duration = Duration::from_millis(150);

fn merged_fakes(n: usize) -> (MergedSource, Vec<FakeHandle>) {
    let mut sources: Vec<Box<dyn EventSource + Send>> = Vec::new();
    let mut handles = Vec::new();
    for _ in 0..n {
        let (source, handle) = FakeSource::new();
        sources.push(Box::new(source));
        handles.push(handle);
    }
    (MergedSource::new(sources), handles)
}

/// Reports `wakes` immediate wakes, then idles; `done` flips after the last one.
struct Burst {
    wakes: u32,
    done: Arc<AtomicBool>,
}

impl EventSource for Burst {
    fn wait(&mut self, timeout: Duration) -> bool {
        if self.wakes > 0 {
            self.wakes -= 1;
            return true;
        }
        // Reached only after the caller took the last wake and came back.
        self.done.store(true, Ordering::SeqCst);
        thread::sleep(timeout.min(Duration::from_millis(10)));
        false
    }
}

#[test]
#[ignore = "until the merged source lands"]
fn any_child_source_waking_wakes_the_merged_source() {
    let (mut merged, handles) = merged_fakes(3);

    for (n, handle) in handles.iter().enumerate() {
        handle.wake();
        assert!(merged.wait(REAL_OS_WAKE_TIMEOUT), "source {n}");
    }
}

#[test]
#[ignore = "until the merged source lands"]
fn it_wakes_again_after_the_wake_was_taken() {
    let (mut merged, handles) = merged_fakes(1);

    for _ in 0..3 {
        handles[0].wake();
        assert!(merged.wait(REAL_OS_WAKE_TIMEOUT));
    }
    assert!(!merged.wait(QUIET));
}

#[test]
#[ignore = "until the merged source lands"]
fn a_burst_of_wakes_coalesces_into_one() {
    let done = Arc::new(AtomicBool::new(false));
    let burst = Burst {
        wakes: 100,
        done: Arc::clone(&done),
    };
    let mut merged = MergedSource::new(vec![Box::new(burst)]);
    let deadline = Instant::now() + REAL_OS_WAKE_TIMEOUT;
    while !done.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "burst never finished");
        thread::yield_now();
    }

    assert!(merged.wait(Duration::ZERO));
    assert!(!merged.wait(QUIET), "100 wakes left one pending wake");
}

#[test]
#[ignore = "until the merged source lands"]
fn it_times_out_without_a_wake() {
    let (mut merged, _handles) = merged_fakes(2);
    let started = Instant::now();

    assert!(!merged.wait(Duration::from_millis(100)));

    assert!(started.elapsed() >= Duration::from_millis(100));
}

#[test]
#[ignore = "until the merged source lands"]
fn dropping_it_joins_the_forwarding_threads_and_drops_the_sources() {
    let (merged, handles) = merged_fakes(3);
    assert!(handles.iter().all(|h| !h.is_closed()));
    let started = Instant::now();

    drop(merged);

    assert!(handles.iter().all(FakeHandle::is_closed));
    assert!(started.elapsed() < REAL_OS_WAKE_TIMEOUT, "prompt shutdown");
}

#[test]
#[ignore = "until the merged source lands"]
fn with_no_sources_it_never_wakes_and_honours_the_timeout() {
    let mut merged = MergedSource::new(Vec::new());
    let started = Instant::now();

    assert!(!merged.wait(Duration::from_millis(100)));

    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(!merged.wait(Duration::ZERO));
}
