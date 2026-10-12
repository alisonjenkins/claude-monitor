#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! The hook records `$PPID` as the agent pid, but a wrapper shell between the
//! agent and the hook makes `$PPID` the wrong process. Claude's live session
//! registry names the real one, so it wins when it has a live entry for the
//! session.

use std::path::PathBuf;

use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::Session;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::{json, Value};
use tempfile::TempDir;

const SESSION: &str = "s1";
const PPID: u32 = 1234;
const PPID_START: u64 = 55;
const REGISTRY_PID: u32 = 9000;
const REGISTRY_START: u64 = 777;

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    fake: FakeEnv,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
        fake.fs.add_dir("/tmp/SCRATCH/r2/work");
        fake.processes.spawn(PPID, PPID_START);
        Self {
            state_dir: dir.path().join("state"),
            _dir: dir,
            fake,
        }
    }

    fn registry_file(&self, pid: u32, session_id: &str, proc_start: u64) {
        let value = json!({
            "pid": pid, "sessionId": session_id, "cwd": "/tmp/SCRATCH/r2/work",
            "startedAt": 1_791_800_000_000_u64, "procStart": proc_start.to_string(),
            "version": "2.1.293", "kind": "interactive", "entrypoint": "cli",
            "status": "busy",
        });
        self.fake
            .fs
            .add_file(format!("/cfg/sessions/{pid}.json"), &value.to_string());
    }

    fn feed(&self) {
        let ctx = HookContext {
            agent: "claude",
            state_dir: &self.state_dir,
            tmux_pane: None,
            clock: self.fake.clock.as_ref(),
            git: &NoGit,
            agent_pid: Some(PPID),
            env: Some(&self.fake.env),
        };
        ingest(&ctx, &payload());
    }

    fn session(&self) -> Session {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer())
            .unwrap()
            .get_session_by_agent_id("claude", SESSION)
            .unwrap()
            .expect("session row exists")
    }

    fn pid_and_start(&self) -> (Option<u32>, Option<u64>) {
        let s = self.session();
        (s.pid, s.pid_start)
    }
}

fn payload() -> String {
    let path = format!(
        "{}/tests/fixtures/claude/SessionStart-startup.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    value["session_id"] = Value::String(SESSION.to_owned());
    value.to_string()
}

#[test]
fn a_live_registry_entry_with_another_pid_wins_over_ppid() {
    let h = Harness::new();
    h.fake.processes.spawn(REGISTRY_PID, REGISTRY_START);
    h.registry_file(REGISTRY_PID, SESSION, REGISTRY_START);

    h.feed();

    assert_eq!(
        h.pid_and_start(),
        (Some(REGISTRY_PID), Some(REGISTRY_START))
    );
}

#[test]
fn no_registry_keeps_ppid() {
    let h = Harness::new();

    h.feed();

    assert_eq!(h.pid_and_start(), (Some(PPID), Some(PPID_START)));
}

#[test]
fn an_unreadable_registry_entry_keeps_ppid() {
    let h = Harness::new();
    h.fake.fs.add_file("/cfg/sessions/9000.json", "{not json");

    h.feed();

    assert_eq!(h.pid_and_start(), (Some(PPID), Some(PPID_START)));
}

#[test]
fn a_registry_entry_whose_process_is_gone_keeps_ppid() {
    let h = Harness::new();
    h.registry_file(REGISTRY_PID, SESSION, REGISTRY_START);

    h.feed();

    assert_eq!(h.pid_and_start(), (Some(PPID), Some(PPID_START)));
}

#[test]
fn a_live_entry_for_another_session_keeps_ppid() {
    let h = Harness::new();
    h.fake.processes.spawn(REGISTRY_PID, REGISTRY_START);
    h.registry_file(REGISTRY_PID, "other-session", REGISTRY_START);

    h.feed();

    assert_eq!(h.pid_and_start(), (Some(PPID), Some(PPID_START)));
}

#[test]
fn a_live_entry_with_the_same_pid_changes_nothing() {
    let h = Harness::new();
    h.registry_file(PPID, SESSION, PPID_START);

    h.feed();

    assert_eq!(h.pid_and_start(), (Some(PPID), Some(PPID_START)));
}
