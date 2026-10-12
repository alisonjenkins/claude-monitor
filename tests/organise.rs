//! Organising sessions (US5, FR-018, FR-019, FR-019a to FR-019c, quickstart
//! S6): projects, notes, park and dismiss markers, and the Done sections.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use std::collections::BTreeSet;

use claude_monitor::cli::{run_in, Command, Output, ProjectAction};
use claude_monitor::error::Error;
use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::{Author, Session, State};
use claude_monitor::store::events::{EventRow, NewEvent};
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
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
        Self { state, fake, store }
    }

    fn put(&self, id: &str, state: State, last_activity_at: &str) {
        let mut session = Session::new(
            id,
            "claude",
            &format!("agent-{id}"),
            &format!("/work/{id}"),
            "2026-07-01T00:00:00Z",
        );
        session.state = state;
        session.last_activity_at = last_activity_at.to_owned();
        session.task_description = Some(format!("task of {id}"));
        session.left_off = Some(format!("left off in {id}"));
        self.store.upsert_session(&session).unwrap();
    }

    fn run(&self, command: Command) -> Result<Output, Error> {
        run_in(command, &self.fake.env, Some(self.state.path()))
    }

    fn json(&self, command: Command) -> Value {
        let output = self.run(command).unwrap();
        serde_json::from_str(&output.render(true)).unwrap()
    }

    fn refused(&self, command: Command) -> &'static str {
        match self.run(command) {
            Err(Error::Refused { code, .. }) => code,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn list(&self, project: Option<&str>, section: Option<&str>, dismissed: bool) -> Value {
        self.json(Command::List {
            states: vec![],
            agent: None,
            repo: None,
            project: project.map(str::to_owned),
            text: None,
            section: section.map(str::to_owned),
            include_dismissed: dismissed,
            expand_finished: false,
            expand_unseen: true,
        })
    }

    fn ids(&self, project: Option<&str>, section: Option<&str>, dismissed: bool) -> Vec<String> {
        let mut ids: Vec<String> = self.list(project, section, dismissed)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    }

    fn entry(&self, id: &str) -> Value {
        self.list(None, None, true)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap_or_else(|| panic!("{id} is not listed"))
            .clone()
    }

    fn events(&self, kind: &str) -> Vec<EventRow> {
        self.store
            .events_since(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
            .collect()
    }
}

fn group(project: &str, sessions: &[&str]) -> Command {
    Command::Group {
        project: project.to_owned(),
        sessions: sessions.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn project(action: ProjectAction, name: &str, value: Option<&str>) -> Command {
    Command::Project {
        action,
        name: name.to_owned(),
        value: value.map(str::to_owned),
    }
}

fn note(target: &str, text: &str) -> Command {
    Command::Note {
        target: target.to_owned(),
        text: text.to_owned(),
    }
}

fn target(make: fn(String) -> Command, target: &str) -> Command {
    make(target.to_owned())
}

fn done(sessions: &[&str]) -> Command {
    Command::Done {
        sessions: sessions.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn undone(sessions: &[&str]) -> Command {
    Command::Undone {
        sessions: sessions.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn is_user(event: &EventRow) -> bool {
    matches!(event.author, Author::User { .. })
}

const RECENT: &str = "2026-10-11T22:00:00Z";

mod projects {
    use super::*;

    #[test]
    fn group_puts_three_sessions_in_a_project() {
        let fx = Fixture::new();
        for id in ["s1", "s2", "s3", "s4"] {
            fx.put(id, State::Idle, RECENT);
        }

        fx.json(group("alpha", &["s1", "s2", "s3"]));

        assert_eq!(fx.ids(Some("alpha"), None, false), ["s1", "s2", "s3"]);
        assert_eq!(fx.ids(Some("-"), None, false), ["s4"]);
        assert_eq!(fx.entry("s2")["project"], "alpha");
        let grouped = fx.events("session.grouped");
        assert_eq!(grouped.len(), 3);
        assert!(grouped.iter().all(is_user));
        assert_eq!(grouped[0].payload["project"], "alpha");
        assert_eq!(fx.events("project.created").len(), 1);
    }

    #[test]
    fn grouping_again_changes_nothing_and_records_nothing() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);
        fx.json(group("alpha", &["s1"]));
        let before = fx.store.last_seq().unwrap();

        fx.json(group("alpha", &["s1"]));

        assert_eq!(fx.store.last_seq().unwrap(), before);
    }

    #[test]
    fn a_dash_unassigns_the_sessions() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);
        fx.put("s2", State::Idle, RECENT);
        fx.json(group("alpha", &["s1", "s2"]));

        fx.json(group("-", &["s1"]));

        assert_eq!(fx.ids(Some("alpha"), None, false), ["s2"]);
        assert_eq!(fx.entry("s1")["project"], Value::Null);
        let moves = fx.events("session.grouped");
        assert_eq!(moves.last().unwrap().payload["project"], Value::Null);
    }

    #[test]
    fn an_unknown_session_refuses_the_whole_group_and_changes_nothing() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);

        let code = fx.refused(group("alpha", &["s1", "nope"]));

        assert_eq!(code, "unknown_session");
        assert_eq!(fx.entry("s1")["project"], Value::Null);
        assert!(fx.events("session.grouped").is_empty());
    }

    #[test]
    fn add_rename_and_set_status_record_user_events() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);
        fx.json(project(ProjectAction::Add, "alpha", None));
        fx.json(group("alpha", &["s1"]));

        fx.json(project(ProjectAction::Rename, "alpha", Some("beta")));
        fx.json(project(ProjectAction::SetStatus, "beta", Some("parked")));

        assert_eq!(fx.ids(Some("beta"), None, false), ["s1"]);
        assert!(fx.ids(Some("alpha"), None, false).is_empty());
        let status: String = fx
            .store
            .connection()
            .query_row("SELECT status FROM projects WHERE name = 'beta'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "parked");
        for kind in ["project.created", "project.renamed", "project.status_set"] {
            let events = fx.events(kind);
            assert_eq!(events.len(), 1, "{kind}");
            assert!(is_user(&events[0]), "{kind}");
        }
    }

    #[test]
    fn project_mistakes_are_refused_or_usage_errors() {
        let fx = Fixture::new();
        fx.json(project(ProjectAction::Add, "alpha", None));

        assert_eq!(
            fx.refused(project(ProjectAction::Add, "alpha", None)),
            "project_exists"
        );
        assert_eq!(
            fx.refused(project(ProjectAction::Rename, "ghost", Some("x"))),
            "unknown_project"
        );
        assert!(matches!(
            fx.run(project(ProjectAction::SetStatus, "alpha", Some("bogus"))),
            Err(Error::Usage(_))
        ));
        assert!(matches!(
            fx.run(project(ProjectAction::Rename, "alpha", None)),
            Err(Error::Usage(_))
        ));
    }
}

mod markers {
    use super::*;

    #[test]
    fn a_note_is_stored_and_recorded_as_a_user_event() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);
        fx.json(group("alpha", &["s1"]));

        fx.json(note("s1", "the flaky test is in tests/store.rs"));
        fx.json(note("alpha", "ship by Friday"));

        let events = fx.events("note.added");
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(is_user));
        assert_eq!(events[0].entity_kind, "session");
        assert_eq!(events[0].entity_id, "s1");
        assert_eq!(events[1].entity_kind, "project");
        let shown = fx.json(Command::Show {
            session: "s1".to_owned(),
        });
        assert_eq!(
            shown["notes"][0]["body"],
            "the flaky test is in tests/store.rs"
        );
        assert_eq!(shown["notes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn an_empty_note_or_unknown_target_is_rejected() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);

        assert!(matches!(fx.run(note("s1", "   ")), Err(Error::Usage(_))));
        assert_eq!(fx.refused(note("nothing", "x")), "unknown_target");
        assert!(fx.events("note.added").is_empty());
    }

    #[test]
    fn dismiss_hides_by_default_and_include_dismissed_returns_it_with_history() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);
        fx.put("s2", State::Interrupted, RECENT);
        let before = json!({ "why": "history" });
        fx.store
            .append_event(&NewEvent {
                at: RECENT,
                entity_kind: "session",
                entity_id: "s2",
                kind: "test.history",
                payload: &before,
                author: &Author::System {
                    component: "test".to_owned(),
                },
            })
            .unwrap();

        fx.json(target(|target| Command::Dismiss { target }, "s2"));

        assert_eq!(fx.ids(None, None, false), ["s1"]);
        assert_eq!(fx.ids(None, None, true), ["s1", "s2"]);
        assert_eq!(fx.ids(None, Some("dismissed"), false), ["s2"]);
        let entry = fx.entry("s2");
        assert_eq!(entry["section"], "dismissed");
        assert!(entry["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("dismissed")));
        assert_eq!(entry["task_description"], "task of s2");
        assert_eq!(entry["left_off"], "left off in s2");
        assert_eq!(fx.events("test.history").len(), 1);
        let dismissed = fx.events("session.dismissed");
        assert_eq!(dismissed.len(), 1);
        assert!(is_user(&dismissed[0]));

        fx.json(target(|target| Command::Undismiss { target }, "s2"));

        assert_eq!(fx.ids(None, None, false), ["s1", "s2"]);
        assert_eq!(fx.entry("s2")["section"], "interrupted");
        assert_eq!(fx.events("session.undismissed").len(), 1);
    }

    #[test]
    fn dismissing_twice_records_one_event() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);

        fx.json(target(|target| Command::Dismiss { target }, "s1"));
        fx.json(target(|target| Command::Dismiss { target }, "s1"));

        assert_eq!(fx.events("session.dismissed").len(), 1);
    }

    #[test]
    fn park_flags_a_session_without_moving_or_hiding_it() {
        let fx = Fixture::new();
        fx.put("s1", State::Working, RECENT);

        fx.json(target(|target| Command::Park { target }, "s1"));

        let entry = fx.entry("s1");
        assert_eq!(entry["section"], "active");
        assert!(entry["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("parked")));
        assert_eq!(fx.ids(None, None, false), ["s1"]);
        assert_eq!(fx.events("session.parked").len(), 1);

        fx.json(target(|target| Command::Unpark { target }, "s1"));

        assert!(!fx.entry("s1")["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("parked")));
        assert_eq!(fx.events("session.unparked").len(), 1);
    }

    #[test]
    fn park_and_dismiss_on_a_project_change_its_status_and_never_delete() {
        let fx = Fixture::new();
        fx.put("s1", State::Idle, RECENT);
        fx.json(group("alpha", &["s1"]));
        let status = |fx: &Fixture| -> String {
            fx.store
                .connection()
                .query_row(
                    "SELECT status FROM projects WHERE name = 'alpha'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };

        fx.json(target(|target| Command::Park { target }, "alpha"));
        assert_eq!(status(&fx), "parked");
        fx.json(target(|target| Command::Unpark { target }, "alpha"));
        assert_eq!(status(&fx), "active");
        fx.json(target(|target| Command::Dismiss { target }, "alpha"));
        assert_eq!(status(&fx), "archived");
        fx.json(target(|target| Command::Undismiss { target }, "alpha"));
        assert_eq!(status(&fx), "active");
        assert_eq!(fx.ids(Some("alpha"), None, false), ["s1"]);
    }

    #[test]
    fn an_unknown_marker_target_is_refused() {
        let fx = Fixture::new();

        assert_eq!(
            fx.refused(target(|target| Command::Park { target }, "ghost")),
            "unknown_target"
        );
    }
}

mod done_sections {
    use super::*;

    fn section_ids(fx: &Fixture, project: Option<&str>, section: &str) -> Vec<String> {
        fx.ids(project, Some(section), false)
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn each_session_is_in_exactly_one_group_and_done_is_per_project() {
        let fx = Fixture::new();
        fx.put("in-p", State::Finished, RECENT);
        fx.put("loose", State::Finished, RECENT);
        fx.put("still-finished", State::Finished, RECENT);
        fx.put("running", State::Working, RECENT);
        fx.json(group("p", &["in-p", "running"]));

        fx.json(done(&["in-p", "loose"]));

        assert_eq!(section_ids(&fx, Some("p"), "done"), ["in-p"]);
        assert_eq!(section_ids(&fx, Some("-"), "done"), ["loose"]);
        assert_eq!(section_ids(&fx, None, "done"), ["in-p", "loose"]);
        assert_eq!(section_ids(&fx, None, "finished"), ["still-finished"]);
        let sections = ["active", "interrupted", "finished", "done", "unseen"];
        let mut seen = BTreeSet::new();
        for section in sections {
            for id in section_ids(&fx, None, section) {
                assert!(seen.insert(id.clone()), "{id} is in two groups");
            }
        }
        assert_eq!(seen.len(), 4);
        let entry = fx.entry("in-p");
        assert_eq!(entry["section"], "done");
        assert_eq!(entry["task_description"], "task of in-p");
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn done_is_rejected_for_a_session_that_is_not_finished() {
        let fx = Fixture::new();
        fx.put("run", State::Working, RECENT);
        fx.put("int", State::Interrupted, RECENT);

        for id in ["run", "int"] {
            assert_eq!(fx.refused(done(&[id])), "not_finished", "{id}");
        }

        assert!(fx.events("session.done").is_empty());
        assert_eq!(fx.entry("int")["section"], "interrupted");
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn a_batch_with_one_unfinished_session_marks_nothing() {
        let fx = Fixture::new();
        fx.put("fin", State::Finished, RECENT);
        fx.put("run", State::Working, RECENT);

        assert_eq!(fx.refused(done(&["fin", "run"])), "not_finished");

        assert_eq!(fx.entry("fin")["section"], "finished");
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn undone_returns_the_session_to_finished() {
        let fx = Fixture::new();
        fx.put("fin", State::Finished, RECENT);
        fx.json(done(&["fin"]));

        fx.json(undone(&["fin"]));

        assert_eq!(fx.entry("fin")["section"], "finished");
        let events = fx.events("session.undone");
        assert_eq!(events.len(), 1);
        assert!(is_user(&events[0]));
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn done_and_undone_are_idempotent() {
        let fx = Fixture::new();
        fx.put("fin", State::Finished, RECENT);

        fx.json(done(&["fin"]));
        fx.json(done(&["fin"]));
        fx.json(undone(&["fin"]));
        fx.json(undone(&["fin"]));

        assert_eq!(fx.events("session.done").len(), 1);
        assert_eq!(fx.events("session.undone").len(), 1);
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn every_done_session_has_a_user_authored_done_event() {
        let fx = Fixture::new();
        fx.put("a", State::Finished, RECENT);
        fx.put("b", State::Finished, RECENT);
        fx.put("c", State::Finished, RECENT);
        fx.json(done(&["a", "b"]));

        let done_ids = section_ids(&fx, None, "done");

        assert_eq!(done_ids, ["a", "b"]);
        let authored: BTreeSet<String> = fx
            .events("session.done")
            .into_iter()
            .filter(is_user)
            .map(|e| e.entity_id)
            .collect();
        assert_eq!(authored, done_ids.into_iter().collect());
        assert_eq!(fx.entry("c")["section"], "finished");
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn the_system_never_marks_a_session_done() {
        let fx = Fixture::new();
        fx.put("fin", State::Finished, "2026-01-01T00:00:00Z");
        fx.put("run", State::Idle, RECENT);

        let sections = fx.list(None, None, true);

        assert!(fx.ids(None, Some("done"), true).is_empty());
        assert_eq!(sections["sessions"].as_array().unwrap().len(), 2);
        assert!(fx.events("session.done").is_empty());
    }

    #[test]
    #[ignore = "implemented by T110"]
    fn a_session_start_hook_clears_done_and_puts_the_work_in_active() {
        let fx = Fixture::new();
        fx.put("fin", State::Finished, RECENT);
        fx.json(done(&["fin"]));
        assert_eq!(fx.entry("fin")["section"], "done");

        let input = json!({
            "session_id": "agent-fin",
            "hook_event_name": "SessionStart",
            "source": "resume",
            "cwd": "/work/fin",
        })
        .to_string();
        ingest(
            &HookContext {
                agent: "claude",
                state_dir: fx.state.path(),
                tmux_pane: None,
                clock: fx.fake.env.clock.as_ref(),
                git: &NoGit,
                agent_pid: None,
                env: None,
            },
            &input,
        );

        let entry = fx.entry("fin");
        assert_eq!(entry["section"], "active");
        let session = fx.store.get_session("fin").unwrap().unwrap();
        assert_eq!(session.done_at, None);
    }
}

mod finished_visible {
    use super::*;

    const NINETY_DAYS_AGO: &str = "2026-07-14T00:00:00Z";

    #[test]
    fn a_ninety_day_old_finished_session_is_listed_by_default() {
        let fx = Fixture::new();
        fx.put("old", State::Finished, NINETY_DAYS_AGO);
        fx.put("new", State::Finished, RECENT);

        assert_eq!(fx.ids(None, None, false), ["new", "old"]);
        assert_eq!(fx.ids(None, Some("finished"), false), ["new", "old"]);
        assert_eq!(fx.entry("old")["section"], "finished");
    }

    fn would_start(fx: &Fixture, include_finished: bool) -> BTreeSet<String> {
        let output = fx
            .run(Command::Restore {
                all_interrupted: true,
                include_finished,
                fresh: false,
                skip: vec![],
                yes: false,
            })
            .unwrap();
        let value: Value = serde_json::from_str(&output.render(true)).unwrap();
        let mut ids = BTreeSet::new();
        for key in ["would_start", "would_refuse"] {
            for item in value[key].as_array().unwrap() {
                ids.insert(item["session"].as_str().unwrap().to_owned());
            }
        }
        ids
    }

    #[test]
    fn restore_does_not_select_finished_sessions_unless_asked() {
        let fx = Fixture::new();
        fx.put("fin", State::Finished, NINETY_DAYS_AGO);
        fx.put("int", State::Interrupted, RECENT);

        assert_eq!(would_start(&fx, false), BTreeSet::from(["int".to_owned()]));
        assert_eq!(
            would_start(&fx, true),
            BTreeSet::from(["fin".to_owned(), "int".to_owned()])
        );
    }

    #[test]
    fn respawning_a_finished_session_returns_it_to_active() {
        let Some(fx) = common::Fixture::new() else {
            return;
        };
        fx.add_session("fin", State::Finished, "w-fin");

        let output = run_in(
            Command::Respawn {
                sessions: vec!["fin".to_owned()],
                fresh: false,
                in_dir: None,
                recreate_worktree: false,
            },
            &fx.env,
            Some(&fx.state_dir),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&output.render(true)).unwrap();
        assert_eq!(value["started"].as_array().unwrap().len(), 1, "{value}");
        assert_eq!(fx.respawn_events().len(), 1);

        let input = json!({
            "session_id": "agent-fin",
            "hook_event_name": "SessionStart",
            "source": "resume",
            "cwd": fx.root().join("w-fin"),
        })
        .to_string();
        ingest(
            &HookContext {
                agent: "claude",
                state_dir: &fx.state_dir,
                tmux_pane: None,
                clock: fx.env.clock.as_ref(),
                git: &NoGit,
                agent_pid: None,
                env: None,
            },
            &input,
        );

        let session = fx.store().get_session("fin").unwrap().unwrap();
        assert_eq!(session.state, State::Idle);
    }
}
