#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use claude_monitor::hook::{ingest, FsGit, GitInfo, GitProbe, HookContext, NoGit};
use claude_monitor::model::{Author, Session, State};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeClock;
use serde_json::Value;
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T10:00:05Z";
const T2: &str = "2026-10-12T10:00:10Z";
const CWD: &str = "/tmp/SCRATCH/r2/work";

struct FakeGit(Option<GitInfo>);

impl GitProbe for FakeGit {
    fn probe(&self, _cwd: &str) -> Option<GitInfo> {
        self.0.clone()
    }
}

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    clock: FakeClock,
    git: FakeGit,
}

impl Harness {
    fn new() -> Self {
        Self::with_git(None)
    }

    fn with_git(git: Option<GitInfo>) -> Self {
        let dir = TempDir::new().unwrap();
        Self {
            state_dir: dir.path().join("state"),
            _dir: dir,
            clock: FakeClock::new(T0),
            git: FakeGit(git),
        }
    }

    fn feed_agent(&self, agent: &str, pane: Option<&str>, raw: &str) {
        let ctx = HookContext {
            agent,
            state_dir: &self.state_dir,
            tmux_pane: pane,
            clock: &self.clock,
            git: &self.git,
            agent_pid: None,
            env: None,
        };
        ingest(&ctx, raw);
    }

    fn feed(&self, pane: Option<&str>, raw: &str) {
        self.feed_agent("claude", pane, raw);
    }

    fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer()).unwrap()
    }

    fn session(&self, agent_session_id: &str) -> Session {
        self.store()
            .get_session_by_agent_id("claude", agent_session_id)
            .unwrap()
            .expect("session row exists")
    }

    fn hook_log(&self) -> String {
        std::fs::read_to_string(self.state_dir.join("hook.log")).unwrap_or_default()
    }
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/claude/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn payload(name: &str, id: &str) -> String {
    payload_with(name, id, |_| {})
}

fn payload_with(name: &str, id: &str, patch: impl FnOnce(&mut Value)) -> String {
    let mut value = fixture(name);
    value["session_id"] = Value::String(id.to_owned());
    patch(&mut value);
    value.to_string()
}

fn fixture_text(name: &str, field: &str) -> String {
    fixture(name)[field].as_str().unwrap().to_owned()
}

fn event_kinds(store: &Store) -> Vec<String> {
    store
        .events_since(0, 1000)
        .unwrap()
        .into_iter()
        .map(|row| row.kind)
        .collect()
}

#[test]
fn full_claude_sequence_leads_to_the_right_row() {
    let git = GitInfo {
        root: "/tmp/SCRATCH/r2".to_owned(),
        branch: Some("main".to_owned()),
    };
    let h = Harness::with_git(Some(git));
    let id = "sess-a";
    let pane = Some("%3");

    let steps = [
        ("SessionStart-startup", State::Idle),
        ("UserPromptSubmit", State::Working),
        ("PreToolUse-Read", State::Working),
        ("PostToolUse-Read", State::Working),
        ("Stop", State::Idle),
        ("SessionEnd-other", State::Finished),
    ];
    for (name, expected) in steps {
        h.feed(pane, &payload(name, id));
        assert_eq!(h.session(id).state, expected, "after {name}");
    }

    let s = h.session(id);
    assert_eq!(s.agent_kind, "claude");
    assert_eq!(s.cwd, CWD);
    assert_eq!(s.tmux_pane.as_deref(), Some("%3"));
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.repo_root.as_deref(), Some("/tmp/SCRATCH/r2"));
    assert_eq!(s.task_description.as_deref(), Some("Say hi"));
    assert_eq!(
        s.left_off.as_deref(),
        Some(fixture_text("Stop", "last_assistant_message").as_str())
    );
    assert_eq!(s.end_reason.as_deref(), Some("other"));
    assert_eq!(s.resumed_from, None);
    assert_eq!(h.store().list_sessions().unwrap().len(), 1);
}

#[test]
fn branch_is_none_when_cwd_is_not_a_git_repo() {
    let h = Harness::new();
    h.feed(None, &payload("SessionStart-startup", "sess-a"));
    let s = h.session("sess-a");
    assert_eq!(s.branch, None);
    assert_eq!(s.repo_root, None);
    assert_eq!(s.tmux_pane, None);
}

#[test]
fn fs_git_reads_branch_from_head_in_a_parent_directory() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".git")).unwrap();
    std::fs::write(repo.path().join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
    let nested = repo.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();

    let info = FsGit.probe(nested.to_str().unwrap()).unwrap();
    assert_eq!(info.branch.as_deref(), Some("feature/x"));
    assert_eq!(info.root, repo.path().to_str().unwrap());

    let plain = TempDir::new().unwrap();
    assert_eq!(FsGit.probe(plain.path().to_str().unwrap()), None);
    assert_eq!(NoGit.probe(CWD), None);
}

#[test]
fn fs_git_follows_a_worktree_gitdir_file() {
    let main = TempDir::new().unwrap();
    let gitdir = main.path().join(".git/worktrees/wt");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/wt-branch\n").unwrap();
    let work = TempDir::new().unwrap();
    std::fs::write(
        work.path().join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();

    let info = FsGit.probe(work.path().to_str().unwrap()).unwrap();
    assert_eq!(info.branch.as_deref(), Some("wt-branch"));
    assert_eq!(info.root, work.path().to_str().unwrap());
}

#[test]
fn task_description_comes_from_the_first_prompt_only() {
    let h = Harness::new();
    h.feed(None, &payload("SessionStart-startup", "sess-a"));
    h.feed(
        None,
        &payload_with("UserPromptSubmit", "sess-a", |v| {
            v["prompt"] = "first prompt".into();
        }),
    );
    h.feed(None, &payload("Stop", "sess-a"));
    h.feed(
        None,
        &payload_with("UserPromptSubmit", "sess-a", |v| {
            v["prompt"] = "second prompt".into();
        }),
    );
    assert_eq!(
        h.session("sess-a").task_description.as_deref(),
        Some("first prompt")
    );
}

#[test]
fn left_off_follows_the_latest_stop() {
    let h = Harness::new();
    h.feed(None, &payload("SessionStart-startup", "sess-a"));
    for text in ["one", "two"] {
        h.feed(None, &payload("UserPromptSubmit", "sess-a"));
        h.feed(
            None,
            &payload_with("Stop", "sess-a", |v| {
                v["last_assistant_message"] = text.into();
            }),
        );
    }
    assert_eq!(h.session("sess-a").left_off.as_deref(), Some("two"));
}

#[test]
fn permission_sequence_moves_through_needs_permission_to_idle() {
    let h = Harness::new();
    let id = "sess-a";
    h.feed(None, &payload("SessionStart-startup", id));
    for (name, expected) in [
        ("PreToolUse-Read", State::Working),
        ("PermissionRequest", State::NeedsPermission),
        ("Notification-permission_prompt", State::NeedsPermission),
        ("Notification-idle_prompt", State::Idle),
    ] {
        h.feed(None, &payload(name, id));
        assert_eq!(h.session(id).state, expected, "after {name}");
    }
}

#[test]
fn tool_search_events_are_ignored() {
    let h = Harness::new();
    h.feed(None, &payload("SessionStart-startup", "sess-a"));
    let before = h.store().last_seq().unwrap();
    for name in ["PreToolUse-Read", "PostToolUse-Read"] {
        h.feed(
            None,
            &payload_with(name, "sess-a", |v| {
                v["tool_name"] = "ToolSearch".into();
            }),
        );
    }
    assert_eq!(h.session("sess-a").state, State::Idle);
    assert_eq!(h.store().last_seq().unwrap(), before);
}

#[test]
fn resume_revives_the_same_row() {
    let h = Harness::new();
    let id = "sess-a";
    h.feed(Some("%1"), &payload("SessionStart-startup", id));
    h.feed(Some("%1"), &payload("SessionEnd-prompt_input_exit", id));
    let ended = h.session(id);
    assert_eq!(ended.state, State::Finished);
    assert_eq!(ended.end_reason.as_deref(), Some("prompt_input_exit"));

    h.clock.set(T1);
    h.feed(Some("%2"), &payload("SessionStart-resume", id));
    let revived = h.session(id);
    assert_eq!(revived.id, ended.id);
    assert_eq!(revived.state, State::Idle);
    assert_eq!(revived.end_reason, None);
    assert_eq!(revived.resumed_from, None);
    assert_eq!(revived.tmux_pane.as_deref(), Some("%2"));
    assert_eq!(revived.state_since, T1);
    assert_eq!(h.store().list_sessions().unwrap().len(), 1);
}

#[test]
fn clear_creates_a_new_row_linked_by_pane() {
    let h = Harness::new();
    h.feed(Some("%1"), &payload("SessionStart-startup", "old"));
    h.feed(Some("%1"), &payload("SessionEnd-clear", "old"));
    h.clock.set(T1);
    h.feed(Some("%1"), &payload("SessionStart-clear", "new"));

    let old = h.session("old");
    let new = h.session("new");
    assert_ne!(old.id, new.id);
    assert_eq!(old.state, State::Finished);
    assert_eq!(old.end_reason.as_deref(), Some("clear"));
    assert_eq!(new.state, State::Idle);
    assert_eq!(new.resumed_from.as_deref(), Some(old.id.as_str()));
    assert_eq!(h.store().list_sessions().unwrap().len(), 2);
}

#[test]
fn clear_without_a_pane_links_by_cwd() {
    let h = Harness::new();
    h.feed(None, &payload("SessionStart-startup", "old"));
    h.feed(None, &payload("SessionEnd-clear", "old"));
    h.feed(None, &payload("SessionStart-clear", "new"));
    assert_eq!(
        h.session("new").resumed_from.as_deref(),
        Some(h.session("old").id.as_str())
    );
}

#[test]
fn clear_does_not_link_a_session_that_did_not_end_by_clear() {
    let h = Harness::new();
    h.feed(Some("%1"), &payload("SessionStart-startup", "old"));
    h.feed(Some("%1"), &payload("SessionEnd-other", "old"));
    h.feed(Some("%1"), &payload("SessionStart-clear", "new"));
    assert_eq!(h.session("new").resumed_from, None);
}

#[test]
fn a_predecessor_is_linked_to_only_one_successor() {
    let h = Harness::new();
    h.feed(None, &payload("SessionStart-startup", "old"));
    h.feed(None, &payload("SessionEnd-clear", "old"));
    h.feed(None, &payload("SessionStart-clear", "first"));
    h.feed(None, &payload("SessionStart-clear", "second"));
    assert!(h.session("first").resumed_from.is_some());
    assert_eq!(h.session("second").resumed_from, None);
}

#[test]
fn real_changes_append_events_with_authors_and_repeats_do_not() {
    let h = Harness::new();
    let id = "sess-a";
    h.feed(Some("%1"), &payload("SessionStart-startup", id));
    for name in [
        "UserPromptSubmit",
        "PreToolUse-Read",
        "PostToolUse-Read",
        "PreToolUse-Read",
        "Stop",
        "SessionEnd-other",
    ] {
        h.feed(Some("%1"), &payload(name, id));
    }

    let store = h.store();
    assert_eq!(
        event_kinds(&store),
        [
            "session.created",
            "session.state_changed",
            "session.task_description_set",
            "session.state_changed",
            "session.left_off_changed",
            "session.state_changed",
        ]
    );
    let session_id = h.session(id).id;
    let agent = Author::Agent {
        kind: "claude".to_owned(),
        model: None,
    };
    let system = Author::System {
        component: "hook".to_owned(),
    };
    for (index, row) in store.events_since(0, 1000).unwrap().iter().enumerate() {
        assert_eq!(row.entity_kind, "session");
        assert_eq!(row.entity_id, session_id);
        let expected = if index == 0 { &system } else { &agent };
        assert_eq!(&row.author, expected, "event {} ({})", index, row.kind);
    }
}

#[test]
fn every_hook_updates_last_activity_without_an_event() {
    let h = Harness::new();
    h.feed(None, &payload("SessionStart-startup", "sess-a"));
    h.feed(None, &payload("UserPromptSubmit", "sess-a"));
    let events = h.store().last_seq().unwrap();
    assert_eq!(h.session("sess-a").last_activity_at, T0);

    h.clock.set(T1);
    h.feed(None, &payload("PreToolUse-Read", "sess-a"));
    h.clock.set(T2);
    h.feed(None, &payload("PostToolUse-Read", "sess-a"));

    let s = h.session("sess-a");
    assert_eq!(s.last_activity_at, T2);
    assert_eq!(s.state_since, T0);
    assert_eq!(h.store().last_seq().unwrap(), events);
}

#[test]
fn a_pane_or_branch_change_appends_a_location_event() {
    let h = Harness::new();
    h.feed(Some("%1"), &payload("SessionStart-startup", "sess-a"));
    h.feed(Some("%1"), &payload("UserPromptSubmit", "sess-a"));
    let before = event_kinds(&h.store()).len();
    h.feed(Some("%4"), &payload("PreToolUse-Read", "sess-a"));
    h.feed(Some("%4"), &payload("PostToolUse-Read", "sess-a"));

    assert_eq!(h.session("sess-a").tmux_pane.as_deref(), Some("%4"));
    let kinds = event_kinds(&h.store());
    assert_eq!(kinds.len(), before + 1);
    assert_eq!(kinds.last().unwrap(), "session.location_changed");
}

#[test]
fn the_hook_no_longer_writes_legacy_runtime_files() {
    let dir = TempDir::new().unwrap();
    let state = dir.path().join("state");
    let runtime = dir.path().join("runtime");

    run_binary(
        &state,
        &runtime,
        &[],
        &payload("SessionStart-startup", "sess-a"),
    );
    run_binary(
        &state,
        &runtime,
        &[],
        &payload("UserPromptSubmit", "sess-a"),
    );
    run_binary(
        &state,
        &runtime,
        &[],
        &payload("SessionEnd-other", "sess-a"),
    );

    assert!(!runtime.exists(), "{runtime:?} was created");
    let store = Store::open_in_dir(&state, &StoreOptions::viewer()).unwrap();
    let session = store
        .get_session_by_agent_id("claude", "sess-a")
        .unwrap()
        .unwrap();
    assert_eq!(session.state, State::Finished);
}

#[test]
fn bad_input_never_panics_and_each_failure_logs_one_line() {
    let h = Harness::new();
    h.feed(None, "{ not json");
    h.feed(None, "");
    h.feed(
        None,
        &payload_with("Stop", "sess-a", |v| v["last_assistant_message"] = 7.into()),
    );
    h.feed_agent("bogus", None, &payload("Stop", "sess-a"));

    let log = h.hook_log();
    assert_eq!(log.lines().count(), 4, "{log}");
    assert!(log.lines().next().unwrap().contains("event=-"));
    assert!(log
        .lines()
        .any(|l| l.contains("agent=bogus") && l.contains("event=Stop") && l.contains("unknown")));
    assert!(log
        .lines()
        .any(|l| l.contains("agent=claude") && l.contains("event=Stop")));
}

#[test]
fn payload_without_a_session_id_is_a_silent_no_op() {
    let h = Harness::new();
    h.feed(None, r#"{"hook_event_name":"Stop"}"#);
    h.feed(None, r#"{"session_id":"x"}"#);
    assert_eq!(h.hook_log(), "");
    assert!(h.store().list_sessions().unwrap().is_empty());
}

#[test]
fn log_lines_have_utc_timestamp_agent_event_and_error_in_a_private_file() {
    let h = Harness::new();
    h.clock.set(T1);
    h.feed(None, "garbage");
    h.feed_agent("nope", None, &payload("Stop", "sess-a"));

    let log = h.hook_log();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "appends, never rotates: {log}");
    for line in &lines {
        let (stamp, rest) = line.split_once(' ').unwrap();
        assert_eq!(stamp, T1);
        assert!(stamp.ends_with('Z'));
        assert!(rest.starts_with("agent="), "{line}");
        assert!(rest.contains(" event="), "{line}");
        assert!(rest.contains(" error="), "{line}");
    }
    let mode = std::fs::metadata(h.state_dir.join("hook.log"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn hook_busy_timeout_is_200_ms() {
    assert_eq!(
        StoreOptions::hook().busy_timeout,
        Duration::from_millis(200)
    );
}

#[test]
fn a_locked_database_returns_quickly_and_logs() {
    let h = Harness::new();
    let id = "sess-a";
    h.feed(None, &payload("SessionStart-startup", id));

    let other = rusqlite::Connection::open(h.state_dir.join("state.db")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();

    let started = Instant::now();
    h.feed(None, &payload("UserPromptSubmit", id));
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");

    let log = h.hook_log();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("event=UserPromptSubmit"), "{log}");

    other.execute_batch("ROLLBACK").unwrap();
    assert_eq!(h.session(id).state, State::Idle);
}

fn run_binary(
    state_dir: &std::path::Path,
    runtime_dir: &std::path::Path,
    args: &[&str],
    stdin: &str,
) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-monitor"))
        .arg("--state-dir")
        .arg(state_dir)
        .arg("hook")
        .args(args)
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env_remove("TMUX_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "exit {:?}", out.status);
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
}

#[test]
fn the_binary_exits_zero_with_empty_stdout_for_good_and_bad_input() {
    let dir = TempDir::new().unwrap();
    let state = dir.path().join("state");
    let runtime = dir.path().join("runtime");

    run_binary(
        &state,
        &runtime,
        &["--agent", "claude"],
        &payload("SessionStart-startup", "bin-a"),
    );
    run_binary(&state, &runtime, &[], &payload("UserPromptSubmit", "bin-a"));
    run_binary(&state, &runtime, &[], "not json");
    run_binary(&state, &runtime, &[], "");
    run_binary(
        &state,
        &runtime,
        &["--agent", "bogus"],
        &payload("Stop", "bin-a"),
    );

    let store = Store::open_in_dir(&state, &StoreOptions::viewer()).unwrap();
    let s = store
        .get_session_by_agent_id("claude", "bin-a")
        .unwrap()
        .unwrap();
    assert_eq!(s.state, State::Working);
    let log = std::fs::read_to_string(state.join("hook.log")).unwrap();
    assert_eq!(log.lines().count(), 3, "{log}");
}

#[test]
#[ignore = "benchmark: run explicitly"]
fn hook_p95_under_50ms() {
    let h = Harness::new();
    let id = "bench";
    h.feed(None, &payload("SessionStart-startup", id));
    let names = [
        "UserPromptSubmit",
        "PreToolUse-Read",
        "PostToolUse-Read",
        "Stop",
    ];

    let mut samples: Vec<Duration> = Vec::with_capacity(200);
    for i in 0..200 {
        let raw = payload(names[i % names.len()], id);
        let started = Instant::now();
        h.feed(Some("%1"), &raw);
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[189];
    println!("hook p95 = {p95:?} (max {:?})", samples[199]);
    assert!(p95 < Duration::from_millis(50), "p95 was {p95:?}");
    assert_eq!(h.hook_log(), "");
}
