#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! `refresh`: discovery, the Claude registry and reconcile in one pass, with no
//! hook involved.

use claude_monitor::model::{Author, Session, State};
use claude_monitor::refresh::refresh;
use claude_monitor::store::events::EventRow;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::json;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const PROC_START: u64 = 5000;
const CWD: &str = "/home/ali/git/proj";

struct Fx {
    _dir: TempDir,
    fake: FakeEnv,
    store: Store,
}

impl Fx {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
        fake.vars.set("COPILOT_HOME", "/cop");
        fake.fs.add_dir(CWD);
        let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
        Self {
            _dir: dir,
            fake,
            store,
        }
    }

    fn run(&self) -> claude_monitor::refresh::RefreshReport {
        refresh(&self.store, &self.fake.env, NOW)
    }

    fn transcript(&self, id: &str) {
        let line = json!({
            "type": "user", "uuid": "u1", "parentUuid": null, "sessionId": id,
            "timestamp": "2026-10-12T09:00:00.000Z", "cwd": CWD, "gitBranch": "main",
            "version": "2.1.293", "isSidechain": false,
            "message": {"role": "user", "content": "hello"},
        });
        self.fake.fs.add_file(
            format!("/cfg/projects/-home-ali-git-proj/{id}.jsonl"),
            &format!("{line}\n"),
        );
    }

    fn registry(&self, pid: u32, id: &str, status: &str, live: bool) {
        let value = json!({
            "pid": pid, "sessionId": id, "cwd": CWD, "startedAt": 1_791_800_000_000_u64,
            "procStart": PROC_START.to_string(), "version": "2.1.293", "kind": "interactive",
            "entrypoint": "cli", "tmux": "main:@3.%5", "status": status,
        });
        self.fake
            .fs
            .add_file(format!("/cfg/sessions/{pid}.json"), &value.to_string());
        if live {
            self.fake.processes.spawn(pid, PROC_START);
        }
    }

    fn session(&self, id: &str) -> Option<Session> {
        self.store.get_session_by_agent_id("claude", id).unwrap()
    }

    fn events(&self) -> Vec<EventRow> {
        self.store.events_since(0, 1000).unwrap()
    }
}

fn registry_author() -> Author {
    Author::System {
        component: "registry".to_owned(),
    }
}

#[test]
fn a_live_session_nothing_else_knows_appears() {
    let fx = Fx::new();
    fx.registry(4242, "sess-new", "busy", true);

    let report = fx.run();

    let session = fx.session("sess-new").expect("session created");
    assert_eq!(report.registry_created, 1);
    assert_eq!(session.cwd, CWD);
    assert_eq!(session.state, State::Working);
    assert_eq!(session.pid, Some(4242));
    assert_eq!(session.pid_start, Some(PROC_START));
    assert_eq!(session.tmux_pane.as_deref(), Some("%5"));
    let events = fx.events();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e.author == registry_author()));
}

#[test]
fn busy_becomes_working_for_a_discovered_session() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "busy", true);

    fx.run();

    let session = fx.session("sess-1").unwrap();
    assert_eq!(session.state, State::Working);
    assert_eq!(session.pid, Some(10));
    assert_eq!(session.branch.as_deref(), Some("main"));
    assert_eq!(session.state_since, NOW);
}

#[test]
fn waiting_becomes_needs_permission() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "waiting", true);

    fx.run();

    assert_eq!(fx.session("sess-1").unwrap().state, State::NeedsPermission);
}

#[test]
fn a_status_change_between_passes_updates_the_state() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "busy", true);
    fx.run();
    fx.registry(10, "sess-1", "idle", true);

    fx.run();

    assert_eq!(fx.session("sess-1").unwrap().state, State::Idle);
}

#[test]
fn a_stale_registry_entry_is_ignored() {
    let fx = Fx::new();
    fx.transcript("sess-known");
    fx.registry(10, "sess-known", "busy", false);
    fx.registry(11, "sess-ghost", "busy", false);

    let report = fx.run();

    assert!(fx.session("sess-ghost").is_none());
    let known = fx.session("sess-known").unwrap();
    assert_eq!(known.state, State::Unknown);
    assert_eq!(known.pid, None);
    assert_eq!(report.registry_created, 0);
    assert_eq!(report.registry_updated, 0);
}

#[test]
fn a_finished_session_is_never_touched() {
    let fx = Fx::new();
    let mut finished = Session::new("id-1", "claude", "sess-1", CWD, "2026-10-12T08:00:00Z");
    finished.state = State::Finished;
    fx.store.upsert_session(&finished).unwrap();
    fx.registry(10, "sess-1", "busy", true);
    let before = fx.events().len();

    fx.run();

    assert_eq!(fx.session("sess-1").unwrap(), finished);
    assert!(fx.events()[before..]
        .iter()
        .all(|e| e.author != registry_author()));
}

#[test]
fn a_session_without_a_pid_is_not_judged_by_processes() {
    let fx = Fx::new();
    let mut idle = Session::new("id-1", "claude", "sess-1", CWD, "2026-10-12T08:00:00Z");
    idle.state = State::Idle;
    fx.store.upsert_session(&idle).unwrap();

    fx.run();

    assert_eq!(fx.session("sess-1").unwrap().state, State::Idle);
}

#[test]
fn a_dead_pid_recorded_by_the_registry_is_then_interrupted() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "busy", true);
    fx.run();
    fx.fake.processes.kill(10);

    fx.run();

    assert_eq!(fx.session("sess-1").unwrap().state, State::Interrupted);
}

#[test]
fn copilot_sessions_are_discovered_too() {
    let fx = Fx::new();
    fx.fake.fs.add_file(
        "/cop/session-state/cs-1/workspace.yaml",
        "id: cs-1\ncwd: /w/a\ncreated_at: 2026-10-12T09:00:00Z\n",
    );

    let report = fx.run();

    assert_eq!(report.discovered.created, 1);
    assert!(fx
        .store
        .get_session_by_agent_id("copilot", "cs-1")
        .unwrap()
        .is_some());
}

#[test]
fn a_second_run_writes_no_events() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "busy", true);
    fx.registry(11, "sess-new", "waiting", true);
    fx.fake.fs.add_file(
        "/cop/session-state/cs-1/workspace.yaml",
        "id: cs-1\ncwd: /w/a\ncreated_at: 2026-10-12T09:00:00Z\n",
    );
    fx.run();
    let after_first = fx.store.last_seq().unwrap();
    assert!(after_first > 0);

    let report = fx.run();

    assert_eq!(fx.store.last_seq().unwrap(), after_first);
    assert!(!report.changed(), "{report:?}");
}

#[test]
fn missing_agent_directories_degrade_to_an_empty_pass() {
    let fx = Fx::new();

    let report = fx.run();

    assert!(!report.changed());
    assert!(fx.store.list_sessions().unwrap().is_empty());
}
