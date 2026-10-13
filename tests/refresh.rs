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
use claude_monitor::tmux::PaneLocation;
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
        self.registry_at(pid, id, status, live, CWD, "main:@3.%5");
    }

    fn registry_at(&self, pid: u32, id: &str, status: &str, live: bool, cwd: &str, tmux: &str) {
        let value = json!({
            "pid": pid, "sessionId": id, "cwd": cwd, "startedAt": 1_791_800_000_000_u64,
            "procStart": PROC_START.to_string(), "version": "2.1.293", "kind": "interactive",
            "entrypoint": "cli", "tmux": tmux, "status": status,
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

impl Fx {
    fn registry_named(&self, pid: u32, id: &str, name: Option<&str>, source: &str) {
        let mut value = json!({
            "pid": pid, "sessionId": id, "cwd": CWD, "startedAt": 1_791_800_000_000_u64,
            "procStart": PROC_START.to_string(), "version": "2.1.293", "kind": "interactive",
            "entrypoint": "cli", "tmux": "main:@3.%5", "status": "busy",
        });
        if let Some(name) = name {
            value["name"] = json!(name);
            value["nameSource"] = json!(source);
        }
        self.fake
            .fs
            .add_file(format!("/cfg/sessions/{pid}.json"), &value.to_string());
        self.fake.processes.spawn(pid, PROC_START);
    }
}

fn name_of(fx: &Fx, id: &str) -> Option<String> {
    let session = fx.session(id);
    assert!(session.is_some(), "no session {id}");
    session.and_then(|s| s.session_name)
}

#[test]
fn a_new_session_from_the_registry_takes_the_user_name() {
    let fx = Fx::new();
    fx.registry_named(10, "sess-1", Some("aquero"), "user");

    fx.run();

    assert_eq!(name_of(&fx, "sess-1").as_deref(), Some("aquero"));
}

#[test]
fn a_discovered_session_gains_the_user_name() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry_named(10, "sess-1", Some("aquero"), "user");

    fx.run();

    assert_eq!(name_of(&fx, "sess-1").as_deref(), Some("aquero"));
}

#[test]
fn an_automatic_name_is_not_recorded() {
    let fx = Fx::new();
    fx.registry_named(10, "sess-1", Some("proj"), "auto");

    fx.run();

    assert_eq!(name_of(&fx, "sess-1"), None);
}

#[test]
fn a_changed_name_is_updated_and_a_repeat_pass_writes_nothing() {
    let fx = Fx::new();
    fx.registry_named(10, "sess-1", Some("first"), "user");
    fx.run();
    fx.registry_named(10, "sess-1", Some("second"), "user");

    let renamed = fx.run();
    let seq = fx.store.last_seq().unwrap();
    let repeat = fx.run();

    assert_eq!(name_of(&fx, "sess-1").as_deref(), Some("second"));
    assert_eq!(renamed.registry_updated, 1);
    assert!(!repeat.changed(), "{repeat:?}");
    assert_eq!(fx.store.last_seq().unwrap(), seq);
}

#[test]
fn a_stored_name_is_never_cleared_by_an_entry_without_one() {
    let fx = Fx::new();
    fx.registry_named(10, "sess-1", Some("aquero"), "user");
    fx.run();
    fx.registry_named(10, "sess-1", None, "");
    fx.run();
    fx.registry_named(10, "sess-1", Some("proj"), "auto");

    fx.run();

    assert_eq!(name_of(&fx, "sess-1").as_deref(), Some("aquero"));
}

fn location(session: &str, window: &str, index: &str) -> PaneLocation {
    PaneLocation {
        session_name: session.to_owned(),
        window_name: window.to_owned(),
        window_index: index.to_owned(),
        pane_index: "0".to_owned(),
        window_panes: "1".to_owned(),
        window_layout: String::new(),
    }
}

fn placement(session: &Session) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        session.tmux_session.as_deref(),
        session.tmux_window_name.as_deref(),
        session.tmux_window_index.as_deref(),
    )
}

fn split_location(index: &str, panes: &str, layout: &str) -> PaneLocation {
    PaneLocation {
        pane_index: index.to_owned(),
        window_panes: panes.to_owned(),
        window_layout: layout.to_owned(),
        ..location("work", "api", "2")
    }
}

fn pane_fields(session: &Session) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        session.tmux_pane_index.as_deref(),
        session.tmux_window_panes.as_deref(),
        session.tmux_window_layout.as_deref(),
    )
}

const LAYOUT: &str = "b25d,200x50,0,0{100x50,0,0,0,99x50,101,0,1}";

#[test]
fn a_live_pane_records_its_pane_index_pane_count_and_layout() {
    let fx = Fx::new();
    fx.registry(10, "sess-1", "busy", true);
    fx.fake
        .tmux
        .add_pane("%5", split_location("1", "2", LAYOUT));

    fx.run();

    assert_eq!(
        pane_fields(&fx.session("sess-1").unwrap()),
        (Some("1"), Some("2"), Some(LAYOUT))
    );
}

#[test]
fn a_changed_pane_count_is_recorded_once_and_never_cleared() {
    let fx = Fx::new();
    fx.registry(10, "sess-1", "busy", true);
    fx.fake
        .tmux
        .add_pane("%5", split_location("1", "2", LAYOUT));
    fx.run();
    fx.fake
        .tmux
        .add_pane("%5", split_location("1", "3", "other,layout"));

    let grown = fx.run();
    let repeat = fx.run();
    fx.fake.tmux.remove_pane("%5");
    fx.run();
    fx.fake.tmux.set_available(false);
    fx.run();

    assert_eq!(
        pane_fields(&fx.session("sess-1").unwrap()),
        (Some("1"), Some("3"), Some("other,layout"))
    );
    assert!(grown.changed(), "{grown:?}");
    assert!(!repeat.changed(), "{repeat:?}");
}

#[test]
fn a_live_pane_records_its_tmux_session_and_window() {
    let fx = Fx::new();
    fx.registry(10, "sess-1", "busy", true);
    fx.fake
        .tmux
        .add_pane("%5", location("my project", "api build", "2"));

    fx.run();

    let session = fx.session("sess-1").unwrap();
    assert_eq!(
        placement(&session),
        (Some("my project"), Some("api build"), Some("2"))
    );
}

#[test]
fn a_moved_window_updates_the_placement_and_a_repeat_writes_nothing() {
    let fx = Fx::new();
    fx.registry(10, "sess-1", "busy", true);
    fx.fake.tmux.add_pane("%5", location("work", "api", "2"));
    fx.run();
    fx.fake.tmux.add_pane("%5", location("work", "api", "4"));

    let moved = fx.run();
    let repeat = fx.run();

    assert_eq!(
        placement(&fx.session("sess-1").unwrap()),
        (Some("work"), Some("api"), Some("4"))
    );
    assert!(moved.changed(), "{moved:?}");
    assert!(!repeat.changed(), "{repeat:?}");
}

#[test]
fn a_vanished_pane_or_missing_tmux_never_clears_the_placement() {
    let fx = Fx::new();
    fx.registry(10, "sess-1", "busy", true);
    fx.fake.tmux.add_pane("%5", location("work", "api", "2"));
    fx.run();

    fx.fake.tmux.remove_pane("%5");
    fx.run();
    fx.fake.tmux.set_available(false);
    fx.run();

    assert_eq!(
        placement(&fx.session("sess-1").unwrap()),
        (Some("work"), Some("api"), Some("2"))
    );
}

fn stored(fx: &Fx, state: State) -> Session {
    let mut s = Session::new("id-1", "claude", "sess-1", CWD, "2026-10-12T08:00:00Z");
    s.state = state;
    fx.store.upsert_session(&s).unwrap();
    s
}

#[test]
fn a_live_registry_entry_revives_an_interrupted_session() {
    let fx = Fx::new();
    let mut old = stored(&fx, State::Interrupted);
    old.pid = Some(9);
    old.pid_start = Some(1);
    old.end_reason = Some("error".to_owned());
    fx.store.upsert_session(&old).unwrap();
    fx.registry(10, "sess-1", "idle", true);

    let report = fx.run();

    let session = fx.session("sess-1").unwrap();
    assert_eq!(session.state, State::Idle);
    assert_eq!(session.pid, Some(10));
    assert_eq!(session.pid_start, Some(PROC_START));
    assert_eq!(session.end_reason, None);
    assert_eq!(report.registry_updated, 1);
    let revived = fx
        .events()
        .into_iter()
        .find(|e| e.kind == "session.state_changed")
        .expect("state_changed event");
    assert_eq!(revived.payload["from"], "interrupted");
    assert_eq!(revived.payload["state"], "idle");
    assert_eq!(revived.payload["reason"], "registry");
    assert_eq!(revived.author, registry_author());
}

#[test]
fn a_live_registry_entry_moves_an_unknown_session_to_its_status() {
    let fx = Fx::new();
    stored(&fx, State::Unknown);
    fx.registry(10, "sess-1", "busy", true);

    fx.run();

    assert_eq!(fx.session("sess-1").unwrap().state, State::Working);
}

#[test]
fn a_dead_registry_entry_does_not_revive_an_interrupted_session() {
    let fx = Fx::new();
    let old = stored(&fx, State::Interrupted);
    fx.registry(10, "sess-1", "busy", false);

    let report = fx.run();

    assert_eq!(fx.session("sess-1").unwrap(), old);
    assert_eq!(report.registry_updated, 0);
}

#[test]
fn a_revived_session_stays_alive_on_the_next_pass() {
    let fx = Fx::new();
    stored(&fx, State::Interrupted);
    fx.registry(10, "sess-1", "idle", true);
    fx.run();
    let after_first = fx.store.last_seq().unwrap();

    let report = fx.run();

    assert_eq!(fx.session("sess-1").unwrap().state, State::Idle);
    assert_eq!(fx.store.last_seq().unwrap(), after_first);
    assert!(!report.changed(), "{report:?}");
}

const OLD: &str = "2026-10-12T08:00:00Z";

fn ghost(fx: &Fx, kind: &str, id: &str, since: &str) -> Session {
    let mut s = Session::new(&format!("ghost-{id}"), kind, id, CWD, since);
    s.state = State::Idle;
    s.tmux_pane = Some("%9".to_owned());
    fx.store.upsert_session(&s).unwrap();
    s
}

fn ghost_fx() -> Fx {
    let fx = Fx::new();
    fx.registry(10, "sess-live", "idle", true);
    fx.fake.tmux.add_pane("%9", location("work", "api", "2"));
    fx
}

#[test]
fn a_session_with_no_registry_entry_and_no_process_is_interrupted() {
    let fx = ghost_fx();
    ghost(&fx, "claude", "sess-ghost", OLD);

    fx.run();

    assert_eq!(fx.session("sess-ghost").unwrap().state, State::Interrupted);
    let event = fx
        .events()
        .into_iter()
        .find(|e| e.payload["reason"] == "registry_missing")
        .expect("registry_missing event");
    assert_eq!(event.payload["from"], "idle");
}

#[test]
fn a_session_with_a_live_registry_entry_is_not_a_ghost() {
    let fx = ghost_fx();
    ghost(&fx, "claude", "sess-live", OLD);

    fx.run();

    assert_eq!(fx.session("sess-live").unwrap().state, State::Idle);
}

#[test]
fn a_session_whose_recorded_process_is_alive_is_not_a_ghost() {
    let fx = ghost_fx();
    let mut s = ghost(&fx, "claude", "sess-ghost", OLD);
    s.pid = Some(77);
    s.pid_start = Some(PROC_START);
    fx.store.upsert_session(&s).unwrap();
    fx.fake.processes.spawn(77, PROC_START);

    fx.run();

    assert_eq!(fx.session("sess-ghost").unwrap().state, State::Idle);
}

#[test]
fn a_copilot_session_is_never_a_ghost() {
    let fx = ghost_fx();
    ghost(&fx, "copilot", "cs-1", OLD);

    fx.run();

    let kept = fx
        .store
        .get_session_by_agent_id("copilot", "cs-1")
        .unwrap()
        .unwrap();
    assert_eq!(kept.state, State::Idle);
}

#[test]
fn a_session_younger_than_two_minutes_is_not_a_ghost() {
    let fx = ghost_fx();
    ghost(&fx, "claude", "sess-ghost", "2026-10-12T11:58:30Z");

    fx.run();

    assert_eq!(fx.session("sess-ghost").unwrap().state, State::Idle);
}

#[test]
fn an_empty_registry_marks_nothing_as_a_ghost() {
    let fx = Fx::new();
    fx.fake.tmux.add_pane("%9", location("work", "api", "2"));
    ghost(&fx, "claude", "sess-ghost", OLD);

    fx.run();

    assert_eq!(fx.session("sess-ghost").unwrap().state, State::Idle);
}

#[test]
fn a_ghost_that_was_interrupted_stays_quiet_on_the_next_pass() {
    let fx = ghost_fx();
    ghost(&fx, "claude", "sess-ghost", OLD);
    fx.run();
    let seq = fx.store.last_seq().unwrap();

    let report = fx.run();

    assert_eq!(fx.store.last_seq().unwrap(), seq);
    assert!(!report.changed(), "{report:?}");
}

const MOVED: &str = "/home/ali/git/proj/.claude/worktrees/moved";

#[test]
fn a_changed_registry_cwd_replaces_the_stored_one() {
    let fx = Fx::new();
    fx.fake.fs.add_dir(MOVED);
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "idle", true);
    fx.run();
    fx.registry_at(10, "sess-1", "idle", true, MOVED, "main:@3.%5");

    let report = fx.run();

    let session = fx.session("sess-1").unwrap();
    assert_eq!(session.cwd, MOVED);
    assert!(session.worktree_exists);
    assert_eq!(report.registry_updated, 1);
}

#[test]
fn a_registry_cwd_that_is_gone_clears_worktree_exists() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "idle", true);
    fx.run();
    fx.registry_at(10, "sess-1", "idle", true, MOVED, "main:@3.%5");

    fx.run();

    let session = fx.session("sess-1").unwrap();
    assert_eq!(session.cwd, MOVED);
    assert!(!session.worktree_exists);
}

#[test]
fn the_registry_records_the_tmux_session_and_pane() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry_at(10, "sess-1", "idle", true, CWD, "my work:@3.%7");

    fx.run();

    let session = fx.session("sess-1").unwrap();
    assert_eq!(session.tmux_pane.as_deref(), Some("%7"));
    assert_eq!(session.tmux_session.as_deref(), Some("my work"));
}

#[test]
fn a_moved_tmux_session_in_the_registry_is_recorded_and_a_repeat_writes_nothing() {
    let fx = Fx::new();
    fx.transcript("sess-1");
    fx.registry(10, "sess-1", "idle", true);
    fx.run();
    fx.registry_at(10, "sess-1", "idle", true, CWD, "other:@4.%6");

    let moved = fx.run();
    let seq = fx.store.last_seq().unwrap();
    let repeat = fx.run();

    let session = fx.session("sess-1").unwrap();
    assert_eq!(session.tmux_pane.as_deref(), Some("%6"));
    assert_eq!(session.tmux_session.as_deref(), Some("other"));
    assert!(moved.changed(), "{moved:?}");
    assert_eq!(fx.store.last_seq().unwrap(), seq);
    assert!(!repeat.changed(), "{repeat:?}");
}
