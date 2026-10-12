#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! `boot-check` (FR-019a, FR-032, FR-034): after a login it counts the
//! interrupted sessions, notifies once, leaves a marker and starts nothing;
//! `--banner` shows the marker once through tmux and clears it.

use claude_monitor::cli::{run_in, Command};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::{FakeEnv, TmuxCall};
use serde_json::Value;
use tempfile::TempDir;

const MARKER: &str = "boot-check-pending";

struct Fx {
    fake: FakeEnv,
    state: TempDir,
}

fn fixture(sessions: &[(&str, State, bool)]) -> Fx {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
    fake.vars.set("COPILOT_HOME", "/cop");
    let state = TempDir::new().unwrap();
    let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
    let empty = TempDir::new().unwrap();
    legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
    for (id, state, dismissed) in sessions {
        let mut session = Session::new(id, "claude", id, "/w/proj", "2026-10-12T08:00:00Z");
        session.state = *state;
        session.dismissed = *dismissed;
        store.upsert_session(&session).unwrap();
    }
    Fx { fake, state }
}

fn boot_check(fx: &Fx, banner: bool) -> Value {
    let output = run_in(
        Command::BootCheck { banner },
        &fx.fake.env,
        Some(fx.state.path()),
    )
    .unwrap();
    let rendered = output.render(true);
    serde_json::from_str(&rendered).unwrap()
}

fn marker_exists(fx: &Fx) -> bool {
    fx.state.path().join(MARKER).exists()
}

fn spawn_calls(fx: &Fx) -> Vec<TmuxCall> {
    fx.fake
        .tmux
        .calls()
        .into_iter()
        .filter(|c| !matches!(c, TmuxCall::ListPanes | TmuxCall::DisplayMessage(_)))
        .collect()
}

#[test]
fn interrupted_sessions_give_one_notification_and_a_marker() {
    let fx = fixture(&[
        ("a", State::Interrupted, false),
        ("b", State::Interrupted, false),
        ("c", State::Idle, false),
    ]);

    let value = boot_check(&fx, false);

    let sent = fx.fake.notifier.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].0.contains("interrupted") || sent[0].1.contains("interrupted"),
        "{sent:?}"
    );
    assert!(sent[0].1.contains('2'), "count in body: {sent:?}");
    assert!(marker_exists(&fx));
    assert_eq!(value["interrupted"], 2);
}

#[test]
fn a_second_run_over_the_same_sessions_does_not_notify_again() {
    let fx = fixture(&[("a", State::Interrupted, false)]);

    let first = boot_check(&fx, false);
    boot_check(&fx, true);
    let second = boot_check(&fx, false);

    assert_eq!(fx.fake.notifier.sent().len(), 1);
    assert!(!marker_exists(&fx), "the shown banner must not come back");
    assert_eq!(first["interrupted"], 1);
    assert_eq!(second["interrupted"], 1);
}

#[test]
fn a_newly_interrupted_session_notifies_again() {
    let fx = fixture(&[("a", State::Interrupted, false)]);
    boot_check(&fx, false);
    boot_check(&fx, true);

    let store = Store::open_in_dir(fx.state.path(), &StoreOptions::viewer()).unwrap();
    let mut late = Session::new("b", "claude", "b", "/w/proj", "2026-10-12T09:00:00Z");
    late.state = State::Interrupted;
    store.upsert_session(&late).unwrap();
    boot_check(&fx, false);

    let sent = fx.fake.notifier.sent();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[1].1.contains('2'), "{sent:?}");
    assert!(marker_exists(&fx));
}

#[test]
fn once_everything_is_cleared_a_later_interruption_notifies_again() {
    let fx = fixture(&[("a", State::Interrupted, false)]);
    boot_check(&fx, false);
    let store = Store::open_in_dir(fx.state.path(), &StoreOptions::viewer()).unwrap();
    let mut session = store
        .get_session_by_agent_id("claude", "a")
        .unwrap()
        .unwrap();
    session.dismissed = true;
    store.upsert_session(&session).unwrap();
    boot_check(&fx, false);
    session.dismissed = false;
    store.upsert_session(&session).unwrap();

    boot_check(&fx, false);

    assert_eq!(fx.fake.notifier.sent().len(), 2);
}

#[test]
fn boot_check_starts_nothing() {
    let fx = fixture(&[("a", State::Interrupted, false)]);

    boot_check(&fx, false);

    assert_eq!(spawn_calls(&fx), vec![]);
    assert!(!fx
        .fake
        .tmux
        .calls()
        .iter()
        .any(|c| matches!(c, TmuxCall::DisplayMessage(_))));
}

#[test]
fn no_interrupted_sessions_emits_nothing() {
    let fx = fixture(&[("a", State::Idle, false), ("b", State::Working, false)]);

    let value = boot_check(&fx, false);

    assert!(fx.fake.notifier.sent().is_empty());
    assert!(!marker_exists(&fx));
    assert_eq!(value["interrupted"], 0);
}

#[test]
fn finished_and_dismissed_sessions_are_not_counted() {
    let fx = fixture(&[
        ("fin", State::Finished, false),
        ("dis", State::Interrupted, true),
    ]);

    let value = boot_check(&fx, false);

    assert!(fx.fake.notifier.sent().is_empty());
    assert!(!marker_exists(&fx));
    assert_eq!(value["interrupted"], 0);
}

#[test]
fn a_stale_marker_is_cleared_when_nothing_is_interrupted() {
    let fx = fixture(&[("a", State::Idle, false)]);
    std::fs::write(fx.state.path().join(MARKER), "1 AI session interrupted").unwrap();

    boot_check(&fx, false);

    assert!(!marker_exists(&fx));
}

#[test]
#[ignore = "implemented by T076"]
fn banner_prints_the_message_once_via_tmux_and_clears_the_marker() {
    let fx = fixture(&[
        ("a", State::Interrupted, false),
        ("b", State::Interrupted, false),
    ]);
    boot_check(&fx, false);

    boot_check(&fx, true);

    let messages: Vec<_> = fx
        .fake
        .tmux
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            TmuxCall::DisplayMessage(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages,
        vec!["2 AI sessions interrupted, prefix+A to review"]
    );
    assert!(!marker_exists(&fx));
}

#[test]
#[ignore = "implemented by T076"]
fn a_second_banner_prints_nothing() {
    let fx = fixture(&[("a", State::Interrupted, false)]);
    boot_check(&fx, false);
    boot_check(&fx, true);
    let before = fx.fake.tmux.calls().len();

    boot_check(&fx, true);

    assert_eq!(fx.fake.tmux.calls().len(), before);
}

#[test]
#[ignore = "implemented by T076"]
fn banner_without_a_marker_prints_nothing_and_spawns_nothing() {
    let fx = fixture(&[("a", State::Interrupted, false)]);

    boot_check(&fx, true);

    assert!(fx.fake.tmux.calls().is_empty());
    assert!(fx.fake.notifier.sent().is_empty());
}
