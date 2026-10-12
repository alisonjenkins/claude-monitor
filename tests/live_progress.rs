#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Plan files in the live loop (T089): the file source is part of the real
//! source list, and editing a bound file refreshes the store with no other wake.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use claude_monitor::env::Env;
use claude_monitor::live::{build_sources, build_sources_with_files, spawn_live};
use claude_monitor::model::{Author, OwnerKind, Session, State};
use claude_monitor::progress::binding::{choose, Choice};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeVars;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const PATIENCE: Duration = Duration::from_secs(8);

struct Rig {
    _state: TempDir,
    tree: TempDir,
    store: Store,
    env: Env,
}

fn rig() -> Rig {
    let state = TempDir::new().unwrap();
    let tree = TempDir::new().unwrap();
    for sub in ["cfg/sessions", "cfg/projects", "cop/session-state"] {
        fs::create_dir_all(tree.path().join(sub)).unwrap();
    }
    let vars = Arc::new(FakeVars::default());
    vars.set(
        "CLAUDE_CONFIG_DIR",
        tree.path().join("cfg").to_str().unwrap(),
    );
    vars.set("COPILOT_HOME", tree.path().join("cop").to_str().unwrap());
    let mut env = Env::real();
    env.vars = vars;
    let store = Store::open_in_dir(state.path(), &StoreOptions::default()).unwrap();
    Rig {
        _state: state,
        tree,
        store,
        env,
    }
}

impl Rig {
    fn bind_plan(&self, contents: &str) -> PathBuf {
        let repo = self.tree.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let plan = repo.join("PENDING.md");
        fs::write(&plan, contents).unwrap();
        let mut session = Session::new("s1", "claude", "s1", &repo.display().to_string(), NOW);
        session.state = State::Idle;
        self.store.upsert_session(&session).unwrap();
        choose(
            &self.store,
            &self.env,
            &session,
            &Choice::Path(plan.clone()),
            &Author::User {
                name: "ali".to_owned(),
            },
            NOW,
        )
        .unwrap();
        plan
    }
}

#[test]
fn the_file_source_joins_the_real_sources() {
    let rig = rig();

    let plain = build_sources(&rig.env);
    let with_files = build_sources_with_files(&rig.env, &rig.store);

    assert_eq!(with_files.sources.len(), plain.sources.len() + 1);
    assert_eq!(with_files.warnings, plain.warnings);
}

#[test]
fn an_unwatchable_bound_file_is_a_warning_not_a_failure() {
    let rig = rig();
    rig.bind_plan("- [ ] one\n");
    fs::remove_dir_all(rig.tree.path().join("repo")).unwrap();

    let built = build_sources_with_files(&rig.env, &rig.store);

    assert!(
        built.warnings.iter().any(|w| w.contains("plan files")),
        "{:?}",
        built.warnings
    );
    assert!(!built.sources.is_empty());
}

#[test]
fn editing_a_bound_file_updates_the_store_without_another_wake() {
    let rig = rig();
    let plan = rig.bind_plan("- [ ] one\n- [ ] two\n");
    let built = build_sources_with_files(&rig.env, &rig.store);
    let live_store = Store::open(rig.store.path(), &StoreOptions::default()).unwrap();
    let live = spawn_live(
        live_store,
        rig.env.clone(),
        built.sources,
        built.proc_exit,
        |_| {},
    )
    .unwrap();

    fs::write(&plan, "- [x] one\n- [ ] two\n").unwrap();

    let binding = rig
        .store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .unwrap();
    let started = Instant::now();
    let done = loop {
        let steps = rig.store.list_steps(&binding.id).unwrap();
        if steps
            .iter()
            .any(|s| s.text == "one" && s.status == claude_monitor::model::StepStatus::Done)
        {
            break true;
        }
        if started.elapsed() > PATIENCE {
            break false;
        }
        thread::sleep(Duration::from_millis(100));
    };
    drop(live);
    assert!(done, "the store never saw the edit");
}
