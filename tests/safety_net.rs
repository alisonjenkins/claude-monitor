// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use claude_monitor::feed::{
    ChangeFeed, EventSource, ManualClock, Monotonic, SafetyNetSource, SystemMonotonic,
    SAFETY_NET_INTERVAL,
};
use claude_monitor::live::spawn_live;
use claude_monitor::model::{Author, Session, State};
use claude_monitor::store::events::NewEvent;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::tmux::PaneLocation;
use serde_json::json;
use tempfile::TempDir;

const INTERVAL: Duration = Duration::from_secs(60);
const WINDOW: Duration = Duration::from_millis(50);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn the_default_interval_is_sixty_seconds() {
    assert_eq!(SAFETY_NET_INTERVAL, Duration::from_secs(60));
}

#[test]
fn it_does_not_fire_before_the_interval() {
    let clock = ManualClock::new();
    let mut net = SafetyNetSource::new(INTERVAL, clock.clone());

    assert!(!net.wait(Duration::ZERO));
    clock.advance(INTERVAL - ms(1));

    assert!(!net.wait(Duration::ZERO));
    assert!(!net.due(clock.now()));
}

#[test]
fn it_fires_once_at_the_interval() {
    let clock = ManualClock::new();
    let mut net = SafetyNetSource::new(INTERVAL, clock.clone());

    clock.advance(INTERVAL);

    assert!(net.wait(Duration::ZERO));
    assert!(!net.wait(Duration::ZERO), "one fire per interval");
}

#[test]
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
fn a_blocking_wait_fires_when_the_interval_elapses() {
    let mut net = SafetyNetSource::new(ms(50), SystemMonotonic);
    let started = Instant::now();

    assert!(!net.wait(ms(10)), "shorter than the interval times out");
    assert!(net.wait(Duration::from_secs(2)));

    assert!(started.elapsed() >= ms(50));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
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

#[test]
fn a_pane_that_vanished_without_a_tmux_hook_is_interrupted_by_the_next_tick() {
    let dir = TempDir::new().unwrap();
    let fake = FakeEnv::new();
    fake.fs.add_dir("/w");
    fake.tmux.add_pane(
        "%7",
        PaneLocation {
            session_name: "main".to_owned(),
            window_name: "claude".to_owned(),
            window_index: "1".to_owned(),
            pane_index: "0".to_owned(),
        },
    );
    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    let mut session = Session::new("id-1", "claude", "sess-1", "/w", "2026-10-12T09:00:00Z");
    session.state = State::Working;
    session.tmux_pane = Some("%7".to_owned());
    store.upsert_session(&session).unwrap();
    // The pane goes away; no tmux hook runs and nobody calls `reconcile --pane`.
    fake.tmux.remove_pane("%7");
    let (tx, rx) = mpsc::channel();

    let live = spawn_live(
        Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap(),
        fake.env.clone(),
        vec![Box::new(SafetyNetSource::new(ms(30), SystemMonotonic))],
        None,
        move |report| {
            let _ = tx.send(report.changed());
        },
    )
    .unwrap();

    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));
    drop(live);
    let state = store
        .get_session_by_agent_id("claude", "sess-1")
        .unwrap()
        .unwrap()
        .state;
    assert_eq!(state, State::Interrupted);
}
