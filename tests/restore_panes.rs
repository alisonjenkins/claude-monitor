//! End to end on a private tmux server: sessions that shared one window as
//! split panes come back as panes of one window, in their directories.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::time::{Duration, Instant};

use claude_monitor::cli::{run_in, Command};
use claude_monitor::model::State;
use common::Fixture;
use serde_json::Value;

fn tmux_ok(fx: &Fixture, args: &[&str]) -> String {
    let out = fx.tmux(args);
    assert!(
        out.status.success(),
        "tmux {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// `(pane_index, path)` of the panes of the one window, once every pane
/// reports its directory (tmux reads it from the child, which may lag).
fn panes_with_paths(fx: &Fixture, wanted: &[&str]) -> Vec<(String, String)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let listing = tmux_ok(
            fx,
            &[
                "list-panes",
                "-a",
                "-F",
                "#{pane_index} #{pane_current_path}",
            ],
        );
        let mut rows: Vec<(String, String)> = listing
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(index, path)| (index.to_owned(), path.to_owned()))
            .collect();
        rows.sort();
        let paths: Vec<&str> = rows.iter().map(|(_, path)| path.as_str()).collect();
        if paths == wanted || Instant::now() > deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn two_sessions_that_shared_a_window_come_back_as_two_panes_of_one_window() {
    let Some(fx) = Fixture::new() else { return };
    let a = fx.add_session("int1", State::Interrupted, "a");
    let b = fx.add_session("int2", State::Interrupted, "b");
    tmux_ok(
        &fx,
        &[
            "new-session",
            "-d",
            "-s",
            "my work",
            "-n",
            "api: build",
            "-c",
            &a.cwd,
            "sleep 300",
        ],
    );
    tmux_ok(
        &fx,
        &[
            "split-window",
            "-d",
            "-t",
            "my work:api: build",
            "-c",
            &b.cwd,
            "sleep 300",
        ],
    );
    let panes = tmux_ok(
        &fx,
        &["list-panes", "-t", "my work:api: build", "-F", "#{pane_id}"],
    );
    let panes: Vec<&str> = panes.lines().collect();
    assert_eq!(panes.len(), 2, "{panes:?}");
    for (id, pane) in ["int1", "int2"].into_iter().zip(&panes) {
        let location = fx.env.tmux.pane_location(pane).unwrap().unwrap();
        let mut session = fx.store().get_session(id).unwrap().unwrap();
        assert!(session.record_tmux_location(&location));
        fx.save(&session);
    }
    // Unequal on purpose: a tiled or default split would not reproduce them.
    tmux_ok(&fx, &["resize-pane", "-t", panes[0], "-y", "7"]);
    for (id, pane) in ["int1", "int2"].into_iter().zip(&panes) {
        let location = fx.env.tmux.pane_location(pane).unwrap().unwrap();
        let mut session = fx.store().get_session(id).unwrap().unwrap();
        session.record_tmux_location(&location);
        fx.save(&session);
    }
    let heights = tmux_ok(&fx, &["list-panes", "-F", "#{pane_height}"]);
    assert_eq!(heights.lines().next(), Some("7"), "{heights}");
    tmux_ok(&fx, &["kill-server"]);
    assert!(!fx.server_has_sessions(), "the server should be gone");

    let out = run_in(
        Command::Restore {
            all_interrupted: true,
            include_finished: false,
            fresh: false,
            skip: Vec::new(),
            yes: true,
        },
        &fx.env,
        Some(&fx.state_dir),
    )
    .unwrap();

    let body: Value = serde_json::from_str(&out.render(true)).unwrap();
    let targets: Vec<_> = body["started"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["tmux_target"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(targets.len(), 2, "{body}");
    assert!(targets[0].starts_with("my work:api: build."), "{targets:?}");
    assert_eq!(
        tmux_ok(&fx, &["list-windows", "-a", "-F", "#{window_id}"])
            .lines()
            .count(),
        1,
        "one window, no placeholder"
    );
    assert_eq!(
        tmux_ok(&fx, &["list-sessions", "-F", "#{session_name}"]),
        "my work"
    );
    let rows = panes_with_paths(&fx, &[&a.cwd, &b.cwd]);
    assert_eq!(
        rows.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>(),
        [a.cwd.as_str(), b.cwd.as_str()],
        "panes in recorded order with their own directories"
    );
    assert_eq!(
        tmux_ok(&fx, &["list-panes", "-F", "#{pane_height}"]),
        heights,
        "all original panes came back, so the recorded layout is applied"
    );
}
