#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Project grouping and filters in the dashboard (T111, FR-019, FR-019b).

use std::collections::HashMap;

use claude_monitor::model::State;
use claude_monitor::store::queries::{Overview, OverviewEntry, Section, SectionName};
use claude_monitor::tui::app::{update, App, Effect, Msg, Prompt, Snapshot};
use claude_monitor::tui::view::view;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const NOW: &str = "2026-10-12T12:00:00Z";
const EARLIER: &str = "2026-10-12T11:58:00Z";

struct Row {
    id: &'static str,
    project: Option<&'static str>,
    section: SectionName,
    state: State,
    agent: &'static str,
    task: &'static str,
}

fn row(id: &'static str, project: Option<&'static str>, section: SectionName) -> Row {
    let state = match section {
        SectionName::Finished | SectionName::Done => State::Finished,
        SectionName::Interrupted => State::Interrupted,
        SectionName::Unseen => State::Unknown,
        SectionName::Active | SectionName::Dismissed => State::Idle,
    };
    Row {
        id,
        project,
        section,
        state,
        agent: "claude",
        task: "some work",
    }
}

fn entry(row: &Row) -> OverviewEntry {
    OverviewEntry {
        id: row.id.to_owned(),
        agent_kind: row.agent.to_owned(),
        agent_session_id: format!("agent-{}", row.id),
        repo: Some("nix-config".to_owned()),
        branch: None,
        cwd: "/work/nix-config".to_owned(),
        state: row.state,
        running: false,
        last_activity_at: EARLIER.to_owned(),
        task_description: Some(row.task.to_owned()),
        left_off: None,
        project: row.project.map(str::to_owned),
        flags: Vec::new(),
        section: row.section,
        error: None,
    }
}

fn snapshot(rows: &[Row]) -> Snapshot {
    let names = [
        SectionName::Active,
        SectionName::Interrupted,
        SectionName::Finished,
        SectionName::Done,
        SectionName::Unseen,
        SectionName::Dismissed,
    ];
    Snapshot {
        overview: Overview {
            sections: names
                .into_iter()
                .map(|name| Section {
                    name,
                    sessions: rows
                        .iter()
                        .filter(|r| r.section == name)
                        .map(entry)
                        .collect(),
                })
                .collect(),
        },
        panes: HashMap::new(),
        figures: HashMap::new(),
        now: NOW.to_owned(),
    }
}

fn loaded(rows: &[Row]) -> App {
    update(App::new(), Msg::Refreshed(snapshot(rows))).0
}

fn press(app: App, code: KeyCode) -> (App, Vec<Effect>) {
    update(app, Msg::Key(code, KeyModifiers::NONE))
}

fn keys(mut app: App, chars: &str) -> App {
    for c in chars.chars() {
        app = press(app, KeyCode::Char(c)).0;
    }
    app
}

fn ids(app: &App) -> Vec<String> {
    app.visible().iter().map(|e| e.id.clone()).collect()
}

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(130, 40)).unwrap();
    terminal.draw(|frame| view(app, frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn projects_rows() -> Vec<Row> {
    vec![
        row("b-active", Some("Beta"), SectionName::Active),
        row("none-active", None, SectionName::Active),
        row("a-active", Some("Alpha"), SectionName::Active),
        row("b-done", Some("Beta"), SectionName::Done),
        row("none-done", None, SectionName::Done),
        row("a-finished", Some("Alpha"), SectionName::Finished),
    ]
}

mod grouping {
    use super::*;

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn p_groups_rows_under_their_project_and_p_again_restores_the_sections() {
        let app = loaded(&projects_rows());
        assert!(!app.grouped());
        assert!(render(&app).contains("Active (3)"));

        let app = keys(app, "p");
        let text = render(&app);

        assert!(app.grouped());
        assert!(text.contains("Alpha · Active (1)"), "{text}");
        assert!(text.contains("Alpha · Finished (1)"), "{text}");
        assert!(text.contains("Beta · Done (1)"), "{text}");
        assert!(text.contains("No project · Active (1)"), "{text}");
        assert!(text.contains("No project · Done (1)"), "{text}");

        let app = keys(app, "p");
        assert!(!app.grouped());
        assert!(render(&app).contains("Active (3)"));
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn navigation_follows_the_grouped_display_order() {
        let app = keys(loaded(&projects_rows()), "p");

        assert_eq!(
            ids(&app),
            [
                "a-active",
                "a-finished",
                "b-active",
                "b-done",
                "none-active",
                "none-done"
            ]
        );
        let app = keys(app, "j");
        assert_eq!(app.selected_id(), Some("a-finished"));
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn the_selection_stays_on_the_same_session_across_the_toggle() {
        let app = keys(loaded(&projects_rows()), "jjj");
        let before = app.selected_id().map(str::to_owned);

        let app = keys(app, "p");

        assert_eq!(app.selected_id().map(str::to_owned), before);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn unseen_and_dismissed_stay_global_in_the_grouped_view() {
        let mut rows = projects_rows();
        rows.push(row("seen-not", Some("Alpha"), SectionName::Unseen));
        let app = keys(loaded(&rows), "pu");

        let text = render(&app);

        assert!(text.contains("Unseen (1)"), "{text}");
        assert!(!text.contains("Alpha · Unseen"), "{text}");
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn the_title_says_when_the_view_is_grouped() {
        let text = render(&keys(loaded(&projects_rows()), "p"));

        assert!(text.contains("by project"), "{text}");
    }
}

mod filters {
    use super::*;

    fn mixed() -> Vec<Row> {
        let mut rows = vec![
            row("fix-build", None, SectionName::Active),
            row("write-docs", None, SectionName::Active),
            row("old", None, SectionName::Finished),
        ];
        rows[0].task = "fix the build";
        rows[1].task = "write the docs";
        rows[1].agent = "copilot";
        rows[1].state = State::Working;
        rows
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn slash_opens_a_prompt_and_typing_narrows_the_list_as_it_goes() {
        let app = keys(loaded(&mixed()), "/");
        assert_eq!(app.prompt(), Some(&Prompt::Filter));

        let app = keys(app, "bui");

        assert_eq!(ids(&app), ["fix-build"]);
        assert_eq!(app.filter().text(), "bui");
        assert!(render(&app).contains("/bui"), "{}", render(&app));
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn the_prompt_takes_every_letter_including_command_keys() {
        let (app, effects) = press(loaded(&mixed()), KeyCode::Char('/'));

        let (app, effects_q) = press(app, KeyCode::Char('q'));
        let app = keys(app, "dpfaSDUxnL");

        assert!(effects.is_empty() && effects_q.is_empty());
        assert_eq!(app.filter().text(), "qdpfaSDUxnL");
        assert!(!app.grouped());
        assert_eq!(app.filter().state(), None);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn backspace_widens_and_enter_keeps_the_filter_and_closes_the_prompt() {
        let app = keys(loaded(&mixed()), "/docs");
        assert_eq!(ids(&app), ["write-docs"]);

        let (app, _) = press(app, KeyCode::Backspace);
        assert_eq!(app.filter().text(), "doc");
        let (app, effects) = press(app, KeyCode::Enter);

        assert!(effects.is_empty(), "Enter must not jump: {effects:?}");
        assert_eq!(app.prompt(), None);
        assert_eq!(app.filter().text(), "doc");
        assert_eq!(ids(&app), ["write-docs"]);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn esc_in_the_prompt_clears_the_text_and_closes_it_without_quitting() {
        let app = keys(loaded(&mixed()), "/docs");

        let (app, effects) = press(app, KeyCode::Esc);

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.prompt(), None);
        assert_eq!(app.filter().text(), "");
        assert_eq!(ids(&app).len(), 3);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn f_cycles_the_state_filter_and_the_title_names_it() {
        let app = keys(loaded(&mixed()), "f");
        assert_eq!(app.filter().state(), Some(State::NeedsPermission));
        assert!(ids(&app).is_empty());

        let app = keys(app, "f");
        assert_eq!(app.filter().state(), Some(State::Idle));
        assert_eq!(ids(&app), ["fix-build"]);
        assert!(render(&app).contains("state idle"), "{}", render(&app));

        let app = keys(app, "ff");
        assert_eq!(app.filter().state(), Some(State::Finished));
        assert_eq!(ids(&app), ["old"]);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn a_cycles_the_agent_filter_over_the_kinds_present() {
        let app = keys(loaded(&mixed()), "a");
        assert_eq!(app.filter().agent(), Some("claude"));
        assert_eq!(ids(&app), ["fix-build", "old"]);

        let app = keys(app, "a");
        assert_eq!(app.filter().agent(), Some("copilot"));
        assert_eq!(ids(&app), ["write-docs"]);

        let app = keys(app, "a");
        assert_eq!(app.filter().agent(), None);
        assert_eq!(ids(&app).len(), 3);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn filters_combine() {
        let app = keys(loaded(&mixed()), "/the");
        let (app, _) = press(app, KeyCode::Enter);
        let app = keys(app, "a");

        assert_eq!(ids(&app), ["fix-build"]);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn a_filter_that_hides_the_selection_moves_it_to_a_visible_row() {
        let app = keys(loaded(&mixed()), "j");
        assert_eq!(app.selected_id(), Some("write-docs"));

        let app = keys(app, "/build");

        assert_eq!(app.selected_id(), Some("fix-build"));
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn nothing_matching_says_so_and_the_counts_follow_the_filter() {
        let app = keys(loaded(&mixed()), "/zzz");

        let text = render(&app);

        assert!(text.contains("No sessions match"), "{text}");
        assert!(text.contains("0 working"), "{text}");
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn a_filter_never_sends_a_notification_for_rows_it_reveals() {
        let app = keys(loaded(&mixed()), "/zzz");
        let (app, _) = press(app, KeyCode::Esc);

        let (_, effects) = update(app, Msg::Refreshed(snapshot(&mixed())));

        assert!(effects.is_empty(), "{effects:?}");
    }
}

mod dismissed {
    use super::*;

    fn with_dismissed() -> Vec<Row> {
        vec![
            row("live", None, SectionName::Active),
            row("gone", None, SectionName::Dismissed),
        ]
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn dismissed_sessions_are_hidden_until_s_shows_them() {
        let app = loaded(&with_dismissed());
        assert_eq!(ids(&app), ["live"]);
        assert!(!render(&app).contains("Dismissed"));

        let app = keys(app, "S");

        assert!(app.filter().show_dismissed());
        assert_eq!(ids(&app), ["live", "gone"]);
        assert!(render(&app).contains("Dismissed (1)"), "{}", render(&app));

        let app = keys(app, "S");
        assert_eq!(ids(&app), ["live"]);
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn a_dismissed_session_changing_state_does_not_notify() {
        let app = loaded(&with_dismissed());
        let mut rows = with_dismissed();
        rows[1].state = State::NeedsPermission;

        let (_, effects) = update(app, Msg::Refreshed(snapshot(&rows)));

        assert!(effects.is_empty(), "{effects:?}");
    }

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn hiding_the_selected_session_with_s_moves_the_selection() {
        let app = keys(loaded(&with_dismissed()), "Sj");
        assert_eq!(app.selected_id(), Some("gone"));

        let app = keys(app, "S");

        assert_eq!(app.selected_id(), Some("live"));
    }
}

mod help {
    use super::*;

    #[test]
    #[ignore = "T111: implemented in the next commit"]
    fn the_help_line_lists_the_new_keys() {
        let text = render(&loaded(&projects_rows()));

        for needle in [
            "p projects",
            "/ filter",
            "f state",
            "a agent",
            "S dismissed",
        ] {
            assert!(text.contains(needle), "missing {needle:?}: {text}");
        }
    }
}
