//! SC-011: every user-facing library command is offered by the web view, or is
//! on the exclusion list below with its reason. Adding a command to the CLI
//! without a web route (or an exclusion) fails here.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod web_support;

use std::collections::BTreeSet;

use claude_monitor::cli::Command;
use claude_monitor::web::routes::COMMAND_ROUTES;
use web_support::{Request, Rig};

/// Commands the web view does not offer, and why.
const EXCLUDED: &[(&str, &str)] = &[
    (
        "dashboard",
        "the terminal UI itself; the web view is its counterpart",
    ),
    (
        "hook",
        "called by the agents on every event, never by a person",
    ),
    (
        "reconcile",
        "entry point of the tmux pane hooks, not a user action",
    ),
    (
        "watch",
        "long-running notifier process, nothing to show in a page",
    ),
    (
        "boot-check",
        "runs from the login unit; the page shows its result as the restore list",
    ),
    (
        "migrate",
        "one-off import of the legacy runtime files, CLI only",
    ),
    ("serve", "starts the web view itself"),
    (
        "export-cortexiphan",
        "writes a file for another system, CLI only",
    ),
    (
        "delete",
        "destructive and unscheduled, CLI only behind --yes",
    ),
];

fn excluded(name: &str) -> bool {
    EXCLUDED.iter().any(|(excluded, _)| *excluded == name)
}

fn probe_path(path: &str) -> String {
    path.replace("{id}", "x")
}

#[test]
fn every_command_is_mapped_to_a_route_or_explicitly_excluded() {
    let routed: BTreeSet<&str> = COMMAND_ROUTES.iter().map(|r| r.command).collect();
    let mut missing = Vec::new();
    for info in Command::all() {
        if !routed.contains(info.name) && !excluded(info.name) {
            missing.push(info.name);
        }
    }

    assert!(
        missing.is_empty(),
        "commands with neither a web route nor an exclusion: {missing:?}"
    );
}

#[test]
fn the_exclusion_list_is_honest() {
    let known: BTreeSet<&str> = Command::all().iter().map(|i| i.name).collect();
    let routed: BTreeSet<&str> = COMMAND_ROUTES.iter().map(|r| r.command).collect();

    for (name, reason) in EXCLUDED {
        assert!(known.contains(name), "{name} is not a command any more");
        assert!(!reason.is_empty(), "{name} has no reason");
        assert!(!routed.contains(name), "{name} is both routed and excluded");
    }
    for route in COMMAND_ROUTES {
        assert!(
            known.contains(route.command),
            "{} is routed but is not a command",
            route.command
        );
    }
}

#[test]
fn a_command_with_a_tui_action_is_never_excluded() {
    for info in Command::all() {
        if info.name != "dashboard" && info.is_interactive() {
            assert!(
                !excluded(info.name),
                "{} has a TUI action ({:?}) so the web view must offer it",
                info.name,
                info.tui_action
            );
        }
    }
}

#[test]
fn every_mapped_route_exists_with_its_method() {
    let rig = Rig::start();
    for route in COMMAND_ROUTES {
        assert!(
            route.method == "GET" || route.method == "POST",
            "{route:?}: only reads and writes"
        );
        let path = probe_path(route.path);
        // PUT is registered nowhere, so a real route answers 405 and an unknown
        // one 404; no handler runs.
        let response = Request::new("PUT", &path)
            .header("Cookie", rig.cookie())
            .header("X-CC-Token", rig.csrf())
            .send(rig.addr());
        assert_eq!(response.status, 405, "{route:?} is not a route");
    }
}

#[test]
fn every_write_route_needs_the_csrf_header() {
    let rig = Rig::start();
    for route in COMMAND_ROUTES.iter().filter(|r| r.method == "POST") {
        let path = probe_path(route.path);
        let response = Request::new("POST", &path)
            .header("Cookie", rig.cookie())
            .send(rig.addr());
        assert_eq!(
            response.status, 403,
            "{route:?} accepted a POST without X-CC-Token"
        );
    }
}

#[test]
fn the_projects_page_redirects_to_the_overview_but_creating_a_project_still_posts_there() {
    let rig = Rig::start();

    let read = Request::get("/projects")
        .header("Cookie", rig.cookie())
        .send(rig.addr());
    let create = Request::new("POST", "/projects")
        .signed_in(&rig)
        .json_body(&serde_json::json!({"name": "alpha"}))
        .send(rig.addr());

    assert_eq!(read.status, 302);
    assert_eq!(read.header("location"), Some("/"));
    assert_eq!(create.status, 200, "{}", create.body);
}
