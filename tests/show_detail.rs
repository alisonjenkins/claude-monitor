#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `show <id>` and `show --json` carry the plan progress with its source and
//! both kinds of summary (T093, FR-015a, FR-017, FR-017a).

use std::fs;
use std::path::PathBuf;

use claude_monitor::cli::{run_in, Command, Output};
use claude_monitor::env::Env;
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use serde_json::Value;
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T11:00:00Z";

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
    fn store(&self) -> Store {
        Store::open_in_dir(self.state.path(), &StoreOptions::viewer()).unwrap()
    }

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
        session.task_description = Some("fix the build".to_owned());
        session.left_off = Some("tests are green".to_owned());
        self.store().upsert_session(&session).unwrap();
    }

    fn run(&self, command: Command) -> Output {
        run_in(command, &self.env, Some(self.state.path())).unwrap()
    }

    fn bind(&self, id: &str, file: &std::path::Path) {
        self.run(Command::ProgressBind {
            owner: id.to_owned(),
            file: Some(file.to_owned()),
            source: None,
        });
    }

    fn show_json(&self, id: &str) -> Value {
        let out = self.run(Command::Show {
            session: id.to_owned(),
        });
        serde_json::from_str(&out.render(true)).unwrap()
    }

    fn show_text(&self, id: &str) -> String {
        self.run(Command::Show {
            session: id.to_owned(),
        })
        .render(false)
    }
}

fn bound_rig() -> Rig {
    let rig = rig();
    rig.session("s1", "main");
    let tasks = rig.write("specs/001-x/tasks.md", TASKS);
    rig.bind("s1", &tasks);
    rig
}

#[test]
fn json_has_the_bound_progress_exactly_as_the_progress_command_emits_it() {
    let rig = bound_rig();
    let shown = rig.show_json("s1");
    let from_progress: Value = serde_json::from_str(
        &rig.run(Command::ProgressBind {
            owner: "s1".to_owned(),
            file: None,
            source: None,
        })
        .render(true),
    )
    .unwrap();
    assert_eq!(shown["progress_status"], "bound");
    assert_eq!(shown["progress"], from_progress["progress"]);
    assert_eq!(shown["progress"]["done"], 1);
    assert_eq!(shown["progress"]["total"], 3);
    assert_eq!(
        shown["progress"]["source_label"],
        "specs/001-x/tasks.md (spec-kit tasks)"
    );
    assert!(shown.get("candidates").is_none());
}

#[test]
fn show_names_the_recorded_tmux_session_and_window() {
    let rig = rig();
    rig.session("s1", "main");
    let mut session = rig.store().get_session("s1").unwrap().unwrap();
    session.tmux_session = Some("my project".to_owned());
    session.tmux_window_name = Some("api build".to_owned());
    session.tmux_window_index = Some("3".to_owned());
    rig.store().upsert_session(&session).unwrap();

    let json = rig.show_json("s1");
    let text = rig.show_text("s1");

    assert_eq!(json["tmux_session"], "my project");
    assert_eq!(json["tmux_window_name"], "api build");
    assert_eq!(json["tmux_window_index"], "3");
    assert!(
        text.lines().any(|l| l.starts_with("tmux")
            && l.contains("my project:api build")
            && l.contains('3')),
        "{text}"
    );
}

#[test]
fn show_names_the_recorded_pane_and_the_panes_in_its_window() {
    let rig = rig();
    rig.session("s1", "main");
    let mut session = rig.store().get_session("s1").unwrap().unwrap();
    session.tmux_session = Some("work".to_owned());
    session.tmux_window_name = Some("api".to_owned());
    session.tmux_window_index = Some("3".to_owned());
    session.tmux_pane_index = Some("2".to_owned());
    session.tmux_window_panes = Some("4".to_owned());
    session.tmux_window_layout = Some("b25d,200x50,0,0{100x50,0,0,0,99x50,101,0,1}".to_owned());
    rig.store().upsert_session(&session).unwrap();

    let json = rig.show_json("s1");
    let text = rig.show_text("s1");

    assert_eq!(json["tmux_pane_index"], "2");
    assert_eq!(json["tmux_window_panes"], "4");
    assert!(
        text.lines()
            .any(|l| l.starts_with("tmux") && l.contains("work:api (window 3, pane 2 of 4)")),
        "{text}"
    );
}

#[test]
fn show_names_the_session_when_the_user_gave_it_a_name() {
    let rig = rig();
    rig.session("s1", "main");
    rig.session("s2", "main");
    let mut session = rig.store().get_session("s1").unwrap().unwrap();
    session.session_name = Some("aquero".to_owned());
    rig.store().upsert_session(&session).unwrap();

    let named = rig.show_text("s1");
    let unnamed = rig.show_text("s2");

    assert!(
        named
            .lines()
            .any(|l| l.starts_with("name") && l.ends_with("aquero")),
        "{named}"
    );
    assert!(!unnamed.lines().any(|l| l.starts_with("name")), "{unnamed}");
    assert_eq!(rig.show_json("s1")["session_name"], "aquero");
    assert!(rig.show_json("s2")["session_name"].is_null());
}

#[test]
fn json_keeps_verbatim_and_generated_summaries_apart() {
    let rig = bound_rig();
    rig.store()
        .insert_model_summary("session", "s1", "Fixing CI flakes.", "qwen-9b", T1)
        .unwrap();
    let summaries = rig.show_json("s1")["summaries"].clone();
    assert_eq!(summaries["task_description"], "fix the build");
    assert_eq!(summaries["left_off"], "tests are green");
    let generated = summaries["generated"].as_array().unwrap();
    assert_eq!(generated.len(), 1);
    assert_eq!(generated[0]["text"], "Fixing CI flakes.");
    assert_eq!(generated[0]["model"], "qwen-9b");
    assert_eq!(generated[0]["created_at"], T1);
    assert_eq!(generated[0]["label"], "generated");
}

#[test]
fn json_without_a_plan_omits_progress_and_has_empty_generated() {
    let rig = rig();
    rig.session("s2", "main");
    let shown = rig.show_json("s2");
    assert_eq!(shown["progress_status"], "none");
    assert!(shown.get("progress").is_none());
    assert_eq!(shown["summaries"]["generated"], Value::Array(Vec::new()));
}

#[test]
fn json_with_several_plans_lists_the_candidates_and_no_figure() {
    let rig = rig();
    rig.session("s3", "main");
    rig.write("a.md", "- [ ] one\n- [x] two\n");
    rig.write("b.md", "- [ ] three\n");
    let shown = rig.show_json("s3");
    assert_eq!(shown["progress_status"], "pick_one");
    assert!(shown.get("progress").is_none());
    assert_eq!(shown["candidates"].as_array().unwrap().len(), 2);
}

#[test]
fn text_shows_the_figure_source_current_item_and_phase_counts() {
    let rig = bound_rig();
    let text = rig.show_text("s1");
    assert!(text.contains("1 of 3"), "{text}");
    assert!(
        text.contains("specs/001-x/tasks.md (spec-kit tasks)"),
        "{text}"
    );
    assert!(text.contains("T002"), "{text}");
    assert!(text.contains("Phase 1: Setup 1/3"), "{text}");
}

#[test]
fn text_marks_a_figure_whose_source_cannot_be_read() {
    let rig = bound_rig();
    fs::remove_file(rig.tree.path().join("specs/001-x/tasks.md")).unwrap();
    let text = rig.show_text("s1");
    assert!(text.contains("1 of 3"), "{text}");
    assert!(text.contains("stale"), "{text}");
}

#[test]
fn text_labels_generated_summaries_with_model_and_keeps_verbatim_text_unlabelled() {
    let rig = bound_rig();
    rig.store()
        .insert_model_summary("session", "s1", "Fixing CI flakes.", "qwen-9b", T1)
        .unwrap();
    let text = rig.show_text("s1");
    let task = text.find("fix the build").unwrap();
    let generated = text.find("generated").unwrap();
    assert!(task < generated, "{text}");
    assert!(text.contains("generated (qwen-9b"), "{text}");
    assert!(text.contains("Fixing CI flakes."), "{text}");
}

#[test]
fn text_with_several_plans_hints_at_progress_bind() {
    let rig = rig();
    rig.session("s3", "main");
    rig.write("a.md", "- [ ] one\n- [x] two\n");
    rig.write("b.md", "- [ ] three\n");
    let text = rig.show_text("s3");
    assert!(text.contains("progress bind s3"), "{text}");
    assert!(text.contains("a.md"), "{text}");
    assert!(text.contains("b.md"), "{text}");
}

#[test]
fn text_without_a_plan_shows_no_progress_line() {
    let rig = rig();
    rig.session("s2", "main");
    assert!(!rig.show_text("s2").contains("progress"));
}
