// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::PathBuf;
use std::time::Duration;

use claude_monitor::model::Author;
use claude_monitor::store::events::{EventRow, NewEvent};
use claude_monitor::store::{Store, StoreOptions};
use serde_json::json;
use tempfile::TempDir;

use claude_monitor::feed::{
    ChangeFeed, EventSource, FakeHandle, FakeSource, FsWatcherSource, ManualClock,
};

const AT: &str = "2026-10-12T14:03:22Z";
const WINDOW: Duration = Duration::from_millis(50);
const REAL_OS_WAKE_TIMEOUT: Duration = Duration::from_secs(2);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn db_path(dir: &TempDir) -> PathBuf {
    dir.path().join("state.db")
}

fn open(dir: &TempDir) -> Store {
    Store::open(&db_path(dir), &StoreOptions::default()).unwrap()
}

fn append(store: &Store, n: usize) -> i64 {
    store
        .append_event(&NewEvent {
            at: AT,
            entity_kind: "session",
            entity_id: &format!("s-{n}"),
            kind: "state",
            payload: &json!({ "n": n }),
            author: &Author::System {
                component: "hook".to_owned(),
            },
        })
        .unwrap()
}

fn seqs(rows: &[EventRow]) -> Vec<i64> {
    rows.iter().map(|row| row.seq).collect()
}

struct Rig {
    _dir: TempDir,
    writer: Store,
    feed: ChangeFeed<FakeSource, ManualClock>,
    wake: FakeHandle,
    clock: ManualClock,
}

fn rig() -> Rig {
    let dir = TempDir::new().unwrap();
    let writer = open(&dir);
    let (source, wake) = FakeSource::new();
    let clock = ManualClock::new();
    let feed = ChangeFeed::new(open(&dir), source, clock.clone()).unwrap();
    Rig {
        _dir: dir,
        writer,
        feed,
        wake,
        clock,
    }
}

#[test]
fn a_committed_event_yields_one_poll_result_and_advances_the_cursor() {
    let mut rig = rig();
    let seq = append(&rig.writer, 1);
    rig.wake.wake();

    assert!(rig.feed.poll().unwrap().is_empty(), "window not elapsed");
    rig.clock.advance(WINDOW);
    let rows = rig.feed.poll().unwrap();

    assert_eq!(seqs(&rows), vec![seq]);
    assert_eq!(rows[0].entity_id, "s-1");
    assert_eq!(rig.feed.cursor(), seq);
    assert!(rig.feed.poll().unwrap().is_empty(), "no repeat delivery");
}

#[test]
fn a_burst_of_wakes_inside_one_window_fires_once() {
    let mut rig = rig();
    let mut fires = 0;
    let mut delivered = Vec::new();

    for n in 0..10 {
        delivered.push(append(&rig.writer, n));
        rig.wake.wake();
        let rows = rig.feed.poll().unwrap();
        if !rows.is_empty() {
            fires += 1;
        }
        rig.clock.advance(ms(4));
    }
    assert_eq!(fires, 0, "nothing fires before the window closes");

    rig.clock.advance(WINDOW);
    let rows = rig.feed.poll().unwrap();

    assert_eq!(seqs(&rows), delivered);
    assert!(rig.feed.poll().unwrap().is_empty());
}

#[test]
fn wakes_every_2ms_fire_every_window_instead_of_starving() {
    let mut rig = rig();
    let mut fire_times = Vec::new();
    let mut delivered = Vec::new();
    let mut elapsed = Duration::ZERO;

    while elapsed < ms(500) {
        append(&rig.writer, 0);
        rig.wake.wake();
        let rows = rig.feed.poll().unwrap();
        if !rows.is_empty() {
            fire_times.push(elapsed);
            delivered.extend(seqs(&rows));
        }
        rig.clock.advance(ms(2));
        elapsed += ms(2);
    }

    assert!(
        fire_times.len() >= 8,
        "a trailing debounce would fire at most once: {fire_times:?}"
    );
    for pair in fire_times.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= WINDOW && gap <= WINDOW + ms(4),
            "fires are one window apart, got {gap:?} in {fire_times:?}"
        );
    }
    assert!(delivered.windows(2).all(|p| p[0] < p[1]), "in order, once");
}

#[test]
fn the_last_event_of_a_burst_is_never_lost() {
    let mut rig = rig();
    let mut delivered = Vec::new();

    for n in 0..30 {
        append(&rig.writer, n);
        rig.wake.wake();
        delivered.extend(seqs(&rig.feed.poll().unwrap()));
        rig.clock.advance(ms(3));
    }
    let last = append(&rig.writer, 99);
    rig.wake.wake();
    rig.clock.advance(WINDOW);
    delivered.extend(seqs(&rig.feed.poll().unwrap()));
    rig.clock.advance(WINDOW);
    delivered.extend(seqs(&rig.feed.poll().unwrap()));

    assert_eq!(delivered, (1..=last).collect::<Vec<_>>());
    assert_eq!(rig.feed.cursor(), rig.writer.last_seq().unwrap());
}

#[test]
fn resume_from_replays_exactly_the_missed_events() {
    let mut rig = rig();
    let all: Vec<i64> = (0..5).map(|n| append(&rig.writer, n)).collect();

    rig.feed.resume_from(all[1]);
    let rows = rig.feed.poll().unwrap();

    assert_eq!(seqs(&rows), all[2..].to_vec());
    assert_eq!(rig.feed.cursor(), all[4]);
    assert!(rig.feed.poll().unwrap().is_empty());
}

#[test]
fn resume_from_a_cursor_beyond_the_end_returns_nothing() {
    let mut rig = rig();
    append(&rig.writer, 1);

    rig.feed.resume_from(9_999);

    assert!(rig.feed.poll().unwrap().is_empty());
    rig.wake.wake();
    rig.clock.advance(WINDOW);
    assert!(rig.feed.poll().unwrap().is_empty());
}

#[test]
fn two_feeds_on_one_store_keep_independent_cursors() {
    let dir = TempDir::new().unwrap();
    let writer = open(&dir);
    let clock = ManualClock::new();
    let (source_a, wake_a) = FakeSource::new();
    let (source_b, wake_b) = FakeSource::new();
    let mut a = ChangeFeed::new(open(&dir), source_a, clock.clone()).unwrap();
    let mut b = ChangeFeed::new(open(&dir), source_b, clock.clone()).unwrap();

    let first = append(&writer, 1);
    wake_a.wake();
    wake_b.wake();
    assert!(a.poll().unwrap().is_empty());
    assert!(b.poll().unwrap().is_empty());
    clock.advance(WINDOW);
    assert_eq!(seqs(&a.poll().unwrap()), vec![first]);

    let second = append(&writer, 2);
    wake_a.wake();
    wake_b.wake();
    assert!(a.poll().unwrap().is_empty());
    clock.advance(WINDOW);
    assert_eq!(seqs(&a.poll().unwrap()), vec![second]);
    assert_eq!(seqs(&b.poll().unwrap()), vec![first, second]);
    assert_eq!((a.cursor(), b.cursor()), (second, second));
}

#[test]
fn dropping_a_feed_stops_its_source() {
    let rig = rig();
    assert!(!rig.wake.is_closed());

    drop(rig.feed);

    assert!(rig.wake.is_closed());
}

#[test]
#[cfg(target_os = "linux")]
fn the_fs_watcher_wakes_when_another_handle_commits() {
    let dir = TempDir::new().unwrap();
    let writer = open(&dir);
    let mut source = FsWatcherSource::watch(dir.path()).unwrap();

    append(&writer, 1);

    assert!(
        source.wait(REAL_OS_WAKE_TIMEOUT),
        "no wake within {REAL_OS_WAKE_TIMEOUT:?}"
    );
}
