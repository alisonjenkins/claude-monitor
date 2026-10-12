#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! Claude passive discovery: transcripts under `<config dir>/projects` become
//! `unknown` session records, with no conversation beyond the first prompt.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use claude_monitor::agents::claude::ClaudeKind;
use claude_monitor::agents::claude_discover::{
    discover_and_store, scan, MAX_BRANCH_BYTES, MAX_CWD_BYTES, MAX_PROMPT_CHARS, READ_WINDOW_BYTES,
};
use claude_monitor::agents::{AgentKind, DiscoverError};
use claude_monitor::env::{Env, EnvError, FileMeta, Fs, SystemFs};
use claude_monitor::feed::{ClaudeProjectsSource, EventSource};
use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::{Author, State};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::{FakeClock, FakeVars};
use serde_json::{json, Value};
use tempfile::TempDir;

const CANARY: &str = "CANARY-do-not-copy-9f3a";
const SESSION: &str = "238ed769-bbd3-44c5-a49d-c726cdf2a392";
/// Dots and dashes in the path make the directory encoding ambiguous.
const CWD: &str = "/home/ali/git/my.proj-v2/sub.dir";
const T_FIRST: &str = "2026-10-12T09:00:01.250Z";
const T_LAST: &str = "2026-10-12T09:30:00.000Z";
const HOOK_NOW: &str = "2026-10-12T11:00:00Z";

/// Every non-alphanumeric character becomes `-`, as Claude Code names the
/// project directory.
fn encode(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn user_line(id: &str, cwd: &str, branch: &str, at: &str, content: Value) -> Value {
    json!({
        "type": "user", "uuid": format!("u-{at}"), "parentUuid": null, "sessionId": id,
        "timestamp": at, "cwd": cwd, "gitBranch": branch, "version": "2.1.293",
        "isSidechain": false,
        "message": {"role": "user", "content": content},
    })
}

fn assistant_line(id: &str, cwd: &str, branch: &str, at: &str, text: &str) -> Value {
    json!({
        "type": "assistant", "uuid": format!("a-{at}"), "parentUuid": null, "sessionId": id,
        "timestamp": at, "cwd": cwd, "gitBranch": branch, "version": "2.1.293",
        "isSidechain": false,
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
    })
}

fn preamble() -> Vec<Value> {
    vec![
        json!({"type": "mode", "mode": "auto"}),
        json!({"type": "queue-operation", "operation": "enqueue"}),
        json!({"type": "attachment", "attachment": {"type": "hook_success"}}),
    ]
}

fn simple_transcript(id: &str, prompt: &str) -> Vec<Value> {
    let mut lines = preamble();
    lines.push(user_line(id, CWD, "main", T_FIRST, json!(prompt)));
    lines.push(assistant_line(
        id,
        CWD,
        "main",
        "2026-10-12T09:00:05.000Z",
        CANARY,
    ));
    lines.push(assistant_line(id, CWD, "main", T_LAST, CANARY));
    lines
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
    config: PathBuf,
    state_dir: PathBuf,
    env: Env,
}

impl Fx {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("claude-config");
        let vars = Arc::new(FakeVars::default());
        vars.set("CLAUDE_CONFIG_DIR", config.to_str().unwrap());
        let mut env = Env::real();
        env.vars = vars;
        Self {
            state_dir: dir.path().join("state"),
            config,
            _dir: dir,
            env,
        }
    }

    fn projects(&self) -> PathBuf {
        self.config.join("projects")
    }

    fn project_dir(&self, cwd: &str) -> PathBuf {
        let dir = self.projects().join(encode(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(&self, cwd: &str, id: &str, lines: &[Value]) -> PathBuf {
        let path = self.project_dir(cwd).join(format!("{id}.jsonl"));
        std::fs::write(&path, jsonl(lines)).unwrap();
        path
    }

    fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::default()).unwrap()
    }

    fn hook(&self, raw: &Value) {
        let clock = FakeClock::new(HOOK_NOW);
        let ctx = HookContext {
            agent: "claude",
            state_dir: &self.state_dir,
            tmux_pane: None,
            clock: &clock,
            git: &NoGit,
        };
        ingest(&ctx, &raw.to_string());
    }
}

#[test]
fn a_transcript_becomes_a_stub_with_cwd_taken_from_its_lines() {
    let fx = Fx::new();
    fx.write(CWD, SESSION, &simple_transcript(SESSION, "Fix the build"));

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.degraded, 0);
    assert_eq!(report.stubs.len(), 1);
    let stub = &report.stubs[0];
    assert_eq!(stub.agent_session_id, SESSION);
    assert_eq!(stub.cwd, CWD);
    assert_ne!(stub.cwd, encode(CWD).replace('-', "/"), "never decoded");
    assert_eq!(stub.branch.as_deref(), Some("main"));
    assert_eq!(stub.started_at, "2026-10-12T09:00:01Z");
    assert_eq!(stub.last_activity_at, "2026-10-12T09:30:00Z");
    assert_eq!(stub.first_prompt.as_deref(), Some("Fix the build"));
    assert_eq!(
        ClaudeKind.discover(&fx.env).unwrap(),
        report.stubs,
        "the trait method returns the same stubs"
    );
}

#[test]
fn the_first_prompt_may_be_a_block_array_and_skips_tool_results() {
    let fx = Fx::new();
    let mut lines = preamble();
    lines.push(user_line(
        SESSION,
        CWD,
        "main",
        T_FIRST,
        json!([
            {"type": "tool_result", "content": CANARY},
            {"type": "text", "text": "Refactor the parser"},
        ]),
    ));
    fx.write(CWD, SESSION, &lines);

    let stub = scan(&fx.env).unwrap().stubs.remove(0);

    assert_eq!(stub.first_prompt.as_deref(), Some("Refactor the parser"));
    assert!(!format!("{stub:?}").contains(CANARY));
}

#[test]
fn a_long_prompt_is_truncated_to_the_character_limit() {
    let fx = Fx::new();
    let long = "é".repeat(MAX_PROMPT_CHARS + 100);
    fx.write(CWD, SESSION, &simple_transcript(SESSION, &long));

    let stub = scan(&fx.env).unwrap().stubs.remove(0);

    let prompt = stub.first_prompt.unwrap();
    assert_eq!(prompt.chars().count(), MAX_PROMPT_CHARS);
    assert!(prompt.chars().all(|c| c == 'é'));
}

#[test]
fn cwd_is_the_original_and_branch_and_activity_are_the_latest() {
    let fx = Fx::new();
    let mut lines = preamble();
    lines.push(user_line(SESSION, CWD, "main", T_FIRST, json!("Start")));
    lines.push(assistant_line(
        SESSION,
        "/home/ali/git/elsewhere",
        "feature/x",
        T_LAST,
        CANARY,
    ));
    fx.write(CWD, SESSION, &lines);

    let stub = scan(&fx.env).unwrap().stubs.remove(0);

    assert_eq!(stub.cwd, CWD);
    assert_eq!(stub.branch.as_deref(), Some("feature/x"));
    assert_eq!(stub.last_activity_at, "2026-10-12T09:30:00Z");
}

#[test]
fn no_assistant_or_tool_text_reaches_a_stub_or_a_stored_row() {
    let fx = Fx::new();
    let mut lines = simple_transcript(SESSION, "Fix the build");
    lines.push(user_line(
        SESSION,
        CWD,
        "main",
        "2026-10-12T09:10:00.000Z",
        json!(format!("second prompt {CANARY}")),
    ));
    fx.write(CWD, SESSION, &lines);
    let store = fx.store();

    let report = scan(&fx.env).unwrap();
    discover_and_store(&fx.env, &store).unwrap();

    assert!(!format!("{report:?}").contains(CANARY));
    assert!(!format!("{:?}", store.list_sessions().unwrap()).contains(CANARY));
    assert!(!format!("{:?}", store.events_since(0, 100).unwrap()).contains(CANARY));
}

#[test]
fn discovery_stores_an_unknown_session_and_a_system_event_once() {
    let fx = Fx::new();
    fx.write(CWD, SESSION, &simple_transcript(SESSION, "Fix the build"));
    let store = fx.store();

    let first = discover_and_store(&fx.env, &store).unwrap();
    let second = discover_and_store(&fx.env, &store).unwrap();

    assert_eq!((first.created, first.updated), (1, 0));
    assert_eq!((second.created, second.updated), (0, 0));
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.agent_kind, "claude");
    assert_eq!(session.agent_session_id, SESSION);
    assert_eq!(session.state, State::Unknown);
    assert_eq!(session.cwd, CWD);
    assert_eq!(session.branch.as_deref(), Some("main"));
    assert_eq!(session.started_at, "2026-10-12T09:00:01Z");
    assert_eq!(session.last_activity_at, "2026-10-12T09:30:00Z");
    assert_eq!(session.task_description.as_deref(), Some("Fix the build"));
    let events = store.events_since(0, 100).unwrap();
    assert_eq!(events.len(), 1, "the rescan appends nothing");
    assert_eq!(events[0].kind, "session.discovered");
    assert_eq!(events[0].entity_id, session.id);
    assert_eq!(
        events[0].author,
        Author::System {
            component: "discover".to_owned()
        }
    );
}

#[test]
fn discovery_never_downgrades_a_session_a_hook_reported() {
    let fx = Fx::new();
    fx.hook(&json!({
        "session_id": SESSION, "hook_event_name": "UserPromptSubmit",
        "cwd": "/tmp/hook-cwd", "prompt": "Say hi",
    }));
    let before = fx
        .store()
        .get_session_by_agent_id("claude", SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(before.state, State::Working);
    fx.write(CWD, SESSION, &simple_transcript(SESSION, "Fix the build"));

    let summary = discover_and_store(&fx.env, &fx.store()).unwrap();

    assert_eq!(summary.created, 0);
    let sessions = fx.store().list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0], before, "every hook-derived field is untouched");
    assert_eq!(sessions[0].cwd, "/tmp/hook-cwd");
    assert_eq!(sessions[0].task_description.as_deref(), Some("Say hi"));
}

#[test]
fn a_later_hook_upgrades_a_discovered_session_in_place() {
    let fx = Fx::new();
    fx.write(CWD, SESSION, &simple_transcript(SESSION, "Fix the build"));
    discover_and_store(&fx.env, &fx.store()).unwrap();
    let discovered = fx.store().list_sessions().unwrap().remove(0);
    assert_eq!(discovered.state, State::Unknown);

    fx.hook(&json!({
        "session_id": SESSION, "hook_event_name": "PreToolUse",
        "cwd": CWD, "tool_name": "Read",
    }));

    let sessions = fx.store().list_sessions().unwrap();
    assert_eq!(sessions.len(), 1, "no duplicate row");
    assert_eq!(sessions[0].id, discovered.id);
    assert_eq!(sessions[0].state, State::Working);
}

#[test]
fn a_huge_transcript_is_read_within_the_byte_bound() {
    let mut fx = Fx::new();
    let path = fx.project_dir(CWD).join(format!("{SESSION}.jsonl"));
    let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    out.write_all(jsonl(&simple_transcript(SESSION, "Huge one")[..4]).as_bytes())
        .unwrap();
    let filler = assistant_line(
        SESSION,
        CWD,
        "main",
        "2026-10-12T09:05:00.000Z",
        &"x".repeat(1000),
    );
    let filler = format!("{filler}\n");
    for _ in 0..(20 * 1024) {
        out.write_all(filler.as_bytes()).unwrap();
    }
    let last = assistant_line(SESSION, CWD, "main", "2026-10-12T10:15:00.000Z", CANARY);
    out.write_all(format!("{last}\n").as_bytes()).unwrap();
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
    assert_eq!(report.stubs[0].last_activity_at, "2026-10-12T10:15:00Z");
    assert!(!counting.slurped.load(Ordering::SeqCst), "never read whole");
    let read = counting.range_bytes.load(Ordering::SeqCst);
    assert!(read <= 2 * READ_WINDOW_BYTES as u64, "read {read} bytes");
}

#[test]
fn unreadable_files_are_skipped_and_counted_without_failing_the_scan() {
    let fx = Fx::new();
    fx.write(CWD, SESSION, &simple_transcript(SESSION, "Fix the build"));
    let dir = fx.project_dir(CWD);
    std::fs::write(dir.join("notes.txt"), "not a transcript").unwrap();
    std::fs::write(dir.join("empty.jsonl"), "").unwrap();
    std::fs::write(dir.join("cut.jsonl"), "{\"type\":\"user\",\"cwd\":\"/x").unwrap();
    std::fs::write(
        dir.join("binary.jsonl"),
        [0xff_u8, 0xfe, 0x00, b'\n', 0xc3, 0x28],
    )
    .unwrap();
    // A session directory with subagent transcripts is not a transcript file.
    let nested = dir.join(SESSION).join("subagents");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        nested.join("agent-1.jsonl"),
        jsonl(&simple_transcript("x", "sub")),
    )
    .unwrap();

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.stubs.len(), 1);
    assert_eq!(report.stubs[0].agent_session_id, SESSION);
    assert_eq!(report.degraded, 4, "txt, empty, cut, binary");
}

#[test]
fn a_truncated_last_line_does_not_hide_the_complete_lines_before_it() {
    let fx = Fx::new();
    let path = fx.write(CWD, SESSION, &simple_transcript(SESSION, "Fix the build"));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"{\"type\":\"assistant\",\"timestamp\":\"2026-10-12T09:59:")
        .unwrap();

    let report = scan(&fx.env).unwrap();

    assert_eq!(report.degraded, 0);
    assert_eq!(report.stubs[0].last_activity_at, "2026-10-12T09:30:00Z");
}

#[test]
fn sidechain_transcripts_are_not_sessions() {
    let fx = Fx::new();
    let mut lines = preamble();
    let mut side = user_line("agent-1", CWD, "main", T_FIRST, json!("sub task"));
    side["isSidechain"] = json!(true);
    lines.push(side);
    fx.write(CWD, "agent-1", &lines);

    let report = scan(&fx.env).unwrap();

    assert!(report.stubs.is_empty());
    assert_eq!(report.degraded, 0);
}

#[test]
fn an_absent_projects_directory_is_an_empty_result() {
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
fn the_root_falls_back_to_home_dot_claude_and_errors_without_either() {
    let fx = Fx::new();
    let home = fx._dir.path().join("home");
    let dir = home.join(".claude").join("projects").join(encode(CWD));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{SESSION}.jsonl")),
        jsonl(&simple_transcript(SESSION, "From home")),
    )
    .unwrap();
    let mut env = Env::real();
    let vars = Arc::new(FakeVars::default());
    vars.set("HOME", home.to_str().unwrap());
    env.vars = vars;

    assert_eq!(scan(&env).unwrap().stubs.len(), 1);

    env.vars = Arc::new(FakeVars::default());
    assert!(matches!(
        scan(&env),
        Err(DiscoverError::NoConfigDir { kind: "claude" })
    ));
}

#[test]
fn stub_fields_respect_their_length_limits() {
    let fx = Fx::new();
    let long_cwd = format!("/{}", "d".repeat(MAX_CWD_BYTES + 1));
    fx.write(
        "/short",
        "too-long-cwd",
        &simple_transcript_at("too-long-cwd", &long_cwd, "b"),
    );
    let long_branch = "b".repeat(MAX_BRANCH_BYTES + 1);
    fx.write(
        "/short",
        "long-branch",
        &simple_transcript_at("long-branch", "/short", &long_branch),
    );

    let report = scan(&fx.env).unwrap();

    assert_eq!(
        report.degraded, 1,
        "an unusable cwd leaves nothing to record"
    );
    assert_eq!(report.stubs.len(), 1);
    assert_eq!(report.stubs[0].agent_session_id, "long-branch");
    assert_eq!(
        report.stubs[0].branch, None,
        "an oversized branch is dropped"
    );
}

fn simple_transcript_at(id: &str, cwd: &str, branch: &str) -> Vec<Value> {
    let mut lines = preamble();
    lines.push(user_line(id, cwd, branch, T_FIRST, json!("hello")));
    lines.push(assistant_line(id, cwd, branch, T_LAST, CANARY));
    lines
}

#[test]
fn the_projects_watch_wakes_on_new_transcripts_and_directories_only() {
    let fx = Fx::new();
    let dir = fx.project_dir(CWD);
    let mut source = ClaudeProjectsSource::watch(&fx.projects()).unwrap();

    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    assert!(!source.wait(Duration::from_millis(300)), "not a transcript");

    std::fs::write(dir.join(format!("{SESSION}.jsonl")), "{}\n").unwrap();
    assert!(source.wait(Duration::from_secs(5)), "new transcript");

    std::fs::create_dir_all(fx.projects().join("-new-project")).unwrap();
    assert!(source.wait(Duration::from_secs(5)), "new project directory");
}
