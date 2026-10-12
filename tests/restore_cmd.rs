//! The `restore` command (T073): which sessions it selects, that without
//! `--yes` it only prints the plan, and that nothing beyond the selection
//! starts. The agent is `sleep 30`; the tmux server is private.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use claude_monitor::cli::{run_in, Command, Output};
use claude_monitor::error::Error;
use claude_monitor::model::State;
use common::Fixture;
use serde_json::Value;

fn restore(all_interrupted: bool, include_finished: bool, skip: &[&str], yes: bool) -> Command {
    Command::Restore {
        all_interrupted,
        include_finished,
        fresh: false,
        skip: skip.iter().map(|s| (*s).to_owned()).collect(),
        yes,
    }
}

fn json(result: Result<Output, Error>) -> Value {
    let output = result.unwrap_or_else(|e| panic!("restore failed: {e}"));
    serde_json::from_str(&output.render(true)).unwrap()
}

fn ids(list: &Value) -> Vec<&str> {
    let mut ids: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["session"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    ids
}

/// Two interrupted, one finished, one dismissed-interrupted and one working.
fn mixed(fx: &Fixture) {
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    fx.add_session("fin1", State::Finished, "c");
    let mut dismissed = fx.add_session("dis1", State::Interrupted, "d");
    dismissed.dismissed = true;
    fx.save(&dismissed);
    fx.add_session("wrk1", State::Working, "e");
}

#[test]
fn without_yes_it_prints_the_plan_and_starts_nothing() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);

    let out = json(run_in(
        restore(true, false, &[], false),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(out["ok"], true);
    assert_eq!(out["executed"], false);
    assert_eq!(ids(&out["would_start"]), ["int1", "int2"]);
    assert!(!fx.server_has_sessions(), "tmux was started by a preview");
    assert!(fx.respawn_events().is_empty());
}

#[test]
fn the_text_plan_names_each_session_and_its_directory() {
    let Some(fx) = Fixture::new() else { return };
    let a = fx.add_session("int1", State::Interrupted, "a");

    let output = run_in(
        restore(true, false, &[], false),
        &fx.env,
        Some(&fx.state_dir),
    )
    .unwrap();

    let text = output.render(false);
    assert!(text.contains("int1"), "{text}");
    assert!(text.contains(&a.cwd), "{text}");
}

#[test]
fn all_interrupted_yes_starts_only_the_interrupted_ones() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);

    let out = json(run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(out["executed"], true);
    assert_eq!(ids(&out["started"]), ["int1", "int2"]);
    assert_eq!(
        fx.pane_ids().len(),
        3,
        "the session's shell window plus the two interrupted sessions"
    );
    let events: Vec<_> = fx
        .respawn_events()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(events.len(), 2, "{events:?}");
}

#[test]
fn finished_sessions_are_included_only_with_include_finished() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);

    let out = json(run_in(
        restore(true, true, &[], false),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(ids(&out["would_start"]), ["fin1", "int1", "int2"]);
}

#[test]
fn skip_leaves_a_session_out_by_id_or_prefix() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);

    let out = json(run_in(
        restore(true, false, &["int2"], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(ids(&out["started"]), ["int1"]);
    assert_eq!(out["skipped"], serde_json::json!(["int2"]));
    assert_eq!(fx.pane_ids().len(), 2);
}

#[test]
fn skipping_an_unknown_session_is_a_refusal_that_starts_nothing() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);

    let error = run_in(
        restore(true, false, &["nope"], true),
        &fx.env,
        Some(&fx.state_dir),
    )
    .unwrap_err();

    assert!(
        matches!(
            &error,
            Error::Refused {
                code: "unknown_session",
                ..
            }
        ),
        "{error}"
    );
    assert!(!fx.server_has_sessions());
}

#[test]
fn yes_without_all_interrupted_is_a_usage_error() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);

    let error = run_in(
        restore(false, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    )
    .unwrap_err();

    assert!(matches!(error, Error::Usage(_)), "{error}");
    assert_eq!(error.exit_code(), 1);
    assert!(!fx.server_has_sessions());
}

#[test]
fn fresh_plans_a_fresh_start_for_each_session() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);
    let command = Command::Restore {
        all_interrupted: true,
        include_finished: false,
        fresh: true,
        skip: Vec::new(),
        yes: false,
    };

    let out = json(run_in(command, &fx.env, Some(&fx.state_dir)));

    for entry in out["would_start"].as_array().unwrap() {
        assert_eq!(entry["mode"], "fresh");
        assert!(!entry["notices"].as_array().unwrap().is_empty());
    }
}

#[test]
fn a_session_that_cannot_resume_is_refused_and_the_rest_start_with_exit_3() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);
    let bad = fx.store().get_session("int2").unwrap().unwrap();
    std::fs::remove_dir(&bad.cwd).unwrap();

    let error = run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    )
    .unwrap_err();

    assert!(matches!(error, Error::Partial { .. }), "{error}");
    assert_eq!(error.exit_code(), 3);
    assert_eq!(fx.pane_ids().len(), 2);
}

#[test]
fn the_preview_reports_refusals_and_still_exits_0() {
    let Some(fx) = Fixture::new() else { return };
    mixed(&fx);
    let bad = fx.store().get_session("int2").unwrap().unwrap();
    std::fs::remove_dir(&bad.cwd).unwrap();

    let out = json(run_in(
        restore(true, false, &[], false),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(ids(&out["would_start"]), ["int1"]);
    assert_eq!(ids(&out["would_refuse"]), ["int2"]);
    assert_eq!(out["would_refuse"][0]["code"], "missing_directory");
}

#[test]
fn nothing_to_restore_is_success() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("wrk1", State::Working, "e");

    let out = json(run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(out["ok"], true);
    assert!(out["started"].as_array().unwrap().is_empty());
    assert!(!fx.server_has_sessions());
}
