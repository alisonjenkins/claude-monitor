#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! `watch`: one log line per session state change, from the same sources the
//! dashboard uses.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use claude_monitor::feed::{FakeHandle, FakeSource};
use claude_monitor::live::LiveSources;
use claude_monitor::model::{Session, State};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::watch::{notifications, state_changes, watch};
use serde_json::json;
use tempfile::TempDir;

const AT: &str = "2026-10-12T12:00:00Z";
const PATIENCE: Duration = Duration::from_secs(5);

fn session(id: &str, state: State) -> Session {
    let mut s = Session::new(id, "claude", id, "/w/proj", "2026-10-12T09:00:00Z");
    s.state = state;
    s
}

#[test]
fn the_first_pass_reports_only_active_sessions() {
    let mut seen = BTreeMap::new();
    let sessions = [
        session("aaaaaaaa-1", State::Working),
        session("bbbbbbbb-2", State::Finished),
    ];

    let lines = state_changes(&mut seen, &sessions, AT, true);

    assert_eq!(
        lines,
        [format!("{AT} claude aaaaaaaa working (new) /w/proj")]
    );
    assert_eq!(seen.len(), 2, "finished sessions are still remembered");
}

#[test]
fn a_change_is_one_line_naming_the_old_state() {
    let mut seen = BTreeMap::new();
    state_changes(
        &mut seen,
        &[session("aaaaaaaa-1", State::Working)],
        AT,
        true,
    );

    let lines = state_changes(&mut seen, &[session("aaaaaaaa-1", State::Idle)], AT, false);

    assert_eq!(
        lines,
        [format!("{AT} claude aaaaaaaa idle (was working) /w/proj")]
    );
}

#[test]
fn an_unchanged_session_is_silent() {
    let mut seen = BTreeMap::new();
    let sessions = [session("aaaaaaaa-1", State::Idle)];
    state_changes(&mut seen, &sessions, AT, true);

    assert!(state_changes(&mut seen, &sessions, AT, false).is_empty());
}

#[test]
fn a_session_that_appears_later_is_reported_in_any_state() {
    let mut seen = BTreeMap::new();
    state_changes(&mut seen, &[], AT, true);

    let lines = state_changes(
        &mut seen,
        &[session("cccccccc-3", State::Finished)],
        AT,
        false,
    );

    assert_eq!(
        lines,
        [format!("{AT} claude cccccccc finished (new) /w/proj")]
    );
}

#[test]
fn the_first_pass_notifies_nothing() {
    let sessions = [
        session("aaaaaaaa-1", State::Idle),
        session("bbbbbbbb-2", State::NeedsPermission),
    ];

    assert!(notifications(&BTreeMap::new(), &sessions, true).is_empty());
}

#[test]
fn a_change_into_permission_or_idle_notifies_once_by_project() {
    let mut seen = BTreeMap::new();
    state_changes(
        &mut seen,
        &[
            session("aaaaaaaa-1", State::Working),
            session("bbbbbbbb-2", State::Working),
        ],
        AT,
        true,
    );
    let next = [
        session("aaaaaaaa-1", State::NeedsPermission),
        session("bbbbbbbb-2", State::Finished),
    ];

    let notes = notifications(&seen, &next, false);

    assert_eq!(notes, [("proj".to_owned(), State::NeedsPermission)]);
    state_changes(&mut seen, &next, AT, false);
    assert!(
        notifications(&seen, &next, false).is_empty(),
        "an unchanged state is not notified again"
    );
}

#[test]
fn a_session_that_appears_idle_after_the_first_pass_notifies() {
    let mut seen = BTreeMap::new();
    state_changes(&mut seen, &[], AT, true);

    let notes = notifications(&seen, &[session("cccccccc-3", State::Idle)], false);

    assert_eq!(notes, [("proj".to_owned(), State::Idle)]);
}

#[test]
fn the_project_is_the_repository_name_else_the_directory() {
    let mut with_repo = session("aaaaaaaa-1", State::Idle);
    with_repo.repo_root = Some("/home/u/git/nix-config".to_owned());

    let notes = notifications(&BTreeMap::new(), &[with_repo], false);

    assert_eq!(notes, [("nix-config".to_owned(), State::Idle)]);
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Shared {
    fn text(&self) -> String {
        String::from_utf8(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap()
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn registry_entry(status: &str) -> String {
    json!({
        "pid": 4242, "sessionId": "sess-1", "cwd": "/w/proj", "startedAt": 1_791_800_000_000_u64,
        "procStart": "5000", "version": "2.1.293", "kind": "interactive", "entrypoint": "cli",
        "tmux": "main:@3.%5", "status": status,
    })
    .to_string()
}

struct Running {
    stop: Arc<AtomicBool>,
    out: Shared,
    err: Shared,
    wake: FakeHandle,
    thread: thread::JoinHandle<()>,
}

fn start(dir: &TempDir, fake: &FakeEnv, warnings: Vec<String>) -> Running {
    let (source, wake) = FakeSource::new();
    let stop = Arc::new(AtomicBool::new(false));
    let (out, err) = (Shared::default(), Shared::default());
    let (path, env) = (dir.path().to_owned(), fake.env.clone());
    let (flag, mut o, mut e) = (Arc::clone(&stop), out.clone(), err.clone());
    let thread = thread::spawn(move || {
        let store = Store::open_in_dir(&path, &StoreOptions::default()).unwrap();
        let built = LiveSources {
            sources: vec![Box::new(source)],
            proc_exit: None,
            warnings,
        };
        watch(&store, &env, built, &flag, &mut o, &mut e);
    });
    Running {
        stop,
        out,
        err,
        wake,
        thread,
    }
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < PATIENCE {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn live_fake() -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    fake.vars.set("COPILOT_HOME", "/cop");
    fake.fs.add_dir("/w/proj");
    fake.fs
        .add_file("/cfg/sessions/4242.json", &registry_entry("idle"));
    fake.processes.spawn(4242, 5000);
    fake
}

#[test]
fn a_status_change_in_the_registry_is_logged() {
    let (dir, fake) = (TempDir::new().unwrap(), live_fake());
    let running = start(&dir, &fake, Vec::new());
    running.wake.wake();
    eventually("the session to appear", || {
        running.out.text().contains(" idle (new) /w/proj")
    });

    fake.fs
        .add_file("/cfg/sessions/4242.json", &registry_entry("busy"));
    running.wake.wake();

    eventually("the busy line", || {
        running.out.text().contains(" working (was idle) /w/proj")
    });
    running.stop.store(true, Ordering::SeqCst);
    running.thread.join().unwrap();
    for line in running.out.text().lines() {
        assert!(line.starts_with("2026-10-12T00:00:00Z claude "), "{line}");
    }
}

#[test]
fn a_dead_process_is_logged_as_interrupted() {
    let (dir, fake) = (TempDir::new().unwrap(), live_fake());
    let running = start(&dir, &fake, Vec::new());
    running.wake.wake();
    eventually("the session to appear", || {
        running.out.text().contains(" idle (new)")
    });

    fake.processes.kill(4242);
    running.wake.wake();

    eventually("the interrupted line", || {
        running.out.text().contains(" interrupted (was idle)")
    });
    running.stop.store(true, Ordering::SeqCst);
    running.thread.join().unwrap();
}

#[test]
fn a_corrupt_row_does_not_silence_the_watch() {
    let (dir, fake) = (TempDir::new().unwrap(), live_fake());
    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    store.upsert_session(&session("bad", State::Idle)).unwrap();
    let conn = store.connection();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute("UPDATE sessions SET state = 'bogus' WHERE id = 'bad'", [])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
        .unwrap();
    drop(store);
    let running = start(&dir, &fake, Vec::new());

    running.wake.wake();

    eventually("the session to appear", || {
        running.out.text().contains(" idle (new) /w/proj")
    });
    running.stop.store(true, Ordering::SeqCst);
    running.thread.join().unwrap();
}

#[test]
fn warnings_go_to_the_error_stream_once() {
    let (dir, fake) = (TempDir::new().unwrap(), FakeEnv::new());
    fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    fake.vars.set("COPILOT_HOME", "/cop");
    let running = start(
        &dir,
        &fake,
        vec!["claude registry: /x does not exist".to_owned()],
    );
    for _ in 0..2 {
        running.wake.wake();
        thread::sleep(Duration::from_millis(300));
    }

    running.stop.store(true, Ordering::SeqCst);
    running.thread.join().unwrap();
    assert_eq!(
        running.err.text(),
        "warning: claude registry: /x does not exist\n"
    );
    assert_eq!(running.out.text(), "");
}

#[test]
fn watch_sends_a_desktop_notification_on_a_new_transition_only() {
    let (dir, fake) = (TempDir::new().unwrap(), live_fake());
    let running = start(&dir, &fake, Vec::new());
    running.wake.wake();
    eventually("the session to appear", || {
        running.out.text().contains(" idle (new) /w/proj")
    });
    assert!(
        fake.notifier.sent().is_empty(),
        "an existing session is not news"
    );

    fake.fs
        .add_file("/cfg/sessions/4242.json", &registry_entry("busy"));
    running.wake.wake();
    eventually("the busy line", || {
        running.out.text().contains(" working (was idle)")
    });
    assert!(fake.notifier.sent().is_empty());

    fake.fs
        .add_file("/cfg/sessions/4242.json", &registry_entry("idle"));
    running.wake.wake();
    eventually("the notification", || !fake.notifier.sent().is_empty());
    running.wake.wake();
    thread::sleep(Duration::from_millis(200));

    running.stop.store(true, Ordering::SeqCst);
    running.thread.join().unwrap();
    assert_eq!(
        fake.notifier.sent(),
        [("Claude Code".to_owned(), "proj: IDLE".to_owned())]
    );
}

#[test]
fn setting_stop_ends_the_watch() {
    let (dir, fake) = (TempDir::new().unwrap(), live_fake());
    let running = start(&dir, &fake, Vec::new());

    running.stop.store(true, Ordering::SeqCst);

    running.thread.join().unwrap();
}
