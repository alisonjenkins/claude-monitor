#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! `list`, `show` and the dashboard see sessions that exist only on disk: the
//! binary runs a refresh pass first, unless told not to.

use claude_monitor::cli::{run_in, run_with, Command, RunOptions};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::{json, Value};
use tempfile::TempDir;

const WITH_REFRESH: RunOptions = RunOptions { refresh: true };
const WITHOUT_REFRESH: RunOptions = RunOptions { refresh: false };

fn list() -> Command {
    Command::List {
        states: vec![],
        agent: None,
        repo: None,
        project: None,
        text: None,
        section: None,
        include_dismissed: false,
        expand_finished: false,
        expand_unseen: false,
    }
}

fn disk_only_session() -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    fake.vars.set("COPILOT_HOME", "/cop");
    let line = json!({
        "type": "user", "uuid": "u1", "parentUuid": null, "sessionId": "sess-1",
        "timestamp": "2026-10-12T09:00:00.000Z", "cwd": "/w/proj", "version": "2.1.293",
        "isSidechain": false, "message": {"role": "user", "content": "hello"},
    });
    fake.fs
        .add_file("/cfg/projects/-w-proj/sess-1.jsonl", &format!("{line}\n"));
    fake.fs.add_dir("/w/proj");
    fake
}

/// A state directory whose legacy-import marker is already recorded, so the
/// viewer never imports from the real runtime directory.
fn state_dir(fake: &FakeEnv) -> TempDir {
    let state = TempDir::new().unwrap();
    let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
    let empty = TempDir::new().unwrap();
    legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
    state
}

fn listed(fake: &FakeEnv, state: &TempDir, options: RunOptions) -> usize {
    let output = run_with(list(), &fake.env, Some(state.path()), options).unwrap();
    let value: Value = serde_json::from_str(&output.render(true)).unwrap();
    value["sessions"].as_array().unwrap().len()
}

#[test]
fn list_shows_sessions_found_on_disk() {
    let fake = disk_only_session();
    let state = state_dir(&fake);

    assert_eq!(listed(&fake, &state, WITH_REFRESH), 1);
}

#[test]
fn no_refresh_leaves_the_store_as_it_was() {
    let fake = disk_only_session();
    let state = state_dir(&fake);

    assert_eq!(listed(&fake, &state, WITHOUT_REFRESH), 0);
    assert_eq!(listed(&fake, &state, WITHOUT_REFRESH), 0);
}

#[test]
fn run_in_never_refreshes() {
    let fake = disk_only_session();
    let state = state_dir(&fake);

    let output = run_in(list(), &fake.env, Some(state.path())).unwrap();

    let value: Value = serde_json::from_str(&output.render(true)).unwrap();
    assert_eq!(value["sessions"].as_array().unwrap().len(), 0);
}

#[test]
fn show_refreshes_before_it_looks() {
    let fake = disk_only_session();
    let state = state_dir(&fake);
    let show = Command::Show {
        session: "no-such".to_owned(),
    };

    assert!(run_with(show, &fake.env, Some(state.path()), WITH_REFRESH).is_err());

    assert_eq!(listed(&fake, &state, WITHOUT_REFRESH), 1);
}
