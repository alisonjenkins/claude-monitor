// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::time::{Duration, Instant};

use claude_monitor::feed::{
    ChangeFeed, EventSource, ManualClock, Monotonic, SafetyNetSource, SystemMonotonic,
    SAFETY_NET_INTERVAL,
};
use claude_monitor::model::Author;
use claude_monitor::store::events::NewEvent;
use claude_monitor::store::{Store, StoreOptions};
use serde_json::json;
use tempfile::TempDir;

const INTERVAL: Duration = Duration::from_secs(60);
const WINDOW: Duration = Duration::from_millis(50);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
#[ignore = "until the safety net source lands"]
fn the_default_interval_is_sixty_seconds() {
    assert_eq!(SAFETY_NET_INTERVAL, Duration::from_secs(60));
}

#[test]
#[ignore = "until the safety net source lands"]
fn it_does_not_fire_before_the_interval() {
    let clock = ManualClock::new();
    let mut net = SafetyNetSource::new(INTERVAL, clock.clone());

    assert!(!net.wait(Duration::ZERO));
    clock.advance(INTERVAL - ms(1));

    assert!(!net.wait(Duration::ZERO));
    assert!(!net.due(clock.now()));
}

#[test]
#[ignore = "until the safety net source lands"]
fn it_fires_once_at_the_interval() {
    let clock = ManualClock::new();
    let mut net = SafetyNetSource::new(INTERVAL, clock.clone());

    clock.advance(INTERVAL);

    assert!(net.wait(Duration::ZERO));
    assert!(!net.wait(Duration::ZERO), "one fire per interval");
}

#[test]
#[ignore = "until the safety net source lands"]
fn it_fires_again_every_interval() {
    let clock = ManualClock::new();
    let mut net = SafetyNetSource::new(INTERVAL, clock.clone());

    for round in 0..4 {
        clock.advance(INTERVAL - ms(1));
        assert!(!net.wait(Duration::ZERO), "round {round} early");
        clock.advance(ms(1));
        assert!(net.wait(Duration::ZERO), "round {round} on time");
    }
}

#[test]
#[ignore = "until the safety net source lands"]
fn a_long_gap_fires_once_without_catching_up() {
    let clock = ManualClock::new();
    let mut net = SafetyNetSource::new(INTERVAL, clock.clone());

    clock.advance(INTERVAL * 10);

    assert!(net.wait(Duration::ZERO));
    assert!(!net.wait(Duration::ZERO), "no burst of missed fires");
    clock.advance(INTERVAL);
    assert!(net.wait(Duration::ZERO));
}

#[test]
#[ignore = "until the safety net source lands"]
fn due_takes_the_time_as_an_argument() {
    let clock = ManualClock::new();
    let start = clock.now();
    let mut net = SafetyNetSource::new(INTERVAL, clock);

    assert!(!net.due(start + INTERVAL - ms(1)));
    assert!(net.due(start + INTERVAL));
    assert!(!net.due(start + INTERVAL + ms(1)));
    assert!(net.due(start + INTERVAL * 2));
}

#[test]
#[ignore = "until the safety net source lands"]
fn a_blocking_wait_fires_when_the_interval_elapses() {
    let mut net = SafetyNetSource::new(ms(50), SystemMonotonic);
    let started = Instant::now();

    assert!(!net.wait(ms(10)), "shorter than the interval times out");
    assert!(net.wait(Duration::from_secs(2)));

    assert!(started.elapsed() >= ms(50));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
#[ignore = "until the safety net source lands"]
fn a_dropped_event_is_repaired_by_the_next_safety_net_fire() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.db");
    let writer = Store::open(&path, &StoreOptions::default()).unwrap();
    let clock = ManualClock::new();
    let net = SafetyNetSource::new(INTERVAL, clock.clone());
    let mut feed = ChangeFeed::new(
        Store::open(&path, &StoreOptions::default()).unwrap(),
        net,
        clock.clone(),
    )
    .unwrap();

    // No source wake accompanies this write: the event was dropped.
    let seq = writer
        .append_event(&NewEvent {
            at: "2026-10-12T14:03:22Z",
            entity_kind: "session",
            entity_id: "s-1",
            kind: "state",
            payload: &json!({}),
            author: &Author::System {
                component: "hook".to_owned(),
            },
        })
        .unwrap();
    assert!(feed.poll().unwrap().is_empty(), "nothing wakes the feed");

    clock.advance(INTERVAL);
    assert!(feed.poll().unwrap().is_empty(), "the throttle window opens");
    clock.advance(WINDOW);
    let rows = feed.poll().unwrap();

    assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![seq]);
}
