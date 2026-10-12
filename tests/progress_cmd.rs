#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `claude-monitor progress [<id>]` and `progress bind` (T088, FR-015a,
//! contracts/cli.md): the figure with its source, the pick-one list, and the
//! user's choice. Writes to a temporary state dir and a temporary repository.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use claude_monitor::cli::{run_in, Cli, Command, Output};
use claude_monitor::env::Env;
use claude_monitor::error::Error;
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use serde_json::Value;
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";

const TASKS: &str = "\
## Phase 1: Setup
- [x] T001 first
- [ ] T002 second
- [ ] T003 third
";

struct Rig {
    tree: TempDir,
    state: TempDir,
    env: Env,
}

fn rig() -> Rig {
    let state = TempDir::new().unwrap();
    let env = Env::real();
    let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
    let empty = TempDir::new().unwrap();
    legacy::migrate(&store, empty.path(), env.clock.as_ref()).unwrap();
    Rig {
        tree: TempDir::new().unwrap(),
        state,
        env,
    }
}

impl Rig {
    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.tree.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn session(&self, id: &str, branch: &str) {
        let root = self.tree.path().display().to_string();
        let mut session = Session::new(id, "claude", &format!("agent-{id}"), &root, T0);
        session.repo_root = Some(root);
        session.branch = Some(branch.to_owned());
        session.state = State::Idle;
        let store = Store::open_in_dir(self.state.path(), &StoreOptions::viewer()).unwrap();
        store.upsert_session(&session).unwrap();
    }

    fn run(&self, command: Command) -> Result<Output, Error> {
        run_in(command, &self.env, Some(self.state.path()))
    }

    fn view(&self, id: &str) -> Value {
        json(self.run(show(id)))
    }
}

fn show(id: &str) -> Command {
    Command::ProgressBind {
        owner: id.to_owned(),
        file: None,
        source: None,
    }
}

fn bind(id: &str, file: &Path) -> Command {
    Command::ProgressBind {
        owner: id.to_owned(),
        file: Some(file.to_owned()),
        source: None,
    }
}

fn json(result: Result<Output, Error>) -> Value {
    let output = result.unwrap_or_else(|e| panic!("progress failed: {e}"));
    serde_json::from_str(&output.render(true)).unwrap()
}

fn text(result: Result<Output, Error>) -> String {
    result
        .unwrap_or_else(|e| panic!("progress failed: {e}"))
        .render(false)
}

mod parsing {
    use super::*;

    fn command(args: &[&str]) -> Command {
        let mut argv = vec!["claude-monitor"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).unwrap().command()
    }

    #[test]
    fn progress_with_an_id_shows_it() {
        assert_eq!(command(&["progress", "s1"]), show("s1"));
    }

    #[test]
    fn bind_takes_a_path() {
        assert_eq!(
            command(&["progress", "bind", "s1", "todo.md"]),
            Command::ProgressBind {
                owner: "s1".to_owned(),
                file: Some(PathBuf::from("todo.md")),
                source: None,
            }
        );
    }

    #[test]
    fn bind_none_opts_the_session_out() {
        assert_eq!(
            command(&["progress", "bind", "s1", "--none"]),
            Command::ProgressBind {
                owner: "s1".to_owned(),
                file: None,
                source: Some("none".to_owned()),
            }
        );
    }

    #[test]
    fn a_path_and_none_together_are_refused() {
        assert!(Cli::try_parse_from([
            "claude-monitor",
            "progress",
            "bind",
            "s1",
            "a.md",
            "--none"
        ])
        .is_err());
    }
}

#[test]
fn the_figure_names_its_source() {
    let rig = rig();
    rig.write("specs/008-a/tasks.md", TASKS);
    rig.session("s1", "008-a");

    let value = rig.view("s1");

    assert_eq!(value["status"], "bound");
    let progress = &value["progress"];
    assert_eq!(progress["done"], 1);
    assert_eq!(progress["total"], 3);
    assert_eq!(progress["source"], "specs/008-a/tasks.md");
    assert_eq!(progress["source_kind"], "speckit_tasks");
    assert_eq!(progress["current"], "T002 second");
    assert_eq!(progress["chosen_by"], "auto");
    assert_eq!(progress["stale"], false);
    let shown = text(rig.run(show("s1")));
    assert!(shown.contains("1 of 3"), "{shown}");
    assert!(shown.contains("specs/008-a/tasks.md"), "{shown}");
}

#[test]
fn ambiguous_candidates_list_without_a_figure() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    rig.write("todo.md", "- [ ] two\n- [x] three\n");
    rig.session("s1", "main");

    let value = rig.view("s1");

    assert_eq!(value["status"], "pick_one");
    assert!(value.get("progress").is_none(), "no figure when ambiguous");
    let candidates = value["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates[0]["path"]
        .as_str()
        .unwrap()
        .ends_with("PENDING.md"));
    assert_eq!(candidates[0]["total"], 1);
    let shown = text(rig.run(show("s1")));
    assert!(
        shown.contains("PENDING.md") && shown.contains("todo.md"),
        "{shown}"
    );
    assert!(shown.contains("progress bind"), "{shown}");
}

#[test]
fn no_plan_means_no_progress_key_not_zero() {
    let rig = rig();
    rig.write("README.md", "nothing\n");
    rig.session("s1", "main");

    let value = rig.view("s1");

    assert_eq!(value["status"], "none");
    assert!(value.get("progress").is_none());
}

#[test]
fn a_chosen_file_is_remembered_by_the_next_run() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    let todo = rig.write("todo.md", "- [ ] two\n- [x] three\n- [ ] four\n");
    rig.session("s1", "main");
    assert_eq!(rig.view("s1")["status"], "pick_one");

    let bound = json(rig.run(bind("s1", &todo)));

    assert_eq!(bound["status"], "bound");
    let value = rig.view("s1");
    assert_eq!(value["status"], "bound");
    assert_eq!(value["progress"]["chosen_by"], "user");
    assert_eq!(value["progress"]["done"], 1);
    assert_eq!(value["progress"]["total"], 3);
    assert_eq!(value["progress"]["source"], "todo.md");
}

#[test]
fn a_session_id_prefix_is_enough() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    rig.session("session-abcdef", "main");

    assert_eq!(rig.view("session-ab")["session"], "session-abcdef");
}

#[test]
fn none_opts_out_and_stays_out() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    rig.session("s1", "main");
    assert_eq!(rig.view("s1")["status"], "bound");

    let opted_out = json(rig.run(Command::ProgressBind {
        owner: "s1".to_owned(),
        file: None,
        source: Some("none".to_owned()),
    }));

    assert_eq!(opted_out["status"], "none");
    assert_eq!(opted_out["opted_out"], true);
    let value = rig.view("s1");
    assert_eq!(value["status"], "none");
    assert!(value.get("progress").is_none());
}

#[test]
fn a_vanished_file_is_reported_stale_with_its_last_figure() {
    let rig = rig();
    let tasks = rig.write("specs/008-a/tasks.md", TASKS);
    rig.session("s1", "008-a");
    assert_eq!(rig.view("s1")["progress"]["stale"], false);

    fs::remove_file(&tasks).unwrap();
    let value = rig.view("s1");

    assert_eq!(value["status"], "bound");
    assert_eq!(value["progress"]["stale"], true);
    assert_eq!(value["progress"]["done"], 1);
    assert_eq!(value["progress"]["total"], 3);
    let shown = text(rig.run(show("s1")));
    assert!(shown.contains("stale"), "{shown}");
}

#[test]
fn a_file_with_no_tasks_is_refused_and_nothing_is_bound() {
    let rig = rig();
    let readme = rig.write("README.md", "no checkboxes\n");
    rig.session("s1", "main");

    let error = rig.run(bind("s1", &readme)).unwrap_err();

    assert!(matches!(error, Error::Refused { .. }), "{error}");
    assert_eq!(error.exit_code(), 2);
    assert_eq!(rig.view("s1")["status"], "none");
}

#[test]
fn an_unknown_session_is_refused() {
    let rig = rig();

    let error = rig.run(show("nope")).unwrap_err();

    assert!(
        matches!(
            error,
            Error::Refused {
                code: "unknown_session",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_path_and_a_source_together_are_a_usage_error() {
    let rig = rig();
    rig.session("s1", "main");

    let error = rig
        .run(Command::ProgressBind {
            owner: "s1".to_owned(),
            file: Some(PathBuf::from("todo.md")),
            source: Some("none".to_owned()),
        })
        .unwrap_err();

    assert!(matches!(error, Error::Usage(_)), "{error}");
}

#[test]
fn an_unknown_source_is_a_usage_error() {
    let rig = rig();
    rig.session("s1", "main");

    let error = rig
        .run(Command::ProgressBind {
            owner: "s1".to_owned(),
            file: None,
            source: Some("telepathy".to_owned()),
        })
        .unwrap_err();

    assert!(matches!(error, Error::Usage(_)), "{error}");
}
