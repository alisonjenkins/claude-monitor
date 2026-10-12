#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Organise keys in the dashboard: D done, U undone, x dismiss, n note
//! (T111, FR-018, FR-019b, FR-019c).

use std::collections::HashMap;

use claude_monitor::cli::{run_in, Command, ProjectAction};
use claude_monitor::model::{Author, OwnerKind, Session, State};
use claude_monitor::store::queries::{Overview, OverviewEntry, Section, SectionName};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::tui::app::{update, App, Effect, Msg, Prompt, Snapshot};
use claude_monitor::tui::organise::{self, Organise};
use claude_monitor::tui::view::view;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const EARLIER: &str = "2026-10-12T11:58:00Z";

fn press(app: App, code: KeyCode) -> (App, Vec<Effect>) {
    update(app, Msg::Key(code, KeyModifiers::NONE))
}

fn keys(mut app: App, chars: &str) -> App {
    for c in chars.chars() {
        app = press(app, KeyCode::Char(c)).0;
    }
    app
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

fn entry(id: &str, section: SectionName) -> OverviewEntry {
    OverviewEntry {
        id: id.to_owned(),
        agent_kind: "claude".to_owned(),
        agent_session_id: format!("agent-{id}"),
        repo: None,
        branch: None,
        cwd: "/work".to_owned(),
        state: match section {
            SectionName::Finished | SectionName::Done => State::Finished,
            _ => State::Idle,
        },
        running: false,
        last_activity_at: EARLIER.to_owned(),
        task_description: Some(format!("task of {id}")),
        left_off: None,
        project: None,
        flags: Vec::new(),
        section,
        error: None,
    }
}

/// One session in the named section; the section decides its state.
fn single(id: &str, section: SectionName) -> App {
    let snapshot = Snapshot {
        overview: Overview {
            sections: vec![Section {
                name: section,
                sessions: vec![entry(id, section)],
            }],
        },
        panes: HashMap::new(),
        figures: HashMap::new(),
        now: NOW.to_owned(),
    };
    update(App::new(), Msg::Refreshed(snapshot)).0
}

fn organise(action: Organise) -> Vec<Effect> {
    vec![Effect::Organise(action)]
}

mod keys_on_a_row {
    use super::*;

    #[test]
    fn d_upper_marks_a_finished_session_done() {
        let (_, effects) = press(single("s1", SectionName::Finished), KeyCode::Char('D'));

        assert_eq!(
            effects,
            organise(Organise::Done {
                id: "s1".to_owned()
            })
        );
    }

    #[test]
    fn d_upper_on_anything_but_a_finished_session_is_refused_in_the_footer() {
        for section in [
            SectionName::Active,
            SectionName::Interrupted,
            SectionName::Done,
            SectionName::Unseen,
        ] {
            let listed = keys(single("s1", section), "u");
            let (app, effects) = press(listed, KeyCode::Char('D'));

            assert!(effects.is_empty(), "{section:?}: {effects:?}");
            assert_eq!(
                app.status(),
                Some("only a finished session can be marked done"),
                "{section:?}"
            );
        }
    }

    #[test]
    fn u_upper_returns_a_done_session_to_finished() {
        let (_, effects) = press(single("s1", SectionName::Done), KeyCode::Char('U'));

        assert_eq!(
            effects,
            organise(Organise::Undone {
                id: "s1".to_owned()
            })
        );
    }

    #[test]
    fn u_upper_on_a_session_that_is_not_done_is_refused_in_the_footer() {
        let (app, effects) = press(single("s1", SectionName::Finished), KeyCode::Char('U'));

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.status(), Some("that session is not done"));
    }

    #[test]
    fn x_on_a_listed_dismissed_session_undismisses_without_asking() {
        let shown = keys(single("s1", SectionName::Dismissed), "S");
        let (app, effects) = press(shown, KeyCode::Char('x'));
        assert_eq!(
            effects,
            organise(Organise::Undismiss {
                id: "s1".to_owned()
            })
        );
        assert_eq!(app.prompt(), None);
    }

    #[test]
    fn the_keys_do_nothing_with_no_session_selected() {
        let app = update(
            App::new(),
            Msg::Refreshed(Snapshot {
                overview: Overview {
                    sections: Vec::new(),
                },
                panes: HashMap::new(),
                figures: HashMap::new(),
                now: NOW.to_owned(),
            }),
        )
        .0;

        for key in ['D', 'U', 'x', 'n'] {
            let (next, effects) = press(app.clone(), KeyCode::Char(key));
            assert!(effects.is_empty(), "{key}");
            assert_eq!(next.prompt(), None, "{key}");
        }
    }

    #[test]
    fn the_keys_act_on_the_selected_row_in_the_grouped_view_too() {
        let app = keys(single("s1", SectionName::Finished), "p");

        let (_, effects) = press(app, KeyCode::Char('D'));

        assert_eq!(
            effects,
            organise(Organise::Done {
                id: "s1".to_owned()
            })
        );
    }

    #[test]
    fn the_help_line_lists_the_organise_keys() {
        let text = render(&single("s1", SectionName::Finished));

        for needle in ["D done", "U undone", "x dismiss", "n note"] {
            assert!(text.contains(needle), "missing {needle:?}: {text}");
        }
    }
}

mod note_prompt {
    use super::*;

    fn typing(text: &str) -> App {
        keys(
            press(single("s1", SectionName::Active), KeyCode::Char('n')).0,
            text,
        )
    }

    #[test]
    fn n_opens_a_note_prompt_for_the_selected_session() {
        let (app, effects) = press(single("s1", SectionName::Active), KeyCode::Char('n'));

        assert!(effects.is_empty());
        assert_eq!(
            app.prompt(),
            Some(&Prompt::Note {
                id: "s1".to_owned(),
                text: String::new()
            })
        );
        assert!(render(&app).contains("note: "), "{}", render(&app));
    }

    #[test]
    fn typed_text_shows_in_the_prompt_and_command_keys_are_plain_letters() {
        let app = typing("quit D x");

        assert_eq!(
            app.prompt(),
            Some(&Prompt::Note {
                id: "s1".to_owned(),
                text: "quit D x".to_owned()
            })
        );
        assert!(render(&app).contains("note: quit D x"));
    }

    #[test]
    fn backspace_deletes_and_enter_saves_the_trimmed_note() {
        let app = typing("  call back  now!");
        let (app, _) = press(app, KeyCode::Backspace);

        let (app, effects) = press(app, KeyCode::Enter);

        assert_eq!(
            effects,
            organise(Organise::Note {
                id: "s1".to_owned(),
                text: "call back  now".to_owned()
            })
        );
        assert_eq!(app.prompt(), None);
    }

    #[test]
    fn an_empty_note_is_not_saved() {
        let (app, effects) = press(typing("   "), KeyCode::Enter);

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.prompt(), None);
    }

    #[test]
    fn esc_cancels_without_saving_or_quitting() {
        let (app, effects) = press(typing("draft"), KeyCode::Esc);

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.prompt(), None);
    }
}

fn user() -> Author {
    Author::User {
        name: "ali".to_owned(),
    }
}

struct Rig {
    state: TempDir,
    fake: FakeEnv,
    store: Store,
}

fn rig() -> Rig {
    let state = TempDir::new().unwrap();
    let fake = FakeEnv::new();
    fake.vars.set("USER", "ali");
    let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
    let empty = TempDir::new().unwrap();
    legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
    for (id, state) in [("fin", State::Finished), ("act", State::Idle)] {
        let mut session = Session::new(id, "claude", &format!("agent-{id}"), "/work", EARLIER);
        session.state = state;
        store.upsert_session(&session).unwrap();
    }
    let rig = Rig { state, fake, store };
    rig.command(Command::Project {
        action: ProjectAction::Add,
        name: "P".to_owned(),
        value: None,
    });
    rig.command(Command::Group {
        project: "P".to_owned(),
        sessions: vec!["fin".to_owned()],
    });
    rig
}

impl Rig {
    fn command(&self, command: Command) {
        run_in(command, &self.fake.env, Some(self.state.path())).unwrap();
    }

    fn app(&self) -> App {
        let snapshot = Snapshot::load(&self.store, NOW).unwrap();
        update(App::new(), Msg::Refreshed(snapshot)).0
    }

    /// What the dashboard does with an effect: run it and feed the answer back.
    fn perform(&self, app: App, effects: Vec<Effect>) -> App {
        effects.into_iter().fold(app, |app, effect| match effect {
            Effect::Organise(action) => {
                update(app, organise::run(&self.fake.env, &self.store, &action)).0
            }
            other => panic!("unexpected effect {other:?}"),
        })
    }

    fn select(&self, app: App, id: &str) -> App {
        let mut app = app;
        for _ in 0..10 {
            if app.selected_id() == Some(id) {
                return app;
            }
            app = keys(app, "j");
        }
        panic!("cannot select {id}");
    }

    fn act(&self, app: App, id: &str, key: char) -> App {
        let app = self.select(app, id);
        let (app, effects) = press(app, KeyCode::Char(key));
        self.perform(app, effects)
    }

    /// `x` on a live row, then `y` at the footer question.
    fn dismiss(&self, app: App, id: &str) -> App {
        let app = self.act(app, id, 'x');
        let (app, effects) = press(app, KeyCode::Char('y'));
        self.perform(app, effects)
    }
}

mod confirming_a_dismissal {
    use super::*;

    fn asked() -> App {
        press(single("s1", SectionName::Active), KeyCode::Char('x')).0
    }

    #[test]
    #[ignore = "the dismiss question is not wired up yet"]
    fn x_asks_in_the_footer_and_changes_nothing_yet() {
        let (app, effects) = press(single("s1", SectionName::Active), KeyCode::Char('x'));

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(
            app.prompt(),
            Some(&Prompt::ConfirmDismiss {
                id: "s1".to_owned(),
                name: "task of s1".to_owned()
            })
        );
        let text = render(&app);
        assert!(text.contains("Dismiss task of s1? y/N"), "{text}");
    }

    #[test]
    #[ignore = "the dismiss question is not wired up yet"]
    fn y_dismisses_and_closes_the_question() {
        let (app, effects) = press(asked(), KeyCode::Char('y'));

        assert_eq!(
            effects,
            organise(Organise::Dismiss {
                id: "s1".to_owned()
            })
        );
        assert_eq!(app.prompt(), None);
    }

    #[test]
    #[ignore = "the dismiss question is not wired up yet"]
    fn every_other_key_cancels_with_no_change_and_is_not_acted_on() {
        let others = [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Char('n'),
            KeyCode::Char('Y'),
            KeyCode::Char(' '),
            KeyCode::Char('x'),
            KeyCode::Char('q'),
            KeyCode::Char('D'),
            KeyCode::Char('j'),
            KeyCode::Backspace,
            KeyCode::Tab,
        ];
        for code in others {
            let (app, effects) = press(asked(), code);

            assert!(effects.is_empty(), "{code:?}: {effects:?}");
            assert_eq!(app.prompt(), None, "{code:?}");
            assert_eq!(app.status(), Some("dismiss cancelled"), "{code:?}");
        }
    }

    #[test]
    #[ignore = "the dismiss question is not wired up yet"]
    fn the_row_falls_back_to_its_id_when_it_has_no_task_description() {
        let mut row = entry("s1", SectionName::Active);
        row.task_description = None;
        let snapshot = Snapshot {
            overview: Overview {
                sections: vec![Section {
                    name: SectionName::Active,
                    sessions: vec![row],
                }],
            },
            panes: HashMap::new(),
            figures: HashMap::new(),
            now: NOW.to_owned(),
        };
        let app = update(App::new(), Msg::Refreshed(snapshot)).0;

        let app = press(app, KeyCode::Char('x')).0;

        assert!(render(&app).contains("Dismiss s1? y/N"), "{}", render(&app));
    }

    #[test]
    #[ignore = "the dismiss question is not wired up yet"]
    fn declining_in_a_real_store_leaves_the_session_listed() {
        let rig = rig();

        let app = rig.act(rig.app(), "act", 'x');
        let (app, effects) = press(app, KeyCode::Char('n'));
        let app = rig.perform(app, effects);

        assert!(!rig.store.get_session("act").unwrap().unwrap().dismissed);
        assert!(render(&app).contains("Active (1)"), "{}", render(&app));
    }

    #[test]
    #[ignore = "the dismiss question is not wired up yet"]
    fn the_selection_does_not_move_while_the_question_is_open() {
        let rig = rig();
        let app = rig.select(rig.app(), "act");
        let app = press(app, KeyCode::Char('x')).0;

        let (app, _) = press(app, KeyCode::Char('j'));

        assert_eq!(app.selected_id(), Some("act"));
    }
}

mod with_a_real_store {
    use super::*;

    #[test]
    fn d_upper_moves_the_session_into_its_projects_done_section_and_records_the_user() {
        let rig = rig();

        let app = rig.act(rig.app(), "fin", 'D');
        let text = render(&keys(app, "p"));

        assert!(text.contains("P · Done (1)"), "{text}");
        assert!(rig
            .store
            .get_session("fin")
            .unwrap()
            .unwrap()
            .done_at
            .is_some());
        let events: Vec<_> = rig
            .store
            .events_since(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "session.done")
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].author, user());
    }

    #[test]
    fn u_upper_returns_it_to_finished() {
        let rig = rig();
        let app = rig.act(rig.app(), "fin", 'D');
        assert!(rig
            .store
            .get_session("fin")
            .unwrap()
            .unwrap()
            .done_at
            .is_some());

        let app = rig.act(app, "fin", 'U');

        assert!(rig
            .store
            .get_session("fin")
            .unwrap()
            .unwrap()
            .done_at
            .is_none());
        assert!(render(&app).contains("Finished (1)"), "{}", render(&app));
    }

    #[test]
    fn the_store_refuses_done_on_a_session_that_is_not_finished() {
        let rig = rig();

        let msg = organise::run(
            &rig.fake.env,
            &rig.store,
            &Organise::Done {
                id: "act".to_owned(),
            },
        );

        let Msg::Failed(message) = msg else {
            panic!("expected a refusal");
        };
        assert!(message.contains("finished"), "{message}");
        assert!(rig
            .store
            .get_session("act")
            .unwrap()
            .unwrap()
            .done_at
            .is_none());
    }

    #[test]
    fn x_dismisses_hides_and_s_then_x_undismisses() {
        let rig = rig();

        let app = rig.dismiss(rig.app(), "act");
        assert!(!render(&app).contains("task of act"), "{}", render(&app));
        let app = keys(app, "S");
        assert!(render(&app).contains("Dismissed (1)"), "{}", render(&app));

        let app = rig.act(app, "act", 'x');

        let text = render(&app);
        assert!(text.contains("Active (1)"), "{text}");
        assert!(!text.contains("Dismissed"), "{text}");
    }

    #[test]
    fn a_dismissed_session_keeps_its_history_and_is_in_the_snapshot() {
        let rig = rig();
        rig.command(Command::Dismiss {
            target: "act".to_owned(),
            yes: true,
        });

        let snapshot = Snapshot::load(&rig.store, NOW).unwrap();

        let dismissed = snapshot
            .overview
            .sections
            .iter()
            .find(|s| s.name == SectionName::Dismissed)
            .unwrap();
        assert_eq!(dismissed.sessions.len(), 1);
        assert!(rig.store.get_session("act").unwrap().is_some());
    }

    #[test]
    fn n_saves_a_user_authored_note_on_the_session() {
        let rig = rig();
        let app = rig.select(rig.app(), "act");
        let app = keys(app, "n");
        let app = keys(app, " call back ");
        let (app, effects) = press(app, KeyCode::Enter);

        rig.perform(app, effects);

        let notes = rig.store.notes_for(OwnerKind::Session, "act").unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].body, "call back");
        assert_eq!(notes[0].author, user());
    }
}
