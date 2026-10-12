//! Spike R8: does a notify watch on the db's parent directory fire for commits
//! made by another process to a WAL-mode SQLite database, and how fast?
//!
//! Run: cargo test --test wal_notify -- --ignored --nocapture
// Spike code: panicking on setup failure and plain arithmetic on timestamps is fine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::Connection;

const ENV_DIR: &str = "WAL_SPIKE_DIR";
const ENV_COUNT: &str = "WAL_SPIKE_COUNT";
const ENV_SLEEP_US: &str = "WAL_SPIKE_SLEEP_US";
const COMMITS_FILE: &str = "commits.txt";
const DEBOUNCE: Duration = Duration::from_millis(50);
// Time allowed for late events to arrive after the writer exits.
const DRAIN: Duration = Duration::from_millis(500);
const SLEEP_BETWEEN_COMMITS_US: u64 = 2_000;

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos() as u64
}

/// Child-process entry point; a no-op unless the env gate is set.
#[test]
#[ignore]
fn writer_child() {
    let Ok(dir) = std::env::var(ENV_DIR) else {
        return;
    };
    let count: u32 = std::env::var(ENV_COUNT).unwrap().parse().unwrap();
    let sleep_us: u64 = std::env::var(ENV_SLEEP_US).unwrap().parse().unwrap();
    let conn = Connection::open(Path::new(&dir).join("state.db")).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    let mut stamps = Vec::new();
    for i in 1..=count {
        conn.execute("INSERT INTO t (id, v) VALUES (?1, 'x')", [i])
            .unwrap();
        stamps.push(format!("{i} {}", now_nanos()));
        if sleep_us > 0 {
            std::thread::sleep(Duration::from_micros(sleep_us));
        }
    }
    drop(conn);
    std::fs::write(Path::new(&dir).join(COMMITS_FILE), stamps.join("\n")).unwrap();
}

struct Event {
    at: u64,
    file: String,
}

struct Run {
    commits: Vec<u64>,
    events: Vec<Event>,
    final_max_id: i64,
}

fn run_scenario(count: u32, sleep_us: u64) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let db = dir.join("state.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
            [],
        )
        .unwrap();
    }

    let (tx, rx) = mpsc::channel::<Event>();
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            let at = now_nanos();
            if let Ok(ev) = res {
                for p in ev.paths {
                    let file = p
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let _ = tx.send(Event { at, file });
                }
            }
        },
        notify::Config::default(),
    )
    .unwrap();
    watcher.watch(dir, RecursiveMode::NonRecursive).unwrap();

    let status = Command::new(std::env::current_exe().unwrap())
        .args(["writer_child", "--exact", "--ignored", "--nocapture"])
        .env(ENV_DIR, dir)
        .env(ENV_COUNT, count.to_string())
        .env(ENV_SLEEP_US, sleep_us.to_string())
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "writer child failed");

    std::thread::sleep(DRAIN);
    let events: Vec<Event> = rx
        .try_iter()
        .filter(|e| e.file.starts_with("state.db"))
        .collect();

    let commits: Vec<u64> = std::fs::read_to_string(dir.join(COMMITS_FILE))
        .unwrap()
        .lines()
        .map(|l| l.split(' ').nth(1).unwrap().parse().unwrap())
        .collect();

    let conn = Connection::open(&db).unwrap();
    let final_max_id: i64 = conn
        .query_row("SELECT max(id) FROM t", [], |r| r.get(0))
        .unwrap();
    Run {
        commits,
        events,
        final_max_id,
    }
}

fn percentile(sorted: &[i64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx] as f64 / 1e6
}

fn report_signed(label: &str, mut lat: Vec<i64>, missed: usize) {
    lat.sort_unstable();
    println!(
        "  {label}: missed={missed} p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms",
        percentile(&lat, 0.50),
        percentile(&lat, 0.95),
        percentile(&lat, 0.99),
        percentile(&lat, 1.0),
    );
}

/// Fixed window anchored at the first event of a group: fires once, DEBOUNCE later.
fn throttled_fires(event_times: &[u64]) -> Vec<u64> {
    let window = DEBOUNCE.as_nanos() as u64;
    let mut fires: Vec<u64> = Vec::new();
    for &t in event_times {
        if fires.last().is_none_or(|&f| t > f) {
            fires.push(t + window);
        }
    }
    fires
}

/// Trailing-edge debounce: a burst of events fires once, DEBOUNCE after its last event.
fn debounced_fires(event_times: &[u64]) -> Vec<u64> {
    let window = DEBOUNCE.as_nanos() as u64;
    let mut fires = Vec::new();
    let mut last: Option<u64> = None;
    for &t in event_times {
        if let Some(prev) = last {
            if t - prev >= window {
                fires.push(prev + window);
            }
        }
        last = Some(t);
    }
    if let Some(prev) = last {
        fires.push(prev + window);
    }
    fires
}

fn analyse(name: &str, run: &Run) {
    let commits = &run.commits;
    let mut times: Vec<u64> = run.events.iter().map(|e| e.at).collect();
    times.sort_unstable();

    let mut by_file: BTreeMap<&str, usize> = BTreeMap::new();
    for e in &run.events {
        *by_file.entry(e.file.as_str()).or_default() += 1;
    }
    let data_times: Vec<u64> = {
        let mut v: Vec<u64> = run
            .events
            .iter()
            .filter(|e| e.file == "state.db-wal" || e.file == "state.db")
            .map(|e| e.at)
            .collect();
        v.sort_unstable();
        v
    };

    println!("== {name} ==");
    println!(
        "  commits={} events={} (events/commit={:.2}) by file={by_file:?}",
        commits.len(),
        run.events.len(),
        run.events.len() as f64 / commits.len() as f64
    );

    // The writer stamps after COMMIT returns, and inotify fires while the write
    // happens, so an event usually precedes its own stamp. Attribute to commit i
    // the first event after stamp i-1; latency is signed (<= 0: event beat the stamp).
    let mut lat = Vec::new();
    let mut missed_window = 0;
    let mut missed_any = 0;
    for (i, &c) in commits.iter().enumerate() {
        let prev = if i == 0 { 0 } else { commits[i - 1] };
        match data_times.iter().find(|&&t| t > prev) {
            Some(&t) if t <= c.saturating_add(DRAIN.as_nanos() as u64) => {
                lat.push(t as i64 - c as i64)
            }
            _ => missed_any += 1,
        }
        let next = commits.get(i + 1).copied().unwrap_or(u64::MAX);
        if !data_times.iter().any(|&t| t > prev && t < next) {
            missed_window += 1;
        }
    }
    println!(
        "  commits with no db/db-wal event between neighbouring stamps: {missed_window}; unattributable: {missed_any}"
    );
    report_signed("event time minus commit stamp", lat, missed_any);

    let trailing = debounced_fires(&data_times);
    let throttled = throttled_fires(&data_times);
    for (label, fires) in [
        ("trailing debounce", &trailing),
        ("start-anchored window", &throttled),
    ] {
        let mut dlat = Vec::new();
        let mut dmissed = 0;
        for &c in commits {
            match fires.iter().find(|&&f| f >= c) {
                Some(&f) => dlat.push((f - c) as i64),
                None => dmissed += 1,
            }
        }
        println!(
            "  {label} {}ms: fires={}",
            DEBOUNCE.as_millis(),
            fires.len()
        );
        report_signed("latency to first later fire", dlat, dmissed);
        let last_commit = commits.last().copied().unwrap_or(0);
        let last_fire = fires.last().copied().unwrap_or(0);
        println!(
            "    final state: max(id)={} expected={} last fire after last commit={}",
            run.final_max_id,
            commits.len(),
            last_fire >= last_commit
        );
    }
    assert_eq!(run.final_max_id, commits.len() as i64);
}

#[test]
#[ignore]
fn wal_change_notification() {
    println!(
        "kernel: {}",
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
    );
    let steady = run_scenario(1000, SLEEP_BETWEEN_COMMITS_US);
    analyse("steady: 1000 commits, 2ms apart", &steady);
    let burst = run_scenario(50, 0);
    analyse("burst: 50 commits back-to-back", &burst);
}
