#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The TUI detail page and the overview rows' compact progress figure (T093,
//! FR-015, FR-015a, FR-017a).

use std::collections::HashMap;
use std::path::PathBuf;

use claude_monitor::cli::detail::SessionDetail;
use claude_monitor::model::{OwnerKind, State, StepStatus};
use claude_monitor::progress::binding::{Bound, Candidate, CandidateReason, ProgressView};
use claude_monitor::progress::{Item, PhaseCount, Progress, Source, SourceKind};
use claude_monitor::store::progress::{Binding, ChosenBy};
use claude_monitor::store::queries::{Overview, OverviewEntry, Section, SectionName};
use claude_monitor::store::summaries::Summary;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::tui::app::{update, App, Effect, Msg, Snapshot};
use claude_monitor::tui::view::view;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const EARLIER: &str = "2026-10-12T11:58:00Z";
const PLAN: &str = "specs/001-x/tasks.md";

fn entry(id: &str) -> OverviewEntry {
    OverviewEntry {
        id: id.to_owned(),
        agent_kind: "claude".to_owned(),
        agent_session_id: format!("agent-{id}"),
        repo: Some("nix-config".to_owned()),
        branch: Some("main".to_owned()),
        cwd: "/work/nix-config".to_owned(),
        state: State::Idle,
        running: false,
        last_activity_at: EARLIER.to_owned(),
        task_description: Some(format!("task of {id}")),
        left_off: None,
        project: None,
        flags: Vec::new(),
        section: SectionName::Active,
        error: None,
    }
}

fn snapshot(ids: &[&str], figures: &[(&str, (u32, u32))]) -> Snapshot {
    Snapshot {
        overview: Overview {
            sections: vec![Section {
                name: SectionName::Active,
                sessions: ids.iter().map(|id| entry(id)).collect(),
            }],
        },
        panes: HashMap::new(),
        figures: figures
            .iter()
            .map(|(id, figure)| ((*id).to_owned(), *figure))
            .collect(),
        now: NOW.to_owned(),
    }
}

fn loaded(ids: &[&str], figures: &[(&str, (u32, u32))]) -> App {
    update(App::new(), Msg::Refreshed(snapshot(ids, figures))).0
}

fn press(app: App, code: KeyCode) -> (App, Vec<Effect>) {
    update(app, Msg::Key(code, KeyModifiers::NONE))
}

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
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

fn item(ordinal: u32, text: &str, status: StepStatus, group: &str, id: &str) -> Item {
    Item {
        ordinal,
        text: text.to_owned(),
        status,
        group: Some(group.to_owned()),
        external_id: Some(id.to_owned()),
    }
}

fn bound(stale: bool) -> ProgressView {
    let source = Source {
        kind: SourceKind::SpeckitTasks,
        path: Some(PLAN.to_owned()),
    };
    let items = vec![
        item(0, "first", StepStatus::Done, "Phase 1: Setup", "T001"),
        item(1, "second", StepStatus::Doing, "Phase 1: Setup", "T002"),
        item(2, "third", StepStatus::Todo, "Phase 2: Build", "T003"),
        item(3, "fourth", StepStatus::Todo, "Phase 2: Build", "T004"),
    ];
    let progress = Progress {
        done: 1,
        total: 4,
        current: items.get(1).cloned(),
        phases: vec![
            PhaseCount {
                name: "Phase 1: Setup".to_owned(),
                done: 1,
                total: 2,
            },
            PhaseCount {
                name: "Phase 2: Build".to_owned(),
                done: 0,
                total: 2,
            },
        ],
        items,
        source: source.clone(),
    };
    ProgressView::Bound(Box::new(Bound {
        binding: Binding {
            id: "b1".to_owned(),
            owner_kind: OwnerKind::Session,
            owner_id: "s1".to_owned(),
            source,
            chosen_by: ChosenBy::Auto,
            read_at: EARLIER.to_owned(),
        },
        progress: Some(progress),
        stale,
    }))
}

fn generated(body: &str) -> Summary {
    Summary {
        id: "m1".to_owned(),
        owner_kind: "session".to_owned(),
        owner_id: "s1".to_owned(),
        body: body.to_owned(),
        model: Some("qwen-9b".to_owned()),
        model_written: true,
        created_at: EARLIER.to_owned(),
    }
}

fn detail(progress: ProgressView) -> SessionDetail {
    SessionDetail {
        links: Vec::new(),
        id: "s1".to_owned(),
        progress,
        task_description: Some("fix the build".to_owned()),
        left_off: Some("tests are green".to_owned()),
        generated: vec![generated("Fixing CI flakes.")],
        root: PathBuf::from("/work/nix-config"),
    }
}

fn opened(progress: ProgressView) -> App {
    let (app, _) = press(loaded(&["s1"], &[("s1", (1, 4))]), KeyCode::Char('i'));
    update(app, Msg::Detail(detail(progress))).0
}

mod rows {
    use super::*;

    #[test]
    fn the_store_gives_a_figure_per_bound_session_from_stored_steps() {
        use claude_monitor::model::Session;
        let dir = TempDir::new().unwrap();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::viewer()).unwrap();
        let session = Session::new("s1", "claude", "agent-s1", "/work", EARLIER);
        store.upsert_session(&session).unwrap();
        let source = Source {
            kind: SourceKind::SpeckitTasks,
            path: Some(PLAN.to_owned()),
        };
        let binding = store
            .bind_progress(OwnerKind::Session, "s1", &source, ChosenBy::Auto, EARLIER)
            .unwrap();
        let items = vec![
            item(0, "a", StepStatus::Done, "P", "T001"),
            item(1, "b", StepStatus::Todo, "P", "T002"),
            item(2, "c", StepStatus::Done, "P", "T003"),
        ];
        store.replace_steps(&binding.id, &items).unwrap();
        let other = Session::new("s2", "claude", "agent-s2", "/work", EARLIER);
        store.upsert_session(&other).unwrap();

        let snapshot = Snapshot::load(&store, NOW).unwrap();

        assert_eq!(snapshot.figures.get("s1"), Some(&(2, 3)));
        assert!(!snapshot.figures.contains_key("s2"));
    }

    #[test]
    fn a_bound_row_shows_a_compact_figure() {
        let app = loaded(&["s1", "s2"], &[("s1", (4, 10))]);
        let text = render(&app);
        let s1 = text.lines().find(|l| l.contains("task of s1")).unwrap();
        let s2 = text.lines().find(|l| l.contains("task of s2")).unwrap();
        assert!(s1.contains("4/10"), "{text}");
        assert!(!s2.contains("4/10"), "{text}");
    }
}

mod page {
    use super::*;

    #[test]
    fn i_asks_for_the_selected_sessions_detail_and_shows_loading() {
        let (app, effects) = press(loaded(&["s1", "s2"], &[]), KeyCode::Char('i'));
        assert_eq!(
            effects,
            vec![Effect::LoadDetail {
                id: "s1".to_owned()
            }]
        );
        assert_eq!(app.detail().unwrap().id(), "s1");
        assert!(app.detail().unwrap().detail().is_none());
        assert!(render(&app).contains("Loading"));
    }

    #[test]
    fn i_with_no_sessions_does_nothing() {
        let (app, effects) = press(loaded(&[], &[]), KeyCode::Char('i'));
        assert!(effects.is_empty());
        assert!(app.detail().is_none());
    }

    #[test]
    fn the_page_shows_the_figure_bar_source_current_item_and_phase_counts() {
        let text = render(&opened(bound(false)));
        assert!(text.contains("1 of 4"), "{text}");
        assert!(text.contains('#') || text.contains('█'), "{text}");
        assert!(text.contains(&format!("{PLAN} (spec-kit tasks)")), "{text}");
        assert!(text.contains("T002 second"), "{text}");
        assert!(text.contains("Phase 1: Setup 1/2"), "{text}");
        assert!(text.contains("Phase 2: Build 0/2"), "{text}");
        assert!(!text.contains("stale"), "{text}");
    }

    #[test]
    fn a_stale_figure_is_marked() {
        assert!(render(&opened(bound(true))).contains("stale"));
    }

    #[test]
    fn verbatim_and_generated_summaries_are_apart_and_the_generated_one_is_labelled() {
        let text = render(&opened(bound(false)));
        let task = text.find("fix the build").expect(&text);
        let left = text.find("tests are green").expect(&text);
        let label = text.find("generated (qwen-9b").expect(&text);
        assert!(task < label && left < label, "{text}");
        assert!(text.contains("Fixing CI flakes."), "{text}");
    }

    #[test]
    fn pick_one_shows_the_candidates_and_the_bind_hint() {
        let candidates = vec![
            Candidate {
                path: PathBuf::from("/work/nix-config/a.md"),
                kind: SourceKind::MarkdownChecklist,
                reason: CandidateReason::Checklist,
                done: 1,
                total: 2,
            },
            Candidate {
                path: PathBuf::from("/work/nix-config/b.md"),
                kind: SourceKind::MarkdownChecklist,
                reason: CandidateReason::Checklist,
                done: 0,
                total: 1,
            },
        ];
        let text = render(&opened(ProgressView::PickOne(candidates)));
        assert!(text.contains("progress bind s1"), "{text}");
        assert!(text.contains("a.md"), "{text}");
        assert!(text.contains("b.md"), "{text}");
        assert!(!text.contains("1 of 4"), "{text}");
    }

    #[test]
    fn no_plan_shows_no_progress_block_but_still_the_summaries() {
        let text = render(&opened(ProgressView::None));
        assert!(!text.contains("progress "), "{text}");
        assert!(text.contains("fix the build"), "{text}");
    }

    #[test]
    fn esc_closes_the_page_and_a_second_esc_quits() {
        let (app, effects) = press(opened(bound(false)), KeyCode::Esc);
        assert!(effects.is_empty());
        assert!(app.detail().is_none());
        let (_, effects) = press(app, KeyCode::Esc);
        assert_eq!(effects, vec![Effect::Quit]);
    }

    #[test]
    fn row_keys_do_nothing_while_the_page_is_open() {
        let app = {
            let (app, _) = press(loaded(&["s1", "s2"], &[]), KeyCode::Char('i'));
            update(app, Msg::Detail(detail(bound(false)))).0
        };
        for code in [KeyCode::Char('j'), KeyCode::Enter, KeyCode::Char('d')] {
            let (next, effects) = press(app.clone(), code);
            assert!(effects.is_empty(), "{code:?}");
            assert_eq!(next.selected_id(), Some("s1"), "{code:?}");
            assert_eq!(next.visible().len(), 2, "{code:?}");
        }
    }

    #[test]
    fn a_refresh_while_open_asks_for_the_detail_again() {
        let (_, effects) = update(
            opened(bound(false)),
            Msg::Refreshed(snapshot(&["s1"], &[("s1", (2, 4))])),
        );
        assert!(effects.contains(&Effect::LoadDetail {
            id: "s1".to_owned()
        }));
    }

    #[test]
    fn a_detail_for_another_session_is_dropped() {
        let (app, _) = press(loaded(&["s1"], &[]), KeyCode::Char('i'));
        let mut other = detail(bound(false));
        other.id = "s9".to_owned();
        let (app, _) = update(app, Msg::Detail(other));
        assert!(app.detail().unwrap().detail().is_none());
    }
}
