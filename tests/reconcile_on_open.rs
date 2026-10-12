#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! The first read of `list`, `show` and the dashboard data path judges a
//! recorded-as-working session by its process, with no hook or watcher having
//! fired since the process died.

use claude_monitor::cli::{run_with, Command, RunOptions};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::Value;
use tempfile::TempDir;

const WITH_REFRESH: RunOptions = RunOptions { refresh: true };
const WITHOUT_REFRESH: RunOptions = RunOptions { refresh: false };
const DEAD_PID: u32 = 4242;
const CWD: &str = "/w/proj";

struct Fx {
    fake: FakeEnv,
    state: TempDir,
}

fn working_session_whose_process_is_gone() -> Fx {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    fake.vars.set("COPILOT_HOME", "/cop");
    fake.fs.add_dir(CWD);
    let state = TempDir::new().unwrap();
    let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
    let empty = TempDir::new().unwrap();
    legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
    let mut session = Session::new("id-1", "claude", "sess-1", CWD, "2026-10-12T08:00:00Z");
    session.state = State::Working;
    session.pid = Some(DEAD_PID);
    session.pid_start = Some(5000);
    store.upsert_session(&session).unwrap();
    Fx { fake, state }
}

fn stored_state(fx: &Fx) -> State {
    let store = Store::open_in_dir(fx.state.path(), &StoreOptions::viewer()).unwrap();
    store
        .get_session_by_agent_id("claude", "sess-1")
        .unwrap()
        .unwrap()
        .state
}

fn run_json(fx: &Fx, command: Command, options: RunOptions) -> Value {
    let output = run_with(command, &fx.fake.env, Some(fx.state.path()), options).unwrap();
    serde_json::from_str(&output.render(true)).unwrap()
}

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

#[test]
fn the_fixture_is_working_until_something_reads_it() {
    let fx = working_session_whose_process_is_gone();

    run_json(&fx, list(), WITHOUT_REFRESH);

    assert_eq!(stored_state(&fx), State::Working);
}

#[test]
fn the_first_list_shows_a_dead_working_session_as_interrupted() {
    let fx = working_session_whose_process_is_gone();

    let value = run_json(&fx, list(), WITH_REFRESH);

    assert_eq!(stored_state(&fx), State::Interrupted);
    assert!(
        value.to_string().contains("interrupted"),
        "list output: {value}"
    );
}

#[test]
fn the_first_show_shows_a_dead_working_session_as_interrupted() {
    let fx = working_session_whose_process_is_gone();
    let show = Command::Show {
        session: "id-1".to_owned(),
    };

    let value = run_json(&fx, show, WITH_REFRESH);

    assert_eq!(stored_state(&fx), State::Interrupted);
    assert!(
        value.to_string().contains("interrupted"),
        "show output: {value}"
    );
}

#[cfg(feature = "tui")]
#[test]
fn the_dashboard_snapshot_after_the_first_refresh_has_no_active_session() {
    use claude_monitor::refresh::refresh;
    use claude_monitor::store::queries::SectionName;
    use claude_monitor::tui::app::Snapshot;

    let fx = working_session_whose_process_is_gone();
    let store = Store::open_in_dir(fx.state.path(), &StoreOptions::viewer()).unwrap();
    let now = fx.fake.env.clock.now();

    refresh(&store, &fx.fake.env, &now);
    let snapshot = Snapshot::load(&store, &now).unwrap();

    assert_eq!(stored_state(&fx), State::Interrupted);
    let section = |name| {
        snapshot
            .overview
            .sections
            .iter()
            .find(|s| s.name == name)
            .map_or(0, |s| s.sessions.len())
    };
    assert_eq!(section(SectionName::Active), 0);
    assert_eq!(section(SectionName::Interrupted), 1);
}
