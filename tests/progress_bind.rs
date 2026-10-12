#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! Plan binding (T084, US4, FR-015, FR-015a, FR-016): which file a session's
//! progress comes from, when the monitor may pick it alone, and what it shows
//! when the file changes or goes away. Runs on a real temporary tree because
//! it needs removal and modification times.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use claude_monitor::env::Env;
use claude_monitor::model::{Author, OwnerKind, Session, State};
use claude_monitor::progress::binding::{
    choose, discover_candidates, sync_all, sync_session, view, CandidateReason, Choice,
    ProgressView,
};
use claude_monitor::progress::SourceKind;
use claude_monitor::store::progress::ChosenBy;
use claude_monitor::store::{Store, StoreOptions};
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T11:00:00Z";
const T2: &str = "2026-10-12T12:00:00Z";

const TASKS: &str = "\
## Phase 1: Setup
- [x] T001 first
- [ ] T002 second
- [ ] T003 third
";

const TASKS_AFTER_EDIT: &str = "\
## Phase 1: Setup
- [x] T001 first
- [x] T002 second
- [ ] T003 third
";

struct Rig {
    _tree: TempDir,
    root: PathBuf,
    _state: TempDir,
    store: Store,
    env: Env,
}

fn rig() -> Rig {
    let tree = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let store = Store::open_in_dir(state.path(), &StoreOptions::default()).unwrap();
    Rig {
        root: tree.path().to_owned(),
        _tree: tree,
        _state: state,
        store,
        env: Env::real(),
    }
}

impl Rig {
    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn session(&self, id: &str, branch: Option<&str>) -> Session {
        let mut session = Session::new(
            id,
            "claude",
            &format!("agent-{id}"),
            &self.root.display().to_string(),
            T0,
        );
        session.repo_root = Some(self.root.display().to_string());
        session.branch = branch.map(str::to_owned);
        session.state = State::Idle;
        self.store.upsert_session(&session).unwrap();
        session
    }

    fn reopened(&self) -> Store {
        Store::open(self.store.path(), &StoreOptions::default()).unwrap()
    }
}

fn user() -> Author {
    Author::User {
        name: "ali".to_owned(),
    }
}

fn bound_label(view: &ProgressView) -> String {
    match view {
        ProgressView::Bound(bound) => bound.progress.as_ref().unwrap().source_label(),
        other => panic!("expected a bound view, got {other:?}"),
    }
}

fn set_mtime(path: &Path, secs_ago: u64) {
    let when = SystemTime::now() - Duration::from_secs(secs_ago);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

#[test]
fn the_branch_name_binds_its_spec_kit_tasks_file() {
    let rig = rig();
    rig.write("specs/008-ai-control-centre/tasks.md", TASKS);
    let session = rig.session("s1", Some("008-ai-control-centre"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    let shown = view(&rig.store, &rig.env, &session).unwrap();
    let ProgressView::Bound(bound) = &shown else {
        panic!("not bound: {shown:?}");
    };
    let progress = bound.progress.as_ref().unwrap();
    assert_eq!((progress.done, progress.total), (1, 3));
    assert!(!bound.stale);
    assert_eq!(bound.binding.chosen_by, ChosenBy::Auto);
    assert_eq!(bound.binding.source.kind, SourceKind::SpeckitTasks);
    assert!(
        bound_label(&shown).starts_with("specs/008-ai-control-centre/tasks.md"),
        "the figure names its source: {}",
        bound_label(&shown)
    );
}

#[test]
fn a_numeric_branch_prefix_is_enough() {
    let rig = rig();
    rig.write("specs/008-ai-control-centre/tasks.md", TASKS);
    let session = rig.session("s1", Some("008-w33-binding"));

    let found = discover_candidates(&session, &rig.env);

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].reason, CandidateReason::Branch);
}

#[test]
fn a_branch_with_a_namespace_still_matches() {
    let rig = rig();
    rig.write("specs/008-ai-control-centre/tasks.md", TASKS);
    let session = rig.session("s1", Some("feature/008-ai-control-centre"));

    assert_eq!(discover_candidates(&session, &rig.env).len(), 1);
}

#[test]
fn feature_json_binds_when_the_branch_says_nothing() {
    let rig = rig();
    rig.write(
        ".specify/feature.json",
        r#"{"feature_directory": "specs/003-x"}"#,
    );
    rig.write("specs/003-x/tasks.md", TASKS);
    let session = rig.session("s1", Some("main"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    let shown = view(&rig.store, &rig.env, &session).unwrap();
    assert!(bound_label(&shown).starts_with("specs/003-x/tasks.md"));
    let found = discover_candidates(&session, &rig.env);
    assert_eq!(found[0].reason, CandidateReason::FeatureJson);
}

#[test]
fn branch_and_feature_json_agreeing_is_one_candidate() {
    let rig = rig();
    rig.write(
        ".specify/feature.json",
        r#"{"feature_directory": "specs/008-a"}"#,
    );
    rig.write("specs/008-a/tasks.md", TASKS);
    let session = rig.session("s1", Some("008-a"));

    assert_eq!(discover_candidates(&session, &rig.env).len(), 1);
}

#[test]
fn two_spec_dirs_with_the_branch_prefix_are_ambiguous() {
    let rig = rig();
    rig.write("specs/008-a/tasks.md", TASKS);
    rig.write("specs/008-b/tasks.md", TASKS);
    let session = rig.session("s1", Some("008-c"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    match view(&rig.store, &rig.env, &session).unwrap() {
        ProgressView::PickOne(candidates) => assert_eq!(candidates.len(), 2),
        other => panic!("expected a pick-one result, got {other:?}"),
    }
    assert!(rig
        .store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .is_none());
}

#[test]
fn branch_and_feature_json_disagreeing_is_ambiguous() {
    let rig = rig();
    rig.write(
        ".specify/feature.json",
        r#"{"feature_directory": "specs/003-x"}"#,
    );
    rig.write("specs/003-x/tasks.md", TASKS);
    rig.write("specs/008-a/tasks.md", TASKS);
    let session = rig.session("s1", Some("008-a"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    assert!(matches!(
        view(&rig.store, &rig.env, &session).unwrap(),
        ProgressView::PickOne(c) if c.len() == 2
    ));
}

#[test]
fn several_checklist_files_are_ambiguous() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n- [x] two\n");
    rig.write("todo.md", "- [ ] three\n");
    let session = rig.session("s1", Some("main"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    match view(&rig.store, &rig.env, &session).unwrap() {
        ProgressView::PickOne(candidates) => {
            assert_eq!(candidates.len(), 2);
            assert!(candidates
                .iter()
                .all(|c| c.reason == CandidateReason::Checklist));
        }
        other => panic!("expected a pick-one result, got {other:?}"),
    }
}

#[test]
fn a_lone_checklist_file_is_the_only_plan_so_it_binds() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n- [x] two\n");
    rig.write("README.md", "no tasks here\n");
    let session = rig.session("s1", Some("main"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    let ProgressView::Bound(bound) = view(&rig.store, &rig.env, &session).unwrap() else {
        panic!("not bound");
    };
    assert_eq!(bound.binding.source.kind, SourceKind::MarkdownChecklist);
    assert_eq!(bound.progress.unwrap().label(), "1 of 2");
}

#[test]
fn a_spec_match_beats_checklist_files() {
    let rig = rig();
    rig.write("specs/008-a/tasks.md", TASKS);
    rig.write("PENDING.md", "- [ ] one\n");
    rig.write("todo.md", "- [ ] two\n");
    let session = rig.session("s1", Some("008-a"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    assert!(bound_label(&view(&rig.store, &rig.env, &session).unwrap())
        .starts_with("specs/008-a/tasks.md"));
}

#[test]
fn no_plan_and_no_candidates_shows_no_figure() {
    let rig = rig();
    rig.write("README.md", "nothing to do\n");
    let session = rig.session("s1", Some("main"));

    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    assert!(matches!(
        view(&rig.store, &rig.env, &session).unwrap(),
        ProgressView::None
    ));
}

#[test]
fn the_users_choice_survives_a_restart() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n- [x] two\n");
    let todo = rig.write("todo.md", "- [ ] three\n- [ ] four\n- [ ] five\n");
    let session = rig.session("s1", Some("main"));
    sync_session(&rig.store, &rig.env, &session, T1).unwrap();
    assert!(matches!(
        view(&rig.store, &rig.env, &session).unwrap(),
        ProgressView::PickOne(_)
    ));

    choose(
        &rig.store,
        &rig.env,
        &session,
        &Choice::Path(todo),
        &user(),
        T1,
    )
    .unwrap();

    let reopened = rig.reopened();
    sync_session(&reopened, &rig.env, &session, T2).unwrap();
    let ProgressView::Bound(bound) = view(&reopened, &rig.env, &session).unwrap() else {
        panic!("choice lost");
    };
    assert_eq!(bound.binding.chosen_by, ChosenBy::User);
    assert_eq!(bound.progress.unwrap().label(), "0 of 3");
}

#[test]
fn a_relative_choice_resolves_against_the_session_root() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    rig.write("todo.md", "- [ ] two\n- [ ] three\n");
    let session = rig.session("s1", Some("main"));

    choose(
        &rig.store,
        &rig.env,
        &session,
        &Choice::Path(PathBuf::from("todo.md")),
        &user(),
        T1,
    )
    .unwrap();

    let shown = view(&rig.store, &rig.env, &session).unwrap();
    assert!(bound_label(&shown).starts_with("todo.md"));
}

#[test]
fn a_file_that_is_not_a_plan_cannot_be_chosen() {
    let rig = rig();
    let readme = rig.write("README.md", "no checkboxes\n");
    let session = rig.session("s1", Some("main"));

    let result = choose(
        &rig.store,
        &rig.env,
        &session,
        &Choice::Path(readme),
        &user(),
        T1,
    );

    assert!(result.is_err());
    assert!(rig
        .store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .is_none());
}

#[test]
fn choosing_none_stays_chosen_and_shows_no_figure() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    let session = rig.session("s1", Some("main"));

    choose(&rig.store, &rig.env, &session, &Choice::None, &user(), T1).unwrap();
    sync_session(&rig.store, &rig.env, &session, T2).unwrap();

    let ProgressView::Bound(bound) = view(&rig.store, &rig.env, &session).unwrap() else {
        panic!("opt-out lost");
    };
    assert!(bound.progress.is_none());
    assert_eq!(bound.binding.chosen_by, ChosenBy::User);
}

#[test]
fn a_vanished_source_is_stale_not_zero() {
    let rig = rig();
    let tasks = rig.write("specs/008-a/tasks.md", TASKS);
    let session = rig.session("s1", Some("008-a"));
    sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    fs::remove_file(&tasks).unwrap();
    sync_session(&rig.store, &rig.env, &session, T2).unwrap();

    let ProgressView::Bound(bound) = view(&rig.store, &rig.env, &session).unwrap() else {
        panic!("binding lost with the file");
    };
    assert!(bound.stale);
    let progress = bound.progress.expect("the last known figure is kept");
    assert_eq!((progress.done, progress.total), (1, 3));

    fs::write(&tasks, TASKS_AFTER_EDIT).unwrap();
    sync_session(&rig.store, &rig.env, &session, T2).unwrap();
    let ProgressView::Bound(bound) = view(&rig.store, &rig.env, &session).unwrap() else {
        panic!("not bound");
    };
    assert!(!bound.stale);
    assert_eq!(bound.progress.unwrap().done, 2);
}

#[test]
fn steps_are_upserted_idempotently_with_stable_ids() {
    let rig = rig();
    let tasks = rig.write("specs/008-a/tasks.md", TASKS);
    let session = rig.session("s1", Some("008-a"));

    let first = sync_session(&rig.store, &rig.env, &session, T1).unwrap();
    let binding = rig
        .store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .unwrap();
    let before = rig.store.list_steps(&binding.id).unwrap();
    let second = sync_session(&rig.store, &rig.env, &session, T1).unwrap();

    assert!(first.bound && first.changed);
    assert!(!second.bound && !second.changed);
    assert_eq!(rig.store.list_steps(&binding.id).unwrap(), before);
    assert_eq!(before.len(), 3);
    assert_eq!(before[1].external_id.as_deref(), Some("T002"));

    fs::write(&tasks, TASKS_AFTER_EDIT).unwrap();
    let third = sync_session(&rig.store, &rig.env, &session, T2).unwrap();

    assert!(third.changed);
    let after = rig.store.list_steps(&binding.id).unwrap();
    assert_eq!(after.len(), 3);
    assert_eq!(after[1].external_id.as_deref(), Some("T002"));
    assert_ne!(after[1].status, before[1].status);
    assert_eq!(after[0], before[0]);
}

#[test]
fn sync_all_rereads_only_the_files_modified_since_their_last_read() {
    let rig = rig();
    let a = rig.write("a/PENDING.md", "- [ ] a1\n- [ ] a2\n");
    let b = rig.write("b/PENDING.md", "- [ ] b1\n- [ ] b2\n");
    let mut sa = rig.session("sa", Some("main"));
    sa.repo_root = Some(rig.root.join("a").display().to_string());
    rig.store.upsert_session(&sa).unwrap();
    let mut sb = rig.session("sb", Some("main"));
    sb.repo_root = Some(rig.root.join("b").display().to_string());
    rig.store.upsert_session(&sb).unwrap();
    let now = rig.env.clock.now();
    sync_all(&rig.store, &rig.env, &now);

    fs::write(&a, "- [x] a1\n- [ ] a2\n").unwrap();
    fs::write(&b, "- [x] b1\n- [x] b2\n").unwrap();
    set_mtime(&b, 3600);
    let summary = sync_all(&rig.store, &rig.env, &now);

    let label = |id: &str| {
        let session = rig.store.get_session(id).unwrap().unwrap();
        match view(&rig.store, &rig.env, &session).unwrap() {
            ProgressView::Bound(bound) => bound.progress.unwrap().label(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(label("sa"), "1 of 2");
    assert_eq!(label("sb"), "0 of 2", "b was not touched since it was read");
    assert_eq!(summary.changed, 1);
}

#[test]
fn sync_all_binds_only_active_sessions_without_a_binding() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    let active = rig.session("active", Some("main"));
    let mut finished = rig.session("finished", Some("main"));
    finished.state = State::Finished;
    rig.store.upsert_session(&finished).unwrap();

    sync_all(&rig.store, &rig.env, T1);

    assert!(rig
        .store
        .get_progress_binding(OwnerKind::Session, &active.id)
        .unwrap()
        .is_some());
    assert!(rig
        .store
        .get_progress_binding(OwnerKind::Session, "finished")
        .unwrap()
        .is_none());
}

#[test]
fn sync_all_skips_a_corrupt_row_and_still_binds_the_rest() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    let active = rig.session("active", Some("main"));
    rig.session("bad", Some("main"));
    let conn = rig.store.connection();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute("UPDATE sessions SET state = 'bogus' WHERE id = 'bad'", [])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
        .unwrap();

    let summary = sync_all(&rig.store, &rig.env, T1);

    assert!(rig
        .store
        .get_progress_binding(OwnerKind::Session, &active.id)
        .unwrap()
        .is_some());
    assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
}

#[test]
fn a_branch_that_appears_later_resolves_an_ambiguous_pick() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    rig.write("todo.md", "- [ ] two\n");
    rig.write("specs/008-a/tasks.md", TASKS);
    let mut session = rig.session("s1", None);
    sync_session(&rig.store, &rig.env, &session, T1).unwrap();
    assert!(matches!(
        view(&rig.store, &rig.env, &session).unwrap(),
        ProgressView::PickOne(_)
    ));

    session.branch = Some("008-a".to_owned());
    sync_session(&rig.store, &rig.env, &session, T2).unwrap();

    assert!(bound_label(&view(&rig.store, &rig.env, &session).unwrap())
        .starts_with("specs/008-a/tasks.md"));
}

#[test]
fn an_auto_binding_moves_to_a_stronger_hint_but_a_user_choice_does_not() {
    let rig = rig();
    rig.write("PENDING.md", "- [ ] one\n");
    rig.write("specs/008-a/tasks.md", TASKS);
    let mut auto = rig.session("auto", None);
    let mut chosen = rig.session("chosen", None);
    sync_session(&rig.store, &rig.env, &auto, T1).unwrap();
    choose(
        &rig.store,
        &rig.env,
        &chosen,
        &Choice::Path(PathBuf::from("PENDING.md")),
        &user(),
        T1,
    )
    .unwrap();

    auto.branch = Some("008-a".to_owned());
    chosen.branch = Some("008-a".to_owned());
    sync_session(&rig.store, &rig.env, &auto, T2).unwrap();
    sync_session(&rig.store, &rig.env, &chosen, T2).unwrap();

    assert!(bound_label(&view(&rig.store, &rig.env, &auto).unwrap())
        .starts_with("specs/008-a/tasks.md"));
    assert!(bound_label(&view(&rig.store, &rig.env, &chosen).unwrap()).starts_with("PENDING.md"));
}
