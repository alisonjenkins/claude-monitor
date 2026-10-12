//! Import of the old dashboard's per-session runtime JSON files into the store
//! (FR-022, `contracts/cli.md` `migrate`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::fs;
use std::path::PathBuf;

use claude_monitor::cli::migrate;
use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::{Author, Session, State};
use claude_monitor::store::events::EventRow;
use claude_monitor::store::legacy::{
    self, MARKER_ENTITY_ID, MARKER_ENTITY_KIND, MARKER_EVENT_KIND,
};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::{FakeClock, FakeEnv};
use serde_json::{json, Value};
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const SINCE_WORKING: &str = "2026-10-12T10:00:00Z";
const SINCE_PERMISSION: &str = "2026-10-12T10:05:00Z";
const SINCE_IDLE: &str = "2026-10-12T10:10:00Z";

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    runtime_dir: PathBuf,
    clock: FakeClock,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let runtime_dir = dir.path().join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        Self {
            state_dir: dir.path().join("state"),
            runtime_dir,
            _dir: dir,
            clock: FakeClock::new(NOW),
        }
    }

    fn write_legacy(&self, id: &str, state: &str, since: &str, pane: Option<&str>) {
        let body = json!({
            "session_id": id,
            "state": state,
            "cwd": format!("/work/{id}"),
            "tmux_pane": pane,
            "since": since,
        });
        self.write_raw(&format!("{id}.json"), &body.to_string());
    }

    fn write_raw(&self, file_name: &str, contents: &str) {
        fs::write(self.runtime_dir.join(file_name), contents).unwrap();
    }

    fn write_three_states(&self) {
        self.write_legacy("sess-w", "working", SINCE_WORKING, Some("%1"));
        self.write_legacy("sess-p", "needs_permission", SINCE_PERMISSION, None);
        self.write_legacy("sess-i", "idle", SINCE_IDLE, Some("%3"));
    }

    fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer()).unwrap()
    }

    fn legacy_file_names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.runtime_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn session(store: &Store, agent_session_id: &str) -> Session {
    store
        .get_session_by_agent_id("claude", agent_session_id)
        .unwrap()
        .expect("session row exists")
}

fn markers(store: &Store) -> Vec<EventRow> {
    store
        .events_since(0, 1000)
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == MARKER_EVENT_KIND)
        .collect()
}

#[test]
fn imports_one_session_per_file_with_mapped_state_and_times() {
    let h = Harness::new();
    h.write_three_states();
    let store = h.store();

    let report = legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    assert_eq!(report.imported, 3);
    assert_eq!(report.skipped_existing, 0);
    assert_eq!(report.skipped_corrupt, 0);
    assert_eq!(store.list_sessions().unwrap().len(), 3);

    for (id, state, since, pane) in [
        ("sess-w", State::Working, SINCE_WORKING, Some("%1")),
        ("sess-p", State::NeedsPermission, SINCE_PERMISSION, None),
        ("sess-i", State::Idle, SINCE_IDLE, Some("%3")),
    ] {
        let s = session(&store, id);
        assert_eq!(s.state, state, "{id}");
        assert_eq!(s.state_since, since, "{id}");
        assert_eq!(s.last_activity_at, since, "{id}");
        assert_eq!(s.started_at, since, "{id}");
        assert_eq!(s.cwd, format!("/work/{id}"), "{id}");
        assert_eq!(s.tmux_pane.as_deref(), pane, "{id}");
        assert_eq!(s.pid, None, "{id}: liveness is unknown");
    }
}

#[test]
fn imported_sessions_are_flagged_in_their_created_event() {
    let h = Harness::new();
    h.write_legacy("sess-w", "working", SINCE_WORKING, None);
    let store = h.store();

    legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    let created: Vec<EventRow> = store
        .events_since(0, 1000)
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == "session.created")
        .collect();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].payload["imported"], Value::Bool(true));
    assert_eq!(created[0].entity_id, session(&store, "sess-w").id);
    assert_eq!(
        created[0].author,
        Author::System {
            component: "migrate".to_owned()
        }
    );
}

#[test]
fn second_run_imports_nothing_and_appends_no_second_marker() {
    let h = Harness::new();
    h.write_three_states();
    let store = h.store();

    legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();
    let ids_before: Vec<String> = store
        .list_sessions()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    let seq_before = store.last_seq().unwrap();

    let second = legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    assert_eq!(second.imported, 0);
    assert_eq!(second.skipped_existing, 3);
    assert_eq!(markers(&store).len(), 1);
    assert_eq!(store.last_seq().unwrap(), seq_before);
    let ids_after: Vec<String> = store
        .list_sessions()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids_after, ids_before);
}

#[test]
fn marker_event_records_the_import() {
    let h = Harness::new();
    h.write_three_states();
    let store = h.store();

    legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    let rows = markers(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entity_kind, MARKER_ENTITY_KIND);
    assert_eq!(rows[0].entity_id, MARKER_ENTITY_ID);
    assert_eq!(rows[0].at, NOW);
    assert_eq!(
        rows[0].author,
        Author::System {
            component: "migrate".to_owned()
        }
    );
    assert!(legacy::is_migrated(&store).unwrap());
}

#[test]
fn migrate_if_needed_runs_once_then_does_nothing() {
    let h = Harness::new();
    h.write_three_states();
    let store = h.store();
    assert!(!legacy::is_migrated(&store).unwrap());

    let first = legacy::migrate_if_needed(&store, &h.runtime_dir, &h.clock).unwrap();
    assert_eq!(first.map(|r| r.imported), Some(3));

    h.write_legacy("sess-late", "idle", SINCE_IDLE, None);
    let second = legacy::migrate_if_needed(&store, &h.runtime_dir, &h.clock).unwrap();
    assert_eq!(second, None);
    assert!(store
        .get_session_by_agent_id("claude", "sess-late")
        .unwrap()
        .is_none());
    assert_eq!(markers(&store).len(), 1);
}

#[test]
fn an_existing_session_with_the_same_agent_id_is_not_overwritten() {
    let h = Harness::new();
    h.write_legacy("sess-w", "working", SINCE_WORKING, Some("%1"));
    let store = h.store();
    let mut existing = Session::new(
        "store-id-1",
        "claude",
        "sess-w",
        "/elsewhere",
        "2026-10-12T09:00:00Z",
    );
    existing.state = State::Finished;
    existing.task_description = Some("keep me".to_owned());
    store.upsert_session(&existing).unwrap();

    let report = legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    assert_eq!(report.imported, 0);
    assert_eq!(report.skipped_existing, 1);
    assert_eq!(store.list_sessions().unwrap(), vec![existing]);
}

#[test]
fn corrupt_and_unsafe_files_are_skipped_counted_and_named() {
    let h = Harness::new();
    h.write_legacy("sess-ok", "idle", SINCE_IDLE, None);
    h.write_raw("broken.json", "{ not json");
    h.write_raw("..evil.json", &legacy_body("..evil"));
    h.write_raw("has space.json", &legacy_body("has space"));
    h.write_raw("mismatch.json", &legacy_body("../../etc/passwd"));
    h.write_raw(
        "unknown-state.json",
        &legacy_body_with_state("unknown-state", "bogus"),
    );
    let store = h.store();

    let report = legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    assert_eq!(report.imported, 1);
    assert_eq!(report.skipped_corrupt, 5);
    let mut names = report.corrupt_files.clone();
    names.sort();
    assert_eq!(
        names,
        [
            "..evil.json",
            "broken.json",
            "has space.json",
            "mismatch.json",
            "unknown-state.json"
        ]
    );
    assert_eq!(store.list_sessions().unwrap().len(), 1);
    assert_eq!(markers(&store).len(), 1);
}

fn legacy_body(session_id: &str) -> String {
    legacy_body_with_state(session_id, "idle")
}

fn legacy_body_with_state(session_id: &str, state: &str) -> String {
    json!({
        "session_id": session_id,
        "state": state,
        "cwd": "/work/x",
        "tmux_pane": null,
        "since": SINCE_IDLE,
    })
    .to_string()
}

#[test]
fn legacy_files_are_never_deleted() {
    let h = Harness::new();
    h.write_three_states();
    h.write_raw("broken.json", "{ not json");
    let before = h.legacy_file_names();
    let store = h.store();

    legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    assert_eq!(h.legacy_file_names(), before);
}

#[test]
fn empty_legacy_dir_is_a_clean_no_op_that_records_the_marker() {
    let h = Harness::new();
    let store = h.store();

    let report = legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    assert_eq!(report, legacy::MigrateReport::default());
    assert!(store.list_sessions().unwrap().is_empty());
    assert_eq!(markers(&store).len(), 1);
}

#[test]
fn missing_legacy_dir_is_a_clean_no_op_that_records_the_marker() {
    let h = Harness::new();
    fs::remove_dir_all(&h.runtime_dir).unwrap();
    let store = h.store();

    let report = legacy::migrate(&store, &h.runtime_dir, &h.clock).unwrap();

    assert_eq!(report, legacy::MigrateReport::default());
    assert_eq!(markers(&store).len(), 1);
}

#[test]
fn the_hook_path_never_triggers_migration() {
    let h = Harness::new();
    h.write_legacy("sess-old", "working", SINCE_WORKING, None);
    let ctx = HookContext {
        agent: "claude",
        state_dir: &h.state_dir,
        tmux_pane: None,
        clock: &h.clock,
        git: &NoGit,
        agent_pid: None,
        env: None,
    };

    ingest(
        &ctx,
        &json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "sess-new",
            "cwd": "/work/new",
            "prompt": "go",
        })
        .to_string(),
    );

    let store = h.store();
    assert!(markers(&store).is_empty());
    assert!(!legacy::is_migrated(&store).unwrap());
    assert!(store
        .get_session_by_agent_id("claude", "sess-old")
        .unwrap()
        .is_none());
    assert!(store
        .get_session_by_agent_id("claude", "sess-new")
        .unwrap()
        .is_some());
}

fn run_json(h: &Harness) -> Value {
    let fake = FakeEnv::new();
    let output = migrate::run(&h.state_dir, &h.runtime_dir, &fake.env).unwrap();
    serde_json::from_str(&output.render(true)).unwrap()
}

#[test]
fn json_output_has_the_documented_shape() {
    let h = Harness::new();
    h.write_three_states();
    h.write_raw("broken.json", "{ not json");

    let first = run_json(&h);
    assert_eq!(
        first,
        json!({
            "ok": true, "imported": 3, "skipped_existing": 0, "skipped_corrupt": 1,
            "corrupt_files": ["broken.json"],
        })
    );

    let second = run_json(&h);
    assert_eq!(
        second,
        json!({
            "ok": true, "imported": 0, "skipped_existing": 3, "skipped_corrupt": 1,
            "corrupt_files": ["broken.json"],
        })
    );
}

#[test]
fn json_output_without_failures_has_exactly_the_four_counters() {
    let h = Harness::new();
    h.write_three_states();

    assert_eq!(
        run_json(&h),
        json!({"ok": true, "imported": 3, "skipped_existing": 0, "skipped_corrupt": 0})
    );
}

#[test]
fn exit_code_is_3_only_when_every_candidate_failed() {
    let h = Harness::new();
    h.write_raw("broken.json", "{ not json");
    let fake = FakeEnv::new();

    let err = migrate::run(&h.state_dir, &h.runtime_dir, &fake.env).unwrap_err();
    assert_eq!(err.exit_code(), 3);
    assert!(err.to_string().contains("broken.json"));

    let h = Harness::new();
    h.write_legacy("sess-ok", "idle", SINCE_IDLE, None);
    h.write_raw("broken.json", "{ not json");
    assert!(migrate::run(&h.state_dir, &h.runtime_dir, &fake.env).is_ok());
}
