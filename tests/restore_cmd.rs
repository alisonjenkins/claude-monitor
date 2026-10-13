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
    assert_eq!(fx.pane_ids().len(), 2, "one window per interrupted session");
    let events: Vec<_> = fx
        .respawn_events()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(events.len(), 2, "{events:?}");
}

fn place(fx: &Fixture, id: &str, session: &str, window: &str, index: &str) {
    let mut s = fx.store().get_session(id).unwrap().unwrap();
    s.tmux_session = Some(session.to_owned());
    s.tmux_window_name = Some(window.to_owned());
    s.tmux_window_index = Some(index.to_owned());
    fx.save(&s);
}

/// `(tmux session, window name)` of every window on the private server, in
/// window index order within each session.
fn windows(fx: &Fixture) -> Vec<(String, String)> {
    let out = fx.tmux(&[
        "list-windows",
        "-a",
        "-F",
        "#{session_name}\t#{window_name}",
    ]);
    let mut rows: Vec<(String, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(session, window)| (session.to_owned(), window.to_owned()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

#[test]
fn each_session_returns_to_its_original_tmux_session_and_window() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    fx.add_session("int3", State::Interrupted, "c");
    place(&fx, "int1", "my project", "api: build", "3");
    place(&fx, "int2", "my project", "web", "1");
    place(&fx, "int3", "notes", "docs", "0");

    let out = json(run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(out["executed"], true);
    assert_eq!(
        windows(&fx),
        [
            ("my project".to_owned(), "web".to_owned()),
            ("my project".to_owned(), "api: build".to_owned()),
            ("notes".to_owned(), "docs".to_owned()),
        ],
        "windows in index order, no placeholder, no `agents` session"
    );
}

#[test]
fn the_plan_shows_each_sessions_target_tmux_session_and_window() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    place(&fx, "int1", "my project", "api: build", "3");

    let out = run_in(
        restore(true, false, &[], false),
        &fx.env,
        Some(&fx.state_dir),
    )
    .unwrap();

    let text = out.render(false);
    assert!(text.contains("my project:api: build"), "{text}");
    assert!(text.contains("agents:"), "{text}");
    let body: Value = serde_json::from_str(&out.render(true)).unwrap();
    let target = |id: &str| {
        let entry = body["would_start"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["session"] == id)
            .unwrap();
        (
            entry["tmux_session"].as_str().unwrap().to_owned(),
            entry["tmux_target"].as_str().unwrap().to_owned(),
        )
    };
    assert_eq!(
        target("int1"),
        ("my project".to_owned(), "my project:api: build".to_owned())
    );
    assert_eq!(target("int2").0, "agents");
    assert!(!fx.server_has_sessions(), "a preview started tmux");
}

fn place_in_pane(fx: &Fixture, id: &str, window: &str, pane: &str) {
    place(fx, id, "work", window, "1");
    let mut s = fx.store().get_session(id).unwrap().unwrap();
    s.tmux_pane_index = Some(pane.to_owned());
    s.tmux_window_panes = Some("2".to_owned());
    s.tmux_window_layout = Some("b25d,200x50,0,0{100x50,0,0,0,99x50,101,0,1}".to_owned());
    fx.save(&s);
}

#[test]
fn the_plan_shows_grouped_panes_as_session_window_pane() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    fx.add_session("int3", State::Interrupted, "c");
    place_in_pane(&fx, "int1", "api: build", "0");
    place_in_pane(&fx, "int2", "api: build", "1");
    place(&fx, "int3", "work", "docs", "2");

    let out = run_in(
        restore(true, false, &[], false),
        &fx.env,
        Some(&fx.state_dir),
    )
    .unwrap();

    let text = out.render(false);
    assert!(text.contains("work:api: build.0"), "{text}");
    assert!(text.contains("work:api: build.1"), "{text}");
    assert!(!text.contains("work:docs."), "{text}");
    let body: Value = serde_json::from_str(&out.render(true)).unwrap();
    let entry = |id: &str| {
        body["would_start"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["session"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(entry("int2")["tmux_target"], "work:api: build.1");
    assert_eq!(entry("int2")["tmux_pane"], 1);
    assert_eq!(entry("int3")["tmux_target"], "work:docs");
    assert_eq!(entry("int3")["tmux_pane"], Value::Null);
    assert!(!fx.server_has_sessions(), "a preview started tmux");
}

#[test]
fn a_started_session_reports_the_window_it_opened_in() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    place(&fx, "int1", "notes", "docs", "0");

    let out = json(run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(out["started"][0]["tmux_target"], "notes:docs");
}

#[test]
fn the_override_variable_puts_every_session_in_one_tmux_session() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    place(&fx, "int1", "one", "api", "0");
    place(&fx, "int2", "two", "web", "0");
    fx.vars.set("CLAUDE_MONITOR_TMUX_SESSION", "all-here");

    json(run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    assert_eq!(
        windows(&fx),
        [
            ("all-here".to_owned(), "api".to_owned()),
            ("all-here".to_owned(), "web".to_owned()),
        ]
    );
}

#[test]
fn a_session_without_a_placement_still_goes_to_the_agents_session() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    place(&fx, "int2", "notes", "docs", "0");

    json(run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    let rows = windows(&fx);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.contains(&("notes".to_owned(), "docs".to_owned())));
    assert!(rows.iter().any(|(session, _)| session == "agents"));
}

#[test]
fn an_existing_tmux_session_gains_the_window_instead_of_being_recreated() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    place(&fx, "int1", "notes", "docs", "0");
    assert!(fx
        .tmux(&["new-session", "-d", "-s", "notes", "-n", "mine"])
        .status
        .success());

    json(run_in(
        restore(true, false, &[], true),
        &fx.env,
        Some(&fx.state_dir),
    ));

    let rows = windows(&fx);
    assert!(rows.contains(&("notes".to_owned(), "mine".to_owned())));
    assert!(rows.contains(&("notes".to_owned(), "docs".to_owned())));
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
    assert_eq!(fx.pane_ids().len(), 1);
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
    assert_eq!(fx.pane_ids().len(), 1);
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
fn the_plan_shows_the_command_that_will_run() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    let plan = || {
        run_in(
            restore(true, false, &[], false),
            &fx.env,
            Some(&fx.state_dir),
        )
        .unwrap()
    };

    let out: Value = serde_json::from_str(&plan().render(true)).unwrap();
    let command = out["would_start"][0]["command"].as_str().unwrap();
    assert!(command.starts_with("cd "), "{command}");

    let text = plan().render(false);
    assert!(text.contains(&format!("  run: {command}")), "{text}");
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
