#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! `refresh` keeps plan progress current (T089): bound files are re-read, and
//! active sessions that have a clear plan get bound, so `list`, `show` and the
//! TUI open on current figures.

use claude_monitor::model::{Author, OwnerKind, Session, State};
use claude_monitor::progress::binding::{choose, view, Choice, ProgressView};
use claude_monitor::refresh::refresh;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use std::path::PathBuf;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const ROOT: &str = "/w/proj";
const PLAN: &str = "/w/proj/PENDING.md";

struct Fx {
    _dir: TempDir,
    fake: FakeEnv,
    store: Store,
}

impl Fx {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
        fake.vars.set("COPILOT_HOME", "/cop");
        fake.fs.add_dir(ROOT);
        let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
        Self {
            _dir: dir,
            fake,
            store,
        }
    }

    fn session(&self, id: &str, state: State, branch: &str) -> Session {
        let mut session = Session::new(id, "claude", id, ROOT, "2026-10-12T09:00:00Z");
        session.repo_root = Some(ROOT.to_owned());
        session.branch = Some(branch.to_owned());
        session.state = state;
        self.store.upsert_session(&session).unwrap();
        session
    }

    fn done_of(&self, session: &Session) -> Option<(u32, u32)> {
        match view(&self.store, &self.fake.env, session).unwrap() {
            ProgressView::Bound(bound) => bound.progress.map(|p| (p.done, p.total)),
            _ => None,
        }
    }
}

fn user() -> Author {
    Author::User {
        name: "ali".to_owned(),
    }
}

#[test]
#[ignore = "implemented by T089"]
fn a_refresh_rereads_a_bound_file_that_changed() {
    let fx = Fx::new();
    fx.fake.fs.add_file(PLAN, "- [ ] one\n- [ ] two\n");
    let session = fx.session("s1", State::Idle, "main");
    choose(
        &fx.store,
        &fx.fake.env,
        &session,
        &Choice::Path(PathBuf::from(PLAN)),
        &user(),
        NOW,
    )
    .unwrap();
    assert_eq!(fx.done_of(&session), Some((0, 2)));

    fx.fake.fs.add_file(PLAN, "- [x] one\n- [ ] two\n");
    let report = refresh(&fx.store, &fx.fake.env, NOW);

    assert_eq!(fx.done_of(&session), Some((1, 2)));
    assert_eq!(report.progress.changed, 1);
    assert!(report.changed(), "a viewer must redraw for new progress");
}

#[test]
#[ignore = "implemented by T089"]
fn a_refresh_binds_an_active_session_to_its_plan() {
    let fx = Fx::new();
    fx.fake.fs.add_file(
        "/w/proj/specs/008-a/tasks.md",
        "- [x] T001 one\n- [ ] T002 two\n",
    );
    let session = fx.session("s1", State::Working, "008-a");

    let report = refresh(&fx.store, &fx.fake.env, NOW);

    assert_eq!(fx.done_of(&session), Some((1, 2)));
    assert_eq!(report.progress.bound, 1);
}

#[test]
#[ignore = "implemented by T089"]
fn a_refresh_does_not_bind_a_finished_session() {
    let fx = Fx::new();
    fx.fake.fs.add_file(PLAN, "- [ ] one\n");
    fx.session("s1", State::Finished, "main");

    refresh(&fx.store, &fx.fake.env, NOW);

    assert!(fx
        .store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .is_none());
}

#[test]
#[ignore = "implemented by T089"]
fn a_second_refresh_changes_nothing() {
    let fx = Fx::new();
    fx.fake.fs.add_file(PLAN, "- [ ] one\n");
    fx.session("s1", State::Working, "main");
    refresh(&fx.store, &fx.fake.env, NOW);
    let events = fx.store.last_seq().unwrap();

    let again = refresh(&fx.store, &fx.fake.env, NOW);

    assert_eq!(again.progress.bound, 0);
    assert_eq!(again.progress.changed, 0);
    assert_eq!(fx.store.last_seq().unwrap(), events, "no new events");
}

#[test]
#[ignore = "implemented by T089"]
fn an_unreadable_bound_file_keeps_its_figure_without_a_warning() {
    let fx = Fx::new();
    fx.fake.fs.add_file(PLAN, "- [x] one\n- [ ] two\n");
    let session = fx.session("s1", State::Idle, "main");
    choose(
        &fx.store,
        &fx.fake.env,
        &session,
        &Choice::Path(PathBuf::from(PLAN)),
        &user(),
        NOW,
    )
    .unwrap();
    let elsewhere = FakeEnv::new();
    elsewhere.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    elsewhere.vars.set("COPILOT_HOME", "/cop");

    let report = refresh(&fx.store, &elsewhere.env, NOW);

    assert!(
        report.warnings.iter().all(|w| !w.contains("progress")),
        "{:?}",
        report.warnings
    );
    let binding = fx
        .store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .unwrap();
    assert_eq!(fx.store.list_steps(&binding.id).unwrap().len(), 2);
}
