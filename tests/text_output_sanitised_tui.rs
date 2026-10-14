#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Guard for the dashboard: text from untrusted data must not reach a terminal
//! cell as a control character, a line or paragraph separator or a bidi
//! override. ratatui drops control characters itself; the rest is up to the
//! view. Every screen with session text is rendered once with a hostile
//! session; adding a screen is one line in `SCREENS`.

use std::collections::HashMap;
use std::path::PathBuf;

use claude_monitor::cli::detail::SessionDetail;
use claude_monitor::model::{State, StepStatus};
use claude_monitor::progress::binding::{Bound, ProgressView};
use claude_monitor::progress::{Item, PhaseCount, Progress, Source, SourceKind};
use claude_monitor::store::progress::{Binding, ChosenBy};
use claude_monitor::store::queries::{Overview, OverviewEntry, Section, SectionName};
use claude_monitor::tui::app::{update, App, Msg, Snapshot};
use claude_monitor::tui::restore::{PreviewRow, RestoreResult};
use claude_monitor::tui::view::view;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const NOW: &str = "2026-10-12T12:00:00Z";
const HOSTILE: &str = "\n2026 forged row\u{1b}[2J\u{1b}]0;x\u{7}\u{85}\u{2028}\u{2029}\u{202e}end";

fn hostile(base: &str) -> String {
    format!("{base}{HOSTILE}")
}

fn entry() -> OverviewEntry {
    OverviewEntry {
        id: hostile("abcd1234"),
        agent_kind: "claude".to_owned(),
        agent_session_id: "agent".to_owned(),
        repo: Some(hostile("repo")),
        branch: Some(hostile("branch")),
        cwd: hostile("/work"),
        state: State::Interrupted,
        running: false,
        last_activity_at: "2026-10-12T11:58:00Z".to_owned(),
        task_description: Some(hostile("task")),
        left_off: Some(hostile("left off")),
        project: Some(hostile("project")),
        flags: Vec::new(),
        section: SectionName::Interrupted,
        error: Some(hostile("error")),
    }
}

fn loaded() -> App {
    let entry = entry();
    let snapshot = Snapshot {
        overview: Overview {
            sections: vec![Section {
                name: SectionName::Interrupted,
                sessions: vec![entry],
            }],
        },
        panes: HashMap::new(),
        figures: HashMap::new(),
        now: NOW.to_owned(),
    };
    update(App::new(), Msg::Refreshed(snapshot)).0
}

fn key(app: App, code: KeyCode) -> App {
    update(app, Msg::Key(code, KeyModifiers::NONE)).0
}

fn item(text: &str, id: &str) -> Item {
    Item {
        ordinal: 0,
        text: hostile(text),
        status: StepStatus::Todo,
        group: Some(hostile("phase")),
        external_id: Some(hostile(id)),
    }
}

fn detail_app() -> App {
    let source = Source {
        kind: SourceKind::SpeckitTasks,
        path: Some(hostile("specs/tasks.md")),
    };
    let current = item("step", "T001");
    let progress = Progress {
        done: 0,
        total: 1,
        current: Some(current.clone()),
        phases: vec![PhaseCount {
            name: hostile("phase"),
            done: 0,
            total: 1,
        }],
        items: vec![current],
        source: source.clone(),
    };
    let bound = ProgressView::Bound(Box::new(Bound {
        binding: Binding {
            id: "b".to_owned(),
            owner_kind: claude_monitor::model::OwnerKind::Session,
            owner_id: hostile("abcd1234"),
            source,
            chosen_by: ChosenBy::Auto,
            read_at: NOW.to_owned(),
        },
        progress: Some(progress),
        stale: true,
    }));
    let detail = SessionDetail {
        id: hostile("abcd1234"),
        progress: bound,
        task_description: Some(hostile("task")),
        left_off: Some(hostile("left off")),
        generated: Vec::new(),
        links: Vec::new(),
        root: PathBuf::from(hostile("/work")),
    };
    let app = key(loaded(), KeyCode::Char('i'));
    update(app, Msg::Detail(detail)).0
}

fn restore_app() -> App {
    let app = key(loaded(), KeyCode::Char('R'));
    let rows = vec![
        PreviewRow::Start {
            session_id: hostile("abcd1234"),
            mode: "resume".to_owned(),
            cwd: hostile("/work"),
            tmux_target: hostile("tmux:window"),
            notices: vec![hostile("notice")],
        },
        PreviewRow::Refuse {
            session_id: hostile("efgh5678"),
            code: "missing_directory".to_owned(),
            message: hostile("message"),
            alternatives: vec![(hostile("description"), hostile("command"))],
        },
    ];
    update(app, Msg::RestorePreview(rows)).0
}

fn restore_results_app() -> App {
    let app = restore_app();
    let app = update(
        app,
        Msg::RestoreResult(RestoreResult::Refused {
            session_id: hostile("abcd1234"),
            code: "spawn_failed".to_owned(),
            message: hostile("message"),
        }),
    )
    .0;
    update(
        app,
        Msg::RestoreResult(RestoreResult::Started {
            session_id: hostile("efgh5678"),
            pane: hostile("%1"),
        }),
    )
    .0
}

struct Screen {
    name: &'static str,
    app: fn() -> App,
}

const SCREENS: &[Screen] = &[
    Screen {
        name: "overview",
        app: loaded,
    },
    Screen {
        name: "detail",
        app: detail_app,
    },
    Screen {
        name: "restore review",
        app: restore_app,
    },
    Screen {
        name: "restore results",
        app: restore_results_app,
    },
];

fn cells(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
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

#[test]
fn no_dashboard_screen_draws_a_hostile_session_as_terminal_control() {
    let mut failures = Vec::new();
    for screen in SCREENS {
        let text = cells(&(screen.app)());
        if let Some(c) = text.chars().find(|&c| {
            (c.is_control() && c != '\n')
                || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        }) {
            failures.push(format!("{}: {c:?} was drawn", screen.name));
        }
        if !text.contains("end") {
            failures.push(format!(
                "{}: the hostile session did not appear",
                screen.name
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
