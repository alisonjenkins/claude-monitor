#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! The live refresh loop: wake sources in, refreshed store out.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use claude_monitor::env::Env;
use claude_monitor::feed::{EventSource, FakeHandle, FakeSource};
use claude_monitor::live::{build_sources, run_live, tracked_processes};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::{FakeEnv, FakeVars};
use serde_json::json;
use tempfile::TempDir;

const PATIENCE: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(400);

struct Loop {
    stop: Arc<AtomicBool>,
    refreshes: Arc<AtomicUsize>,
    wake: FakeHandle,
    done: mpsc::Receiver<()>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Loop {
    fn start(dir: &TempDir, env: Env) -> Self {
        let (source, wake) = FakeSource::new();
        let stop = Arc::new(AtomicBool::new(false));
        let refreshes = Arc::new(AtomicUsize::new(0));
        let (done_tx, done) = mpsc::channel();
        let path = dir.path().to_owned();
        let (flag, count) = (Arc::clone(&stop), Arc::clone(&refreshes));
        let thread = thread::spawn(move || {
            let store = Store::open_in_dir(&path, &StoreOptions::default()).unwrap();
            let sources: Vec<Box<dyn EventSource + Send>> = vec![Box::new(source)];
            run_live(&store, &env, sources, None, &flag, &mut |_| {
                count.fetch_add(1, Ordering::SeqCst);
            });
            done_tx.send(()).unwrap();
        });
        Self {
            stop,
            refreshes,
            wake,
            done,
            thread: Some(thread),
        }
    }

    fn refreshed(&self) -> usize {
        self.refreshes.load(Ordering::SeqCst)
    }

    fn wait_for_refresh(&self) -> bool {
        let start = Instant::now();
        while start.elapsed() < PATIENCE {
            if self.refreshed() > 0 {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn stop(mut self) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        let stopped = self.done.recv_timeout(PATIENCE).is_ok();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        stopped
    }
}

fn disk_only_env() -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    fake.vars.set("COPILOT_HOME", "/cop");
    let line = json!({
        "type": "user", "uuid": "u1", "parentUuid": null, "sessionId": "sess-1",
        "timestamp": "2026-10-12T09:00:00.000Z", "cwd": "/w/proj", "version": "2.1.293",
        "isSidechain": false, "message": {"role": "user", "content": "hello"},
    });
    fake.fs
        .add_file("/cfg/projects/-w-proj/sess-1.jsonl", &format!("{line}\n"));
    fake
}

#[test]
fn a_wake_refreshes_the_store() {
    let dir = TempDir::new().unwrap();
    let fake = disk_only_env();
    let live = Loop::start(&dir, fake.env.clone());

    live.wake.wake();

    assert!(live.wait_for_refresh(), "no refresh after a wake");
    assert!(live.stop());
    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    assert!(store
        .get_session_by_agent_id("claude", "sess-1")
        .unwrap()
        .is_some());
}

#[test]
fn no_wake_means_no_refresh() {
    let dir = TempDir::new().unwrap();
    let fake = disk_only_env();
    let live = Loop::start(&dir, fake.env.clone());

    thread::sleep(QUIET);

    assert_eq!(live.refreshed(), 0);
    assert!(live.stop());
}

#[test]
fn setting_stop_ends_the_loop_promptly() {
    let dir = TempDir::new().unwrap();
    let fake = disk_only_env();
    let live = Loop::start(&dir, fake.env.clone());

    assert!(live.stop(), "the loop did not stop");
}

#[test]
fn missing_directories_are_skipped_with_a_warning_each() {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_CONFIG_DIR", "/nowhere/cfg");
    fake.vars.set("COPILOT_HOME", "/nowhere/cop");

    let built = build_sources(&fake.env);

    assert_eq!(built.warnings.len(), 3, "{:?}", built.warnings);
    for word in ["registry", "projects", "copilot"] {
        assert!(
            built.warnings.iter().any(|w| w.contains(word)),
            "{word}: {:?}",
            built.warnings
        );
    }
    assert!(!built.sources.is_empty(), "the safety net must remain");
}

#[test]
fn existing_directories_are_watched_without_warnings() {
    let dir = TempDir::new().unwrap();
    for sub in ["cfg/sessions", "cfg/projects", "cop/session-state"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    let vars = Arc::new(FakeVars::default());
    vars.set(
        "CLAUDE_CONFIG_DIR",
        dir.path().join("cfg").to_str().unwrap(),
    );
    vars.set("COPILOT_HOME", dir.path().join("cop").to_str().unwrap());
    let mut env = Env::real();
    env.vars = vars;

    let built = build_sources(&env);

    assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    assert!(built.sources.len() >= 4);
}

fn session(id: &str, state: State, pid: Option<u32>, start: Option<u64>) -> Session {
    let mut s = Session::new(id, "claude", id, "/w", "2026-10-12T09:00:00Z");
    s.state = state;
    s.pid = pid;
    s.pid_start = start;
    s
}

#[test]
fn only_active_sessions_with_a_known_process_are_tracked() {
    let sessions = [
        session("a", State::Working, Some(10), Some(100)),
        session("b", State::Idle, Some(11), Some(101)),
        session("c", State::Finished, Some(12), Some(102)),
        session("d", State::Interrupted, Some(13), Some(103)),
        session("e", State::Working, Some(14), None),
        session("f", State::NeedsPermission, None, None),
    ];

    let tracked = tracked_processes(&sessions);

    assert_eq!(
        tracked.into_iter().collect::<Vec<_>>(),
        [(10, 100), (11, 101)]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_corrupt_row_does_not_stop_exit_watches_being_registered() {
    use claude_monitor::env::parse_proc_stat_start_time;
    use claude_monitor::feed::ProcExitSource;
    use std::process::{Command, Stdio};

    let dir = TempDir::new().unwrap();
    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    let mut child = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let start = parse_proc_stat_start_time(&stat).unwrap();
    store
        .upsert_session(&session("good", State::Working, Some(pid), Some(start)))
        .unwrap();
    store
        .upsert_session(&session("bad", State::Working, None, None))
        .unwrap();
    let conn = store.connection();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute("UPDATE sessions SET state = 'bogus' WHERE id = 'bad'", [])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
        .unwrap();
    let source = ProcExitSource::new().unwrap();
    let handle = source.handle();
    let (tx, rx) = mpsc::channel();

    let live = spawn_live(
        store,
        FakeEnv::new().env,
        vec![Box::new(source)],
        Some(handle),
        move |_| {
            let _ = tx.send(());
        },
    )
    .unwrap();
    child.kill().unwrap();

    let woke = rx.recv_timeout(PATIENCE).is_ok();
    drop(live);
    child.wait().unwrap();
    assert!(
        woke,
        "the exit of the good session's process never woke the loop"
    );
}
