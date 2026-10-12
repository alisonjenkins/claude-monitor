//! `list` and `show` through `cli::run_in` (`contracts/cli.md`, FR-001..FR-003,
//! FR-019, FR-021).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use claude_monitor::cli::{run_in, Command, Output};
use claude_monitor::error::Error;
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::{json, Value};
use tempfile::TempDir;

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
        // Record the legacy-import marker against an empty directory so the
        // viewer's first-start import never reads the real runtime directory.
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
        Self { state, fake, store }
    }

    fn put(&self, id: &str, state: State, last_activity_at: &str, edit: impl FnOnce(&mut Session)) {
        let mut session = Session::new(
            id,
            "claude",
            &format!("agent-{id}"),
            &format!("/work/{id}"),
            "2026-10-01T00:00:00Z",
        );
        session.state = state;
        session.last_activity_at = last_activity_at.to_owned();
        edit(&mut session);
        self.store.upsert_session(&session).unwrap();
    }

    fn run(&self, command: Command) -> Result<Output, Error> {
        run_in(command, &self.fake.env, Some(self.state.path()))
    }

    fn json(&self, command: Command) -> Value {
        let output = self.run(command).unwrap();
        serde_json::from_str(&output.render(true)).unwrap()
    }

    fn ids(&self, command: Command) -> Vec<String> {
        let value = self.json(command);
        value["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect()
    }
}

#[derive(Default)]
struct List {
    states: Vec<&'static str>,
    agent: Option<&'static str>,
    repo: Option<&'static str>,
    text: Option<&'static str>,
    section: Option<&'static str>,
    include_dismissed: bool,
    expand_finished: bool,
}

impl List {
    fn command(self) -> Command {
        Command::List {
            states: self.states.into_iter().map(str::to_owned).collect(),
            agent: self.agent.map(str::to_owned),
            repo: self.repo.map(str::to_owned),
            project: None,
            text: self.text.map(str::to_owned),
            section: self.section.map(str::to_owned),
            include_dismissed: self.include_dismissed,
            expand_finished: self.expand_finished,
        }
    }
}

fn show(id: &str) -> Command {
    Command::Show {
        session: id.to_owned(),
    }
}

fn seed(fx: &Fixture) {
    fx.put("a", State::Working, "2026-10-11T22:00:00Z", |s| {
        s.repo_root = Some("/home/u/git/nix-config".into());
        s.branch = Some("feature-a".into());
        s.task_description = Some("write the overview queries".into());
        s.left_off = Some("tests are red".into());
    });
    fx.put("b", State::NeedsPermission, "2026-10-11T23:30:00Z", |s| {
        s.repo_root = Some("/home/u/git/claude-monitor".into());
        s.branch = Some("main".into());
        s.worktree_exists = false;
    });
    fx.put("c", State::Interrupted, "2026-10-11T12:00:00Z", |_| {});
    fx.put("d", State::Finished, "2026-10-10T00:00:00Z", |s| {
        s.agent_kind = "copilot".into();
    });
}

#[test]
fn json_has_the_documented_shape() {
    let fx = Fixture::new();
    seed(&fx);
    let value = fx.json(List::default().command());

    let object = value.as_object().unwrap();
    assert_eq!(object.keys().collect::<Vec<_>>(), ["sessions"]);
    let sessions = value["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 4);

    let first = &sessions[0];
    assert_eq!(first["id"], "b", "needs_permission first");
    let mut keys: Vec<_> = first.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent_kind",
            "agent_session_id",
            "branch",
            "cwd",
            "flags",
            "id",
            "last_activity_at",
            "left_off",
            "project",
            "repo",
            "running",
            "section",
            "state",
            "task_description",
        ]
    );
    assert_eq!(first["agent_kind"], "claude");
    assert_eq!(first["agent_session_id"], "agent-b");
    assert_eq!(first["repo"], "claude-monitor");
    assert_eq!(first["branch"], "main");
    assert_eq!(first["cwd"], "/work/b");
    assert_eq!(first["state"], "needs_permission");
    assert_eq!(first["running"], true);
    assert_eq!(first["last_activity_at"], "2026-10-11T23:30:00Z");
    assert_eq!(first["section"], "active");
    assert_eq!(first["flags"], json!(["worktree_missing"]));
    assert!(first["project"].is_null());
    assert!(first.get("progress").is_none(), "progress is omitted");

    let sections: Vec<_> = sessions
        .iter()
        .map(|s| s["section"].as_str().unwrap())
        .collect();
    assert_eq!(sections, ["active", "active", "interrupted", "finished"]);
}

#[test]
fn text_is_one_line_per_session_with_state_age_repo_branch_and_task() {
    let fx = Fixture::new();
    seed(&fx);
    fx.put("long", State::Idle, "2026-10-11T23:59:30Z", |s| {
        s.task_description = Some("x".repeat(200));
    });
    let text = fx.run(List::default().command()).unwrap().render(false);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 5);

    let a = lines.iter().find(|l| l.contains("feature-a")).unwrap();
    assert!(a.contains("working"), "{a}");
    assert!(a.contains("2h"), "{a}");
    assert!(a.contains("nix-config/feature-a"), "{a}");
    assert!(a.contains("write the overview queries"), "{a}");

    let long = lines.iter().find(|l| l.contains("idle")).unwrap();
    assert!(long.contains("30s"), "{long}");
    assert!(!long.contains(&"x".repeat(200)), "task text is shortened");
    assert!(long.contains(&"x".repeat(20)), "{long}");
}

#[test]
fn state_filter_is_repeatable() {
    let fx = Fixture::new();
    seed(&fx);
    let list = List {
        states: vec!["working", "interrupted"],
        ..List::default()
    };
    assert_eq!(fx.ids(list.command()), ["a", "c"]);
}

#[test]
fn agent_repo_and_text_filters_narrow_the_list() {
    let fx = Fixture::new();
    seed(&fx);
    let by_agent = List {
        agent: Some("copilot"),
        ..List::default()
    };
    assert_eq!(fx.ids(by_agent.command()), ["d"]);
    let by_repo = List {
        repo: Some("nix-config"),
        ..List::default()
    };
    assert_eq!(fx.ids(by_repo.command()), ["a"]);
    let by_text = List {
        text: Some("OVERVIEW"),
        ..List::default()
    };
    assert_eq!(fx.ids(by_text.command()), ["a"]);
}

#[test]
fn section_flag_restricts_to_one_section() {
    let fx = Fixture::new();
    seed(&fx);
    let list = List {
        section: Some("interrupted"),
        ..List::default()
    };
    assert_eq!(fx.ids(list.command()), ["c"]);
}

#[test]
fn dismissed_sessions_need_include_dismissed() {
    let fx = Fixture::new();
    seed(&fx);
    fx.put("gone", State::Idle, "2026-10-11T20:00:00Z", |s| {
        s.dismissed = true;
    });
    assert!(!fx
        .ids(List::default().command())
        .contains(&"gone".to_owned()));
    let list = List {
        include_dismissed: true,
        ..List::default()
    };
    assert!(fx.ids(list.command()).contains(&"gone".to_owned()));
}

#[test]
fn old_finished_sessions_need_expand_finished() {
    let fx = Fixture::new();
    fx.put("recent", State::Finished, "2026-10-10T00:00:00Z", |_| {});
    fx.put("stale", State::Finished, "2026-09-01T00:00:00Z", |_| {});
    assert_eq!(fx.ids(List::default().command()), ["recent"]);
    let list = List {
        expand_finished: true,
        ..List::default()
    };
    assert_eq!(fx.ids(list.command()), ["recent", "stale"]);
}

#[test]
fn unknown_state_or_section_is_a_usage_error() {
    let fx = Fixture::new();
    let bad_state = List {
        states: vec!["sleeping"],
        ..List::default()
    };
    let err = fx.run(bad_state.command()).unwrap_err();
    assert!(matches!(err, Error::Usage(_)), "{err}");
    assert_eq!(err.exit_code(), 1);
    let bad_section = List {
        section: Some("elsewhere"),
        ..List::default()
    };
    let err = fx.run(bad_section.command()).unwrap_err();
    assert!(matches!(err, Error::Usage(_)), "{err}");
}

#[test]
fn an_empty_store_lists_nothing_and_succeeds() {
    let fx = Fixture::new();
    let output = fx.run(List::default().command()).unwrap();
    assert_eq!(output.render(true), "{\"sessions\":[]}\n");
    assert_eq!(output.render(false), "");
}

#[test]
fn show_returns_the_detail_with_left_off_and_lineage() {
    let fx = Fixture::new();
    seed(&fx);
    fx.put("successor", State::Idle, "2026-10-11T23:00:00Z", |s| {
        s.resumed_from = Some("a".into());
    });
    fx.put("fork", State::Idle, "2026-10-11T23:00:00Z", |s| {
        s.forked_from = Some("a".into());
    });

    let parent = fx.json(show("a"));
    assert_eq!(parent["id"], "a");
    assert_eq!(parent["state"], "working");
    assert_eq!(parent["left_off"], "tests are red");
    assert_eq!(parent["task_description"], "write the overview queries");
    assert_eq!(parent["repo"], "nix-config");
    assert_eq!(parent["section"], "active");
    assert!(parent["resumed_from"].is_null());
    assert!(parent["forked_from"].is_null());
    assert_eq!(parent["resumed_by"], json!(["successor"]));

    let child = fx.json(show("successor"));
    assert_eq!(child["resumed_from"], "a");
    let fork = fx.json(show("fork"));
    assert_eq!(fork["forked_from"], "a");

    let text = fx.run(show("a")).unwrap().render(false);
    assert!(text.contains("tests are red"), "{text}");
    assert!(text.contains("successor"), "{text}");
}

const HOSTILE: &str = "ok \u{1b}]52;c;aGk=\u{7}\u{1b}[2Jdone \u{9b}31m";

fn assert_inert(text: &str) {
    for c in text.chars() {
        assert!(
            !c.is_control() || c == '\n' || c == '\t',
            "control character {c:?} reached the terminal text: {text:?}"
        );
    }
}

#[test]
fn list_text_strips_terminal_escapes_from_the_task() {
    let fx = Fixture::new();
    fx.put("evil", State::Idle, "2026-10-11T23:59:30Z", |s| {
        s.task_description = Some(HOSTILE.to_owned());
    });
    let text = fx.run(List::default().command()).unwrap().render(false);
    assert_inert(&text);
    assert!(text.contains("ok "), "{text}");
    assert!(text.contains("done"), "{text}");
}

#[test]
fn show_text_strips_terminal_escapes_but_keeps_multiline_layout() {
    let fx = Fixture::new();
    fx.put("evil", State::Idle, "2026-10-11T23:59:30Z", |s| {
        s.task_description = Some(HOSTILE.to_owned());
        s.left_off = Some(format!("first\n\t{HOSTILE}"));
    });
    let text = fx.run(show("evil")).unwrap().render(false);
    assert_inert(&text);
    assert!(text.contains("first\n\t"), "{text}");

    let json = fx.run(show("evil")).unwrap().render(true);
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["task_description"], HOSTILE, "--json stays verbatim");
}

#[test]
fn show_of_an_unknown_id_is_refused_with_exit_2() {
    let fx = Fixture::new();
    seed(&fx);
    let err = fx.run(show("nope")).unwrap_err();
    assert!(
        matches!(&err, Error::Refused { code, .. } if *code == "unknown_session"),
        "{err}"
    );
    assert_eq!(err.exit_code(), 2);
    let value: Value = serde_json::from_str(&err.render(true)).unwrap();
    assert_eq!(value["ok"], false);
    assert!(value["error"].as_str().unwrap().contains("nope"));
    assert_eq!(value.as_object().unwrap().len(), 2);
}
