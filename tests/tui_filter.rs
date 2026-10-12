#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The dashboard's filter model (T111, FR-019).

use claude_monitor::model::State;
use claude_monitor::store::queries::{OverviewEntry, SectionName};
use claude_monitor::tui::filter::Filter;

fn entry(id: &str, agent: &str, state: State, task: &str) -> OverviewEntry {
    OverviewEntry {
        id: id.to_owned(),
        agent_kind: agent.to_owned(),
        agent_session_id: format!("agent-{id}"),
        repo: Some("nix-config".to_owned()),
        branch: Some("feature-x".to_owned()),
        cwd: "/work/nix-config".to_owned(),
        state,
        running: false,
        last_activity_at: "2026-10-12T11:58:00Z".to_owned(),
        task_description: Some(task.to_owned()),
        left_off: None,
        project: Some("Platform".to_owned()),
        flags: Vec::new(),
        section: SectionName::Active,
        error: None,
    }
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn an_untouched_filter_matches_everything_and_narrows_nothing() {
    let filter = Filter::default();

    assert!(filter.matches(&entry("s1", "claude", State::Idle, "x")));
    assert!(!filter.narrows());
    assert_eq!(filter.summary(), None);
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn text_matches_task_cwd_repo_branch_and_project_ignoring_case() {
    let mut filter = Filter::default();
    let row = entry("s1", "claude", State::Idle, "Fix the BUILD");

    for needle in ["build", "WORK/nix", "nix-config", "feature", "platform"] {
        filter.set_text(needle.to_owned());
        assert!(filter.matches(&row), "{needle}");
    }
    filter.set_text("nothing like this".to_owned());
    assert!(!filter.matches(&row));
    assert!(filter.narrows());
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn a_row_with_no_task_does_not_break_the_text_match() {
    let mut row = entry("s1", "claude", State::Idle, "x");
    row.task_description = None;
    let mut filter = Filter::default();
    filter.set_text("work".to_owned());

    assert!(filter.matches(&row));
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn the_state_filter_cycles_through_every_state_and_back_to_none() {
    let mut filter = Filter::default();
    let mut seen = Vec::new();

    for _ in 0..7 {
        filter.cycle_state();
        seen.push(filter.state());
    }

    assert_eq!(
        seen,
        vec![
            Some(State::NeedsPermission),
            Some(State::Idle),
            Some(State::Working),
            Some(State::Finished),
            Some(State::Interrupted),
            Some(State::Unknown),
            None,
        ]
    );
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn the_state_filter_keeps_only_that_state() {
    let mut filter = Filter::default();
    filter.cycle_state();
    filter.cycle_state();

    assert_eq!(filter.state(), Some(State::Idle));
    assert!(filter.matches(&entry("s1", "claude", State::Idle, "x")));
    assert!(!filter.matches(&entry("s2", "claude", State::Working, "x")));
    assert!(filter.narrows());
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn the_agent_filter_cycles_through_the_kinds_present_and_back_to_none() {
    let kinds = vec!["claude".to_owned(), "copilot".to_owned()];
    let mut filter = Filter::default();
    let mut seen = Vec::new();

    for _ in 0..3 {
        filter.cycle_agent(&kinds);
        seen.push(filter.agent().map(str::to_owned));
    }

    assert_eq!(
        seen,
        vec![Some("claude".to_owned()), Some("copilot".to_owned()), None]
    );
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn the_agent_filter_keeps_only_that_agent_and_resets_when_the_kind_is_gone() {
    let mut filter = Filter::default();
    filter.cycle_agent(&["claude".to_owned(), "copilot".to_owned()]);
    filter.cycle_agent(&["claude".to_owned(), "copilot".to_owned()]);

    assert!(filter.matches(&entry("s1", "copilot", State::Idle, "x")));
    assert!(!filter.matches(&entry("s2", "claude", State::Idle, "x")));

    filter.cycle_agent(&["claude".to_owned()]);
    assert_eq!(filter.agent(), Some("claude"));
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn cycling_the_agent_with_no_kinds_stays_on_none() {
    let mut filter = Filter::default();

    filter.cycle_agent(&[]);

    assert_eq!(filter.agent(), None);
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn the_dismissed_toggle_does_not_narrow_and_flips() {
    let mut filter = Filter::default();
    assert!(!filter.show_dismissed());

    filter.toggle_dismissed();

    assert!(filter.show_dismissed());
    assert!(!filter.narrows());
    filter.toggle_dismissed();
    assert!(!filter.show_dismissed());
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn the_summary_names_every_active_filter() {
    let mut filter = Filter::default();
    filter.cycle_state();
    filter.cycle_state();
    filter.cycle_agent(&["claude".to_owned()]);
    filter.set_text("build".to_owned());

    assert_eq!(
        filter.summary().as_deref(),
        Some("state idle · agent claude · /build")
    );
}
