#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! Copilot passive discovery: session directories under
//! `$COPILOT_HOME/session-state` become `unknown` session records, with no
//! conversation beyond the first prompt.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use claude_monitor::agents::copilot_discover::{
    discover_and_store, scan, MAX_PROMPT_CHARS, READ_WINDOW_BYTES,
};
use claude_monitor::agents::DiscoverError;
use claude_monitor::env::{Env, EnvError, FileMeta, Fs, SystemFs};
use claude_monitor::model::{Author, Session, State};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeVars;
use serde_json::{json, Value};
use tempfile::TempDir;

const CANARY: &str = "CANARY-do-not-copy-9f3a";
const SESSION: &str = "5b0c2c58-0f5e-4d0a-9d0e-0a4a3f5d7c11";
const CWD: &str = "/home/ali/git/my.proj-v2/sub.dir";
const GIT_ROOT: &str = "/home/ali/git/my.proj-v2";
const CREATED: &str = "2026-10-12T09:00:01.250Z";
const UPDATED: &str = "2026-10-12T09:30:00.000Z";

fn workspace_yaml(id: &str) -> String {
    format!(
        "id: {id}\ncwd: {CWD}\ngit_root: {GIT_ROOT}\nrepository: owner/repo\nhost_type: github\n\
         branch: feature/x\nclient_name: copilot-cli\nname: Fix the build\nuser_named: true\n\
         summary_count: 0\nfork_count: 0\ncreated_at: {CREATED}\nupdated_at: {UPDATED}\n"
    )
}

fn event(kind: &str, at: &str, data: &Value) -> Value {
    json!({"type": kind, "data": data, "id": format!("e-{at}"), "parentId": null, "timestamp": at})
}

fn simple_events(id: &str, prompt: &str) -> Vec<Value> {
    vec![
        event(
            "session.start",
            CREATED,
            &json!({"sessionId": id, "copilotVersion": "1.0.93", "startTime": CREATED,
                    "selectedModel": "gpt-5", "context": {"cwd": CWD, "gitRoot": GIT_ROOT}}),
        ),
        event(
            "user.message",
            "2026-10-12T09:00:02.000Z",
            &json!({"content": prompt, "transformedContent": prompt, "messageId": "m1"}),
        ),
        event(
            "assistant.turn_start",
            "2026-10-12T09:00:03.000Z",
            &json!({}),
        ),
        event(
            "assistant.message",
            "2026-10-12T09:00:05.000Z",
            &json!({"content": CANARY, "messageId": "m2"}),
        ),
        event(
            "user.message",
            "2026-10-12T09:10:00.000Z",
            &json!({"content": format!("second prompt {CANARY}")}),
        ),
        event(
            "assistant.message",
            UPDATED,
            &json!({"content": CANARY, "toolRequests": [{"arguments": CANARY}]}),
        ),
    ]
}

fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

struct CountingFs {
    inner: SystemFs,
    range_bytes: AtomicU64,
    slurped: AtomicBool,
}

impl Fs for CountingFs {
    fn read_to_string(&self, path: &Path) -> Result<String, EnvError> {
        self.slurped.store(true, Ordering::SeqCst);
        self.inner.read_to_string(path)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>, EnvError> {
        self.inner.list_dir(path)
    }
    fn read_link(&self, path: &Path) -> Result<PathBuf, EnvError> {
        self.inner.read_link(path)
    }
    fn metadata(&self, path: &Path) -> Result<FileMeta, EnvError> {
        self.inner.metadata(path)
    }
    fn read_range(&self, path: &Path, offset: u64, max_len: usize) -> Result<Vec<u8>, EnvError> {
        let bytes = self.inner.read_range(path, offset, max_len)?;
        self.range_bytes.fetch_add(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
        Ok(bytes)
    }
}

struct Fx {
    _dir: TempDir,
    home: PathBuf,
    state_dir: PathBuf,
    env: Env,
}

impl Fx {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("copilot-home");
        let vars = Arc::new(FakeVars::default());
        vars.set("COPILOT_HOME", home.to_str().unwrap());
        let mut env = Env::real();
        env.vars = vars;
        Self {
            state_dir: dir.path().join("state"),
            home,
            _dir: dir,
            env,
        }
    }

    fn session_state(&self) -> PathBuf {
        self.home.join("session-state")
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        let dir = self.session_state().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(&self, id: &str, yaml: &str, events: &[Value]) -> PathBuf {
        let dir = self.session_dir(id);
        std::fs::write(dir.join("workspace.yaml"), yaml).unwrap();
        std::fs::write(dir.join("events.jsonl"), jsonl(events)).unwrap();
        dir
    }

    fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::default()).unwrap()
    }
}

#[test]
#[ignore = "T040"]
fn a_session_directory_becomes_a_stub() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "Fix the build"),
    );

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.degraded, 0);
    assert_eq!(report.stubs.len(), 1);
    let stub = &report.stubs[0];
    assert_eq!(stub.agent_session_id, SESSION);
    assert_eq!(stub.cwd, CWD);
    assert_eq!(stub.repo_root.as_deref(), Some(GIT_ROOT));
    assert_eq!(stub.branch.as_deref(), Some("feature/x"));
    assert_eq!(stub.name.as_deref(), Some("Fix the build"));
    assert_eq!(stub.started_at, "2026-10-12T09:00:01Z");
    assert_eq!(stub.last_activity_at, "2026-10-12T09:30:00Z");
    assert_eq!(stub.first_prompt.as_deref(), Some("Fix the build"));
}

#[test]
#[ignore = "T040"]
fn the_yaml_reader_tolerates_quotes_empty_values_and_crlf() {
    let fx = Fx::new();
    let yaml = format!(
        "id: \"{SESSION}\"\r\ncwd: '{CWD}'\r\ngit_root:\r\nbranch: \"\"\r\nname:\r\n\
         unknown_key: whatever\r\ncreated_at: {CREATED}\r\nupdated_at: {UPDATED}\r\n"
    );
    fx.write(SESSION, &yaml, &simple_events(SESSION, "Hi"));

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.degraded, 0);
    let stub = &report.stubs[0];
    assert_eq!(stub.agent_session_id, SESSION);
    assert_eq!(stub.cwd, CWD);
    assert_eq!(stub.repo_root, None);
    assert_eq!(stub.branch, None);
    assert_eq!(stub.name, None);
}

#[test]
#[ignore = "T040"]
fn a_long_prompt_is_truncated_to_the_character_limit() {
    let fx = Fx::new();
    let long = "é".repeat(MAX_PROMPT_CHARS + 100);
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, &long),
    );

    let stub = scan(&fx.env).unwrap().stubs.remove(0);

    let prompt = stub.first_prompt.unwrap();
    assert_eq!(prompt.chars().count(), MAX_PROMPT_CHARS);
    assert!(prompt.chars().all(|c| c == 'é'));
}

#[test]
#[ignore = "T040"]
fn no_assistant_or_tool_text_reaches_a_stub_or_a_stored_row() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "Fix the build"),
    );
    let store = fx.store();

    let report = scan(&fx.env).unwrap();
    discover_and_store(&fx.env, &store).unwrap();

    assert!(!format!("{report:?}").contains(CANARY));
    assert!(!format!("{:?}", store.list_sessions().unwrap()).contains(CANARY));
    assert!(!format!("{:?}", store.events_since(0, 100).unwrap()).contains(CANARY));
}

#[test]
#[ignore = "T040"]
fn discovery_stores_an_unknown_session_and_a_system_event_once() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "Fix the build"),
    );
    let store = fx.store();

    let first = discover_and_store(&fx.env, &store).unwrap();
    let second = discover_and_store(&fx.env, &store).unwrap();

    assert_eq!((first.created, first.updated), (1, 0));
    assert_eq!((second.created, second.updated), (0, 0));
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.agent_kind, "copilot");
    assert_eq!(session.agent_session_id, SESSION);
    assert_eq!(session.state, State::Unknown);
    assert_eq!(session.cwd, CWD);
    assert_eq!(session.repo_root.as_deref(), Some(GIT_ROOT));
    assert_eq!(session.branch.as_deref(), Some("feature/x"));
    assert_eq!(session.started_at, "2026-10-12T09:00:01Z");
    assert_eq!(session.last_activity_at, "2026-10-12T09:30:00Z");
    assert_eq!(session.task_description.as_deref(), Some("Fix the build"));
    let events = store.events_since(0, 100).unwrap();
    assert_eq!(events.len(), 1, "the rescan appends nothing");
    assert_eq!(events[0].kind, "session.discovered");
    assert_eq!(
        events[0].author,
        Author::System {
            component: "discover".to_owned()
        }
    );
}

#[test]
#[ignore = "T040"]
fn an_empty_directory_is_skipped_and_counted_degraded() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "Fix the build"),
    );
    fx.session_dir("0a11b8db-4bb1-4cff-acc6-f4f943fe4a76");

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.stubs.len(), 1);
    assert_eq!(report.degraded, 1);
}

#[test]
#[ignore = "T040"]
fn a_missing_workspace_yaml_is_degraded() {
    let fx = Fx::new();
    let dir = fx.session_dir(SESSION);
    std::fs::write(
        dir.join("events.jsonl"),
        jsonl(&simple_events(SESSION, "x")),
    )
    .unwrap();

    let report = scan(&fx.env).unwrap();

    assert!(report.stubs.is_empty());
    assert_eq!(report.degraded, 1);
}

#[test]
#[ignore = "T040"]
fn unparsable_yaml_is_degraded_and_does_not_fail_the_scan() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        "{{{ not: yaml: at all\n\t- [",
        &simple_events(SESSION, "x"),
    );
    let other = "7bb1ff4a-8486-442e-b134-55a7a63408ec";
    let dir = fx.session_dir(other);
    std::fs::write(
        dir.join("workspace.yaml"),
        [0xff_u8, 0xfe, 0x00, b'\n', 0xc3],
    )
    .unwrap();

    let report = scan(&fx.env).unwrap();

    assert!(report.stubs.is_empty());
    assert_eq!(report.degraded, 2);
}

#[test]
#[ignore = "T040"]
fn a_missing_events_file_gives_a_stub_without_a_prompt() {
    let fx = Fx::new();
    let dir = fx.session_dir(SESSION);
    std::fs::write(dir.join("workspace.yaml"), workspace_yaml(SESSION)).unwrap();

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.degraded, 0);
    assert_eq!(report.stubs.len(), 1);
    assert_eq!(report.stubs[0].first_prompt, None);
    assert_eq!(report.stubs[0].cwd, CWD);
}

#[test]
#[ignore = "T040"]
fn a_huge_events_file_is_read_within_the_byte_bound() {
    let mut fx = Fx::new();
    let dir = fx.session_dir(SESSION);
    std::fs::write(dir.join("workspace.yaml"), workspace_yaml(SESSION)).unwrap();
    let path = dir.join("events.jsonl");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    out.write_all(jsonl(&simple_events(SESSION, "Huge one")[..3]).as_bytes())
        .unwrap();
    let filler = format!(
        "{}\n",
        event(
            "assistant.message",
            "2026-10-12T09:05:00.000Z",
            &json!({"content": "x".repeat(1000)})
        )
    );
    for _ in 0..(20 * 1024) {
        out.write_all(filler.as_bytes()).unwrap();
    }
    out.write_all(
        format!(
            "{}\n",
            event("assistant.message", UPDATED, &json!({"content": CANARY}))
        )
        .as_bytes(),
    )
    .unwrap();
    out.flush().unwrap();
    drop(out);
    assert!(std::fs::metadata(&path).unwrap().len() > 20 * 1024 * 1024);
    let counting = Arc::new(CountingFs {
        inner: SystemFs,
        range_bytes: AtomicU64::new(0),
        slurped: AtomicBool::new(false),
    });
    fx.env.fs = counting.clone();

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.stubs.len(), 1);
    assert_eq!(report.stubs[0].first_prompt.as_deref(), Some("Huge one"));
    assert!(!format!("{report:?}").contains(CANARY));
    let read = counting.range_bytes.load(Ordering::SeqCst);
    assert!(read <= 2 * READ_WINDOW_BYTES as u64, "read {read} bytes");
    assert!(!counting.slurped.load(Ordering::SeqCst), "never read whole");
}

#[test]
#[ignore = "T040"]
fn non_directories_in_session_state_are_skipped_and_counted_degraded() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "Fix the build"),
    );
    std::fs::write(fx.session_state().join("notes.txt"), "stray").unwrap();
    std::fs::write(fx.session_state().join(".lock"), "").unwrap();

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.stubs.len(), 1);
    assert_eq!(report.degraded, 2);
}

#[test]
#[ignore = "T040"]
fn ids_with_path_traversal_characters_are_rejected() {
    let fx = Fx::new();
    for (dir_name, id) in [
        ("evil-1", "../../etc/passwd"),
        ("evil-2", "a/b"),
        ("evil-3", "a b;rm"),
        ("evil-4", ".."),
    ] {
        fx.write(dir_name, &workspace_yaml(id), &simple_events(id, "x"));
    }
    let long = "a".repeat(200);
    fx.write("evil-5", &workspace_yaml(&long), &simple_events(&long, "x"));

    let report = scan(&fx.env).unwrap();

    assert!(report.stubs.is_empty(), "{:?}", report.stubs);
    assert_eq!(report.degraded, 5);
}

#[test]
#[ignore = "T040"]
fn discovery_never_downgrades_a_session_a_hook_reported() {
    let fx = Fx::new();
    let store = fx.store();
    let mut known = Session::new(
        "row-1",
        "copilot",
        SESSION,
        "/tmp/hook-cwd",
        "2026-10-12T11:00:00Z",
    );
    known.state = State::Working;
    known.task_description = Some("Say hi".to_owned());
    store.upsert_session(&known).unwrap();
    let before = store
        .get_session_by_agent_id("copilot", SESSION)
        .unwrap()
        .unwrap();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "Fix the build"),
    );

    let summary = discover_and_store(&fx.env, &store).unwrap();

    assert_eq!(summary.created, 0);
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state, State::Working);
    assert_eq!(sessions[0].cwd, "/tmp/hook-cwd");
    assert_eq!(sessions[0].task_description.as_deref(), Some("Say hi"));
    assert_eq!(sessions[0].started_at, before.started_at);
    assert_eq!(sessions[0].last_activity_at, before.last_activity_at);
}

#[test]
#[ignore = "T040"]
fn a_later_hook_upgrades_a_discovered_session_in_place() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "Fix the build"),
    );
    let store = fx.store();
    discover_and_store(&fx.env, &store).unwrap();
    let mut discovered = store
        .get_session_by_agent_id("copilot", SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(discovered.state, State::Unknown);

    // The hook path looks the row up by agent id and rewrites it.
    discovered.state = State::Working;
    store.upsert_session(&discovered).unwrap();

    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1, "no duplicate row");
    assert_eq!(sessions[0].id, discovered.id);
    assert_eq!(sessions[0].state, State::Working);
    discover_and_store(&fx.env, &store).unwrap();
    assert_eq!(store.list_sessions().unwrap()[0].state, State::Working);
}

#[test]
#[ignore = "T040"]
fn an_absent_session_state_directory_is_an_empty_result() {
    let fx = Fx::new();
    let store = fx.store();

    assert_eq!(scan(&fx.env).unwrap().stubs, Vec::new());
    let summary = discover_and_store(&fx.env, &store).unwrap();

    assert_eq!(
        (summary.created, summary.updated, summary.degraded),
        (0, 0, 0)
    );
    assert!(store.list_sessions().unwrap().is_empty());
}

#[test]
#[ignore = "T040"]
fn the_root_is_copilot_home_then_home_dot_copilot_and_errors_without_either() {
    let fx = Fx::new();
    fx.write(
        SESSION,
        &workspace_yaml(SESSION),
        &simple_events(SESSION, "From COPILOT_HOME"),
    );
    assert_eq!(scan(&fx.env).unwrap().stubs.len(), 1);

    let home = fx._dir.path().join("home");
    let dir = home.join(".copilot").join("session-state").join(SESSION);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("workspace.yaml"), workspace_yaml(SESSION)).unwrap();
    let mut env = Env::real();
    let vars = Arc::new(FakeVars::default());
    vars.set("HOME", home.to_str().unwrap());
    env.vars = vars;
    assert_eq!(scan(&env).unwrap().stubs.len(), 1);

    env.vars = Arc::new(FakeVars::default());
    assert!(matches!(
        scan(&env),
        Err(DiscoverError::NoConfigDir { kind: "copilot" })
    ));
}
