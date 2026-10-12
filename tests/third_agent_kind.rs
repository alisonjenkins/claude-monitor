//! SC-008: a third agent kind needs no change to how sessions are viewed,
//! stored or discovered.
//!
//! `FakeAgent` below is defined in this file only. Nothing in `src/` knows it
//! exists, yet its sessions flow through the hook ingest path, `Store::overview`,
//! `cli::run_in` (`list`, `show`) and the shared `discover_store::store_report`
//! exactly as Claude's and Copilot's do.
//!
//! Respawn and export do not exist yet. Their steps are `#[ignore]`d tests that
//! fail through `needs` when run with `--ignored`; the task that builds the
//! feature removes the matching `#[ignore]` and fills the body in:
//! T068 (respawn) and T097 (export).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use claude_monitor::agents::discover_store::store_report;
use claude_monitor::agents::{
    AgentKind, DiscoverError, ParseError, ParsedHook, ScanReport, SessionStub,
};
use claude_monitor::cli::{run_in, Command};
use claude_monitor::env::Env;
use claude_monitor::hook::{ingest_with, HookContext, NoGit};
use claude_monitor::model::{Event, Session, StartSource, State};
use claude_monitor::store::queries::OverviewQuery;
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::{FakeClock, FakeEnv};
use serde_json::{json, Value};
use tempfile::TempDir;

const FAKE_ID: &str = "fake-agent";
const T_HOOK: &str = "2026-10-11T23:00:00Z";

/// A made-up agent whose payloads look nothing like Claude's or Copilot's:
/// `{"fa_event": "begin|prompt|ask|done|end", "fa_sid": .., "fa_dir": .., "fa_text": ..}`.
struct FakeAgent;

impl AgentKind for FakeAgent {
    fn id(&self) -> &'static str {
        FAKE_ID
    }

    fn parse_hook(&self, raw: &Value) -> Result<Vec<ParsedHook>, ParseError> {
        let text_field = |field: &'static str| match raw.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(ParseError::WrongType {
                field,
                expected: "a string",
            }),
        };
        let (Some(sid), Some(name)) = (text_field("fa_sid")?, text_field("fa_event")?) else {
            return Ok(Vec::new());
        };
        let text = text_field("fa_text")?;
        let event = match name.as_str() {
            "begin" => Event::SessionStart {
                source: StartSource::Startup,
            },
            "prompt" => Event::UserPromptSubmit,
            "ask" => Event::PermissionRequest,
            "done" => Event::Stop,
            "end" => Event::SessionEnd {
                reason: "other".to_owned(),
            },
            _ => return Ok(Vec::new()),
        };
        let mut hook = ParsedHook::new(event, &sid);
        hook.cwd = text_field("fa_dir")?;
        match name.as_str() {
            "prompt" => hook.prompt = text,
            "done" => hook.last_assistant_message = text,
            _ => {}
        }
        Ok(vec![hook])
    }

    fn discover(&self, _env: &Env) -> Result<Vec<SessionStub>, DiscoverError> {
        Ok(vec![stub("fa-found", "/fake/found")])
    }
}

fn stub(agent_session_id: &str, cwd: &str) -> SessionStub {
    SessionStub {
        agent_session_id: agent_session_id.to_owned(),
        cwd: cwd.to_owned(),
        repo_root: None,
        branch: None,
        name: None,
        started_at: "2026-10-11T20:00:00Z".to_owned(),
        last_activity_at: "2026-10-11T21:00:00Z".to_owned(),
        first_prompt: Some("found on disk".to_owned()),
    }
}

struct Fixture {
    state: TempDir,
    fake: FakeEnv,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let state = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
        // Mark the legacy import done against an empty directory so the
        // viewer never reads the real runtime directory.
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
        Self { state, fake, store }
    }

    /// One payload through the hook path, with the fake kind registered
    /// beside the built-in ones' registry slot.
    fn hook(&self, agent: &str, payload: &Value) {
        let clock = FakeClock::new(T_HOOK);
        let ctx = HookContext {
            agent,
            state_dir: self.state.path(),
            tmux_pane: None,
            clock: &clock,
            git: &NoGit,
            agent_pid: None,
            env: None,
        };
        let claude = claude_monitor::agents::claude::ClaudeKind;
        let kinds: [&dyn AgentKind; 2] = [&claude, &FakeAgent];
        ingest_with(&ctx, &kinds, &payload.to_string());
    }

    fn fa(&self, event: &str, sid: &str, text: Option<&str>) {
        self.hook(
            FAKE_ID,
            &json!({ "fa_event": event, "fa_sid": sid, "fa_dir": format!("/fake/{sid}"), "fa_text": text }),
        );
    }

    fn seed(&self) {
        self.fa("begin", "fa-1", None);
        self.fa("prompt", "fa-1", Some("third agent task"));
        self.fa("ask", "fa-1", None);
        self.fa("begin", "fa-2", None);
        self.fa("done", "fa-2", Some("all finished"));
        self.fa("end", "fa-2", None);
        self.hook(
            "claude",
            &json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "cl-1",
                "cwd": "/work/claude",
                "prompt": "claude task",
            }),
        );
        let mut copilot = Session::new("copilot-row", "copilot", "co-1", "/work/copilot", T_HOOK);
        copilot.state = State::Idle;
        copilot.last_activity_at = T_HOOK.to_owned();
        self.store.upsert_session(&copilot).unwrap();
    }

    fn list_json(&self, agent: Option<&str>) -> Vec<Value> {
        let command = Command::List {
            states: Vec::new(),
            agent: agent.map(str::to_owned),
            repo: None,
            project: None,
            text: None,
            section: None,
            include_dismissed: false,
            expand_finished: false,
        };
        let output = run_in(command, &self.fake.env, Some(self.state.path())).unwrap();
        let value: Value = serde_json::from_str(&output.render(true)).unwrap();
        value["sessions"].as_array().unwrap().clone()
    }

    fn row(&self, agent_session_id: &str) -> Session {
        self.store
            .get_session_by_agent_id(FAKE_ID, agent_session_id)
            .unwrap()
            .unwrap_or_else(|| panic!("no {FAKE_ID} session {agent_session_id}"))
    }
}

fn find<'a>(sessions: &'a [Value], agent_session_id: &str) -> &'a Value {
    sessions
        .iter()
        .find(|s| s["agent_session_id"] == agent_session_id)
        .unwrap_or_else(|| panic!("{agent_session_id} not listed in {sessions:?}"))
}

fn needs(task: &str) -> ! {
    panic!("{task}: the feature this step exercises does not exist yet; remove the ignore and write the step")
}

#[test]
fn the_hook_path_stores_the_third_kinds_sessions() {
    let fx = Fixture::new();
    fx.seed();

    let one = fx.row("fa-1");
    assert_eq!(one.agent_kind, FAKE_ID);
    assert_eq!(one.state, State::NeedsPermission);
    assert_eq!(one.cwd, "/fake/fa-1");
    assert_eq!(one.task_description.as_deref(), Some("third agent task"));

    let two = fx.row("fa-2");
    assert_eq!(two.state, State::Finished);
    assert_eq!(two.left_off.as_deref(), Some("all finished"));
}

#[test]
fn an_unregistered_kind_is_still_refused_by_the_registry() {
    let fx = Fixture::new();
    fx.hook("nonesuch", &json!({ "fa_event": "begin", "fa_sid": "x" }));
    assert!(fx
        .store
        .get_session_by_agent_id("nonesuch", "x")
        .unwrap()
        .is_none());
    let log = std::fs::read_to_string(fx.state.path().join("hook.log")).unwrap();
    assert!(log.contains("unknown agent kind `nonesuch`"), "{log}");
}

#[test]
fn overview_lists_all_three_kinds_together() {
    let fx = Fixture::new();
    fx.seed();
    let overview = fx.store.overview(&OverviewQuery::default()).unwrap();
    let mut seen: Vec<(String, String, &str)> = overview
        .sections
        .iter()
        .flat_map(|section| {
            section.sessions.iter().map(|entry| {
                (
                    entry.agent_kind.clone(),
                    entry.agent_session_id.clone(),
                    section.name.as_str(),
                )
            })
        })
        .collect();
    seen.sort();
    let expected = [
        ("claude", "cl-1", "active"),
        ("copilot", "co-1", "active"),
        (FAKE_ID, "fa-1", "active"),
        (FAKE_ID, "fa-2", "finished"),
    ];
    let expected: Vec<(String, String, &str)> = expected
        .iter()
        .map(|(k, s, sec)| ((*k).to_owned(), (*s).to_owned(), *sec))
        .collect();
    assert_eq!(seen, expected);
}

#[test]
fn list_json_carries_the_third_kinds_agent_state_and_section() {
    let fx = Fixture::new();
    fx.seed();
    let sessions = fx.list_json(None);
    assert_eq!(sessions.len(), 4);

    let one = find(&sessions, "fa-1");
    assert_eq!(one["agent_kind"], FAKE_ID);
    assert_eq!(one["state"], "needs_permission");
    assert_eq!(one["section"], "active");
    assert_eq!(one["task_description"], "third agent task");
    assert_eq!(one["cwd"], "/fake/fa-1");

    let two = find(&sessions, "fa-2");
    assert_eq!(two["agent_kind"], FAKE_ID);
    assert_eq!(two["state"], "finished");
    assert_eq!(two["section"], "finished");

    assert_eq!(find(&sessions, "cl-1")["agent_kind"], "claude");
    assert_eq!(find(&sessions, "co-1")["agent_kind"], "copilot");
}

#[test]
fn list_filters_by_the_third_kinds_id() {
    let fx = Fixture::new();
    fx.seed();
    let only = fx.list_json(Some(FAKE_ID));
    let ids: Vec<&str> = only
        .iter()
        .map(|s| s["agent_session_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"fa-1") && ids.contains(&"fa-2"), "{ids:?}");
}

#[test]
fn list_text_has_a_line_for_the_third_kind() {
    let fx = Fixture::new();
    fx.seed();
    let command = Command::List {
        states: Vec::new(),
        agent: None,
        repo: None,
        project: None,
        text: None,
        section: None,
        include_dismissed: false,
        expand_finished: false,
    };
    let text = run_in(command, &fx.fake.env, Some(fx.state.path()))
        .unwrap()
        .render(false);
    assert_eq!(text.lines().count(), 4, "{text}");
    let line = text
        .lines()
        .find(|l| l.contains("third agent task"))
        .unwrap();
    assert!(line.contains("needs_permission"), "{line}");
}

#[test]
fn discovery_goes_through_the_shared_store_report_without_downgrading_hook_state() {
    let fx = Fixture::new();
    fx.seed();
    let before = fx.row("fa-1");

    let mut stubs = FakeAgent.discover(&fx.fake.env).unwrap();
    let mut known = stub("fa-1", "/fake/fa-1");
    known.first_prompt = Some("a different first prompt".to_owned());
    known.last_activity_at = "2026-10-11T23:59:00Z".to_owned();
    stubs.push(known);
    let report = ScanReport { stubs, degraded: 0 };

    let summary = store_report(&fx.fake.env, &fx.store, FAKE_ID, &report).unwrap();
    assert_eq!(summary.created, 1);
    assert_eq!(summary.updated, 0);

    let after = fx.row("fa-1");
    assert_eq!(after.state, State::NeedsPermission, "state is hook-derived");
    assert_eq!(after.last_activity_at, before.last_activity_at);
    assert_eq!(after.task_description, before.task_description);

    let sessions = fx.list_json(Some(FAKE_ID));
    assert_eq!(sessions.len(), 3);
    let found = find(&sessions, "fa-found");
    assert_eq!(found["agent_kind"], FAKE_ID);
    assert_eq!(found["state"], "unknown");
    assert_eq!(found["task_description"], "found on disk");
}

#[test]
fn show_works_for_the_third_kind() {
    let fx = Fixture::new();
    fx.seed();
    let id = fx.row("fa-2").id;
    let command = || Command::Show {
        session: id.clone(),
    };

    let output = run_in(command(), &fx.fake.env, Some(fx.state.path())).unwrap();
    let value: Value = serde_json::from_str(&output.render(true)).unwrap();
    assert_eq!(value["agent_kind"], FAKE_ID);
    assert_eq!(value["agent_session_id"], "fa-2");
    assert_eq!(value["state"], "finished");
    assert_eq!(value["section"], "finished");
    assert_eq!(value["left_off"], "all finished");

    let text = run_in(command(), &fx.fake.env, Some(fx.state.path()))
        .unwrap()
        .render(false);
    assert!(text.contains("all finished"), "{text}");
}

#[test]
#[ignore = "T068: respawn exists"]
fn respawn_of_the_third_kind_needs_no_other_change() {
    needs("T068");
}

#[test]
#[ignore = "T097: export exists"]
fn export_of_the_third_kind_needs_no_other_change() {
    needs("T097");
}
