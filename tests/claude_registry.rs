#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! The Claude session registry, `<config dir>/sessions/<pid>.json`: live
//! sessions with pid, status and tmux pane. Liveness needs the pid alive and
//! `procStart` equal to the process start time.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use claude_monitor::agents::claude_registry::{
    live_by_session_id, scan, to_state, RegistryKind, RegistryReport, RegistryStatus, TmuxRef,
    MAX_ENTRY_BYTES,
};
use claude_monitor::env::{Env, EnvError, FileMeta, Fs};
use claude_monitor::feed::{ClaudeRegistrySource, EventSource};
use claude_monitor::model::State;
use claude_monitor::testing::FakeEnv;
use serde_json::{json, Value};
use tempfile::TempDir;

const SESSIONS: &str = "/cfg/sessions";
const KEY_SUFFIX: &str = "a3f1c2d4e5b60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const REAL_OS_WAKE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Access {
    ReadToString(PathBuf),
    ReadRange(PathBuf, usize),
    Other(PathBuf),
}

impl Access {
    fn path(&self) -> &Path {
        match self {
            Self::ReadToString(p) | Self::ReadRange(p, _) | Self::Other(p) => p,
        }
    }
}

/// Wraps the fake file system and records every access.
struct SpyFs {
    inner: Arc<claude_monitor::testing::FakeFs>,
    log: Mutex<Vec<Access>>,
}

impl SpyFs {
    fn record(&self, access: Access) {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(access);
    }

    fn log(&self) -> Vec<Access> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn reads_of(&self, path: &str) -> usize {
        self.log()
            .iter()
            .filter(|a| {
                matches!(a, Access::ReadToString(_) | Access::ReadRange(..))
                    && a.path() == Path::new(path)
            })
            .count()
    }
}

impl Fs for SpyFs {
    fn read_to_string(&self, path: &Path) -> Result<String, EnvError> {
        self.record(Access::ReadToString(path.to_owned()));
        self.inner.read_to_string(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>, EnvError> {
        self.inner.list_dir(path)
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf, EnvError> {
        self.record(Access::Other(path.to_owned()));
        self.inner.read_link(path)
    }

    fn metadata(&self, path: &Path) -> Result<FileMeta, EnvError> {
        self.record(Access::Other(path.to_owned()));
        self.inner.metadata(path)
    }

    fn read_range(&self, path: &Path, offset: u64, max_len: usize) -> Result<Vec<u8>, EnvError> {
        self.record(Access::ReadRange(path.to_owned(), max_len));
        self.inner.read_range(path, offset, max_len)
    }
}

struct Rig {
    fake: FakeEnv,
    spy: Arc<SpyFs>,
    env: Env,
}

fn rig() -> Rig {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    let spy = Arc::new(SpyFs {
        inner: fake.fs.clone(),
        log: Mutex::new(Vec::new()),
    });
    let env = Env {
        fs: spy.clone(),
        ..fake.env.clone()
    };
    Rig { fake, spy, env }
}

fn path_of(pid: u32) -> String {
    format!("{SESSIONS}/{pid}.json")
}

/// A registry entry as Claude Code 2.1.293 writes it.
fn entry(pid: u32, session_id: &str) -> Value {
    json!({
        "pid": pid,
        "sessionId": session_id,
        "cwd": "/home/ali/git/proj",
        "startedAt": 1_791_800_000_000_u64,
        "procStart": "5000",
        "version": "2.1.293",
        "kind": "interactive",
        "entrypoint": "cli",
        "tmux": "main:@3.%5",
        "name": "proj",
        "updatedAt": 1_791_800_100_000_u64,
        "status": "idle",
        "statusUpdatedAt": 1_791_800_100_000_u64,
    })
}

fn with(mut value: Value, key: &str, new: Value) -> Value {
    value[key] = new;
    value
}

fn without(mut value: Value, key: &str) -> Value {
    value.as_object_mut().unwrap().remove(key);
    value
}

fn add(rig: &Rig, pid: u32, value: &Value) {
    rig.fake.fs.add_file(path_of(pid), &value.to_string());
}

/// Adds the entry and a running process whose start time matches it.
fn add_live(rig: &Rig, pid: u32, value: &Value) {
    add(rig, pid, value);
    rig.fake.processes.spawn(pid, 5000);
}

fn scan_ok(rig: &Rig) -> RegistryReport {
    scan(&rig.env).unwrap()
}

#[test]
#[ignore = "T045b"]
fn a_normal_entry_is_parsed() {
    let rig = rig();
    add_live(&rig, 4242, &entry(4242, "sess-1"));

    let report = scan_ok(&rig);

    assert_eq!(report.degraded, 0);
    assert_eq!(report.entries.len(), 1);
    let e = &report.entries[0];
    assert_eq!(e.pid, 4242);
    assert_eq!(e.session_id, "sess-1");
    assert_eq!(e.cwd.as_deref(), Some(Path::new("/home/ali/git/proj")));
    assert_eq!(e.started_at_ms, Some(1_791_800_000_000));
    assert_eq!(e.status, Some(RegistryStatus::Idle));
    assert_eq!(e.waiting_for, None);
    assert_eq!(e.kind, RegistryKind::Interactive);
    assert_eq!(e.entrypoint, "cli");
    assert_eq!(e.version.as_deref(), Some("2.1.293"));
    assert!(e.live);
}

#[test]
#[ignore = "T045b"]
fn a_missing_status_is_none() {
    let rig = rig();
    let first_write = without(without(entry(7, "s"), "status"), "updatedAt");
    add_live(&rig, 7, &first_write);

    let report = scan_ok(&rig);

    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].status, None);
}

#[test]
#[ignore = "T045b"]
fn unknown_extra_keys_are_ignored() {
    let rig = rig();
    let value = with(entry(7, "s"), "somethingNew", json!({"nested": [1, 2, 3]}));
    add_live(&rig, 7, &value);

    let report = scan_ok(&rig);

    assert_eq!(report.degraded, 0);
    assert_eq!(report.entries.len(), 1);
}

#[test]
#[ignore = "T045b"]
fn statuses_map_to_states() {
    assert_eq!(to_state(&RegistryStatus::Busy), State::Working);
    assert_eq!(to_state(&RegistryStatus::Waiting), State::NeedsPermission);
    assert_eq!(to_state(&RegistryStatus::Idle), State::Idle);
    assert_eq!(to_state(&RegistryStatus::Shell), State::Working);

    let rig = rig();
    for (pid, status) in [(1, "busy"), (2, "waiting"), (3, "idle"), (4, "shell")] {
        add_live(&rig, pid, &with(entry(pid, "s"), "status", json!(status)));
    }
    let statuses: Vec<_> = scan_ok(&rig).entries.iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        [
            Some(RegistryStatus::Busy),
            Some(RegistryStatus::Waiting),
            Some(RegistryStatus::Idle),
            Some(RegistryStatus::Shell),
        ]
    );
}

#[test]
#[ignore = "T045b"]
fn waiting_keeps_the_waiting_for_text() {
    let rig = rig();
    let value = with(
        with(entry(9, "s"), "status", json!("waiting")),
        "waitingFor",
        json!("permission prompt"),
    );
    add_live(&rig, 9, &value);

    let report = scan_ok(&rig);

    let e = &report.entries[0];
    assert_eq!(e.status, Some(RegistryStatus::Waiting));
    assert_eq!(e.waiting_for.as_deref(), Some("permission prompt"));
}

#[test]
#[ignore = "T045b"]
fn an_alive_pid_with_a_matching_start_time_is_live() {
    let rig = rig();
    add(&rig, 10, &entry(10, "s"));
    rig.fake.processes.spawn(10, 5000);

    assert!(scan_ok(&rig).entries[0].live);
}

#[test]
#[ignore = "T045b"]
fn a_reused_pid_is_stale() {
    let rig = rig();
    add(&rig, 10, &entry(10, "s"));
    rig.fake.processes.spawn(10, 9999);

    let report = scan_ok(&rig);

    assert_eq!(report.entries.len(), 1);
    assert!(!report.entries[0].live);
}

#[test]
#[ignore = "T045b"]
fn a_dead_pid_is_stale() {
    let rig = rig();
    add(&rig, 10, &entry(10, "s"));

    let report = scan_ok(&rig);

    assert_eq!(report.entries.len(), 1);
    assert!(!report.entries[0].live);
}

#[test]
#[ignore = "T045b"]
fn a_killed_session_goes_stale() {
    let rig = rig();
    add_live(&rig, 10, &entry(10, "s"));
    assert!(scan_ok(&rig).entries[0].live);

    rig.fake.processes.kill(10);

    assert!(!scan_ok(&rig).entries[0].live);
}

#[test]
#[ignore = "T045b"]
fn sdk_cli_entries_are_filtered_out() {
    let rig = rig();
    add_live(&rig, 1, &entry(1, "interactive"));
    add_live(
        &rig,
        2,
        &with(entry(2, "print"), "entrypoint", json!("sdk-cli")),
    );

    let report = scan_ok(&rig);

    let ids: Vec<_> = report
        .entries
        .iter()
        .map(|e| e.session_id.as_str())
        .collect();
    assert_eq!(ids, ["interactive"]);
    assert_eq!(report.degraded, 0);
}

#[test]
#[ignore = "T045b"]
fn background_sessions_are_flagged_by_kind() {
    let rig = rig();
    add_live(&rig, 1, &entry(1, "fg"));
    let bg = with(
        with(entry(2, "bg"), "kind", json!("bg")),
        "tmux",
        Value::Null,
    );
    add_live(&rig, 2, &with(bg, "jobId", json!("job-1")));

    let report = scan_ok(&rig);

    let kinds: Vec<_> = report.entries.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [RegistryKind::Interactive, RegistryKind::Background]);
}

#[test]
#[ignore = "T045b"]
fn the_tmux_string_is_split_into_session_window_and_pane() {
    let rig = rig();
    add_live(&rig, 1, &entry(1, "a"));
    add_live(&rig, 2, &with(entry(2, "b"), "tmux", Value::Null));
    add_live(&rig, 3, &without(entry(3, "c"), "tmux"));
    add_live(
        &rig,
        4,
        &with(entry(4, "d"), "tmux", json!("my.sess:x:@12.%345")),
    );

    let report = scan_ok(&rig);

    let tmux: Vec<_> = report.entries.iter().map(|e| e.tmux.clone()).collect();
    assert_eq!(
        tmux,
        [
            Some(TmuxRef {
                session: "main".into(),
                window_id: "@3".into(),
                pane_id: "%5".into(),
            }),
            None,
            None,
            Some(TmuxRef {
                session: "my.sess:x".into(),
                window_id: "@12".into(),
                pane_id: "%345".into(),
            }),
        ]
    );
}

#[test]
#[ignore = "T045b"]
fn key_files_are_never_touched() {
    let rig = rig();
    add_live(&rig, 1, &entry(1, "s"));
    let key = format!("{SESSIONS}/1.{KEY_SUFFIX}.key");
    rig.fake.fs.add_file(&key, "SECRET-KEY-MATERIAL");

    let report = scan_ok(&rig);

    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.degraded, 0, "a key file is not a degraded entry");
    let touched: Vec<_> = rig
        .spy
        .log()
        .iter()
        .filter(|a| a.path().to_string_lossy().ends_with(".key"))
        .cloned()
        .collect();
    assert_eq!(touched, [], "a .key file was accessed");
}

#[test]
#[ignore = "T045b"]
fn a_corrupt_file_is_degraded_after_one_retry() {
    let rig = rig();
    add_live(&rig, 1, &entry(1, "good"));
    rig.fake.fs.add_file(path_of(2), "{\"pid\": 2, \"sessi");

    let report = scan_ok(&rig);

    assert_eq!(report.degraded, 1);
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].session_id, "good");
    assert_eq!(rig.spy.reads_of(&path_of(2)), 2, "read, retry once, stop");
    assert_eq!(rig.spy.reads_of(&path_of(1)), 1, "a good file is read once");
}

#[test]
#[ignore = "T045b"]
fn a_file_missing_its_session_id_is_degraded() {
    let rig = rig();
    add_live(&rig, 1, &without(entry(1, "x"), "sessionId"));

    let report = scan_ok(&rig);

    assert_eq!(report.degraded, 1);
    assert!(report.entries.is_empty());
}

#[test]
#[ignore = "T045b"]
fn an_absent_directory_is_an_empty_result() {
    let rig = rig();

    let report = scan_ok(&rig);

    assert_eq!(report, RegistryReport::default());
}

#[test]
#[ignore = "T045b"]
fn the_directory_follows_the_config_dir_override_then_home() {
    let rig = rig();
    rig.fake.vars.set("HOME", "/home/x");
    rig.fake.fs.add_file(
        "/home/x/.claude/sessions/5.json",
        &entry(5, "from-home").to_string(),
    );
    add_live(&rig, 6, &entry(6, "from-override"));

    let ids: Vec<_> = scan_ok(&rig)
        .entries
        .iter()
        .map(|e| e.session_id.clone())
        .collect();

    assert_eq!(ids, ["from-override"]);
}

#[test]
#[ignore = "T045b"]
fn no_config_dir_is_an_error() {
    let fake = FakeEnv::new();

    assert!(scan(&fake.env).is_err());
}

#[test]
#[ignore = "T045b"]
fn a_huge_file_is_read_in_bounded_bytes_and_degraded() {
    let rig = rig();
    let padding = "x".repeat(MAX_ENTRY_BYTES * 4);
    add_live(&rig, 1, &with(entry(1, "big"), "name", json!(padding)));
    add_live(&rig, 2, &entry(2, "small"));

    let report = scan_ok(&rig);

    assert_eq!(MAX_ENTRY_BYTES, 16 * 1024);
    let log = rig.spy.log();
    assert!(
        log.iter().all(|a| !matches!(a, Access::ReadToString(_))),
        "unbounded read: {log:?}"
    );
    for access in &log {
        if let Access::ReadRange(_, max_len) = access {
            assert!(*max_len <= MAX_ENTRY_BYTES, "{access:?}");
        }
    }
    assert_eq!(report.degraded, 1);
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].session_id, "small");
}

#[test]
#[ignore = "T045b"]
fn two_files_with_one_session_id_do_not_collide() {
    let rig = rig();
    add_live(&rig, 10, &entry(10, "same"));
    add_live(&rig, 11, &entry(11, "same"));

    let report = scan_ok(&rig);

    let pids: Vec<_> = report.entries.iter().map(|e| e.pid).collect();
    assert_eq!(pids, [10, 11]);
}

#[test]
#[ignore = "T045b"]
fn clear_keeps_the_pid_and_changes_the_session_id() {
    let rig = rig();
    add_live(&rig, 10, &entry(10, "before-clear"));
    assert_eq!(scan_ok(&rig).entries[0].session_id, "before-clear");

    add_live(&rig, 10, &entry(10, "after-clear"));

    let report = scan_ok(&rig);
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].pid, 10);
    assert_eq!(report.entries[0].session_id, "after-clear");
}

#[test]
#[ignore = "T045b"]
fn entries_are_sorted_by_pid_numerically() {
    let rig = rig();
    for pid in [900, 12, 5000, 7] {
        add_live(&rig, pid, &entry(pid, &format!("s{pid}")));
    }

    let pids: Vec<_> = scan_ok(&rig).entries.iter().map(|e| e.pid).collect();

    assert_eq!(pids, [7, 12, 900, 5000]);
}

#[test]
#[ignore = "T045b"]
fn live_by_session_id_keeps_only_live_entries() {
    let rig = rig();
    add_live(&rig, 1, &entry(1, "alive"));
    add(&rig, 2, &entry(2, "dead"));

    let report = scan_ok(&rig);
    let live = live_by_session_id(&report);

    assert_eq!(live.len(), 1);
    assert_eq!(live.get("alive").map(|e| e.pid), Some(1));
}

#[test]
#[ignore = "T045b"]
#[cfg(target_os = "linux")]
fn the_registry_watch_wakes_on_json_files_only() {
    let dir = TempDir::new().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let mut source = ClaudeRegistrySource::watch(&sessions).unwrap();
    let ms = Duration::from_millis;

    let key = sessions.join(format!("42.{KEY_SUFFIX}.key"));
    std::fs::write(&key, "k").unwrap();
    assert!(!source.wait(ms(300)), "a key file must not wake");

    let file = sessions.join("42.json");
    std::fs::write(&file, "{}").unwrap();
    assert!(source.wait(REAL_OS_WAKE_TIMEOUT), "new registry file");
    while source.wait(ms(100)) {}

    std::fs::write(&file, "{\"a\":1}").unwrap();
    assert!(source.wait(REAL_OS_WAKE_TIMEOUT), "modified registry file");
    while source.wait(ms(100)) {}

    std::fs::remove_file(&key).unwrap();
    assert!(!source.wait(ms(300)), "removing a key file must not wake");

    std::fs::remove_file(&file).unwrap();
    assert!(source.wait(REAL_OS_WAKE_TIMEOUT), "removed registry file");
}

#[test]
#[ignore = "T045b"]
fn watching_a_missing_registry_directory_is_an_error_naming_it() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nope");

    let err = ClaudeRegistrySource::watch(&missing).unwrap_err();

    assert!(err.to_string().contains("nope"), "{err}");
}
