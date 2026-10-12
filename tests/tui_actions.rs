#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Dashboard keys that carry the rest of the CLI actions (FR-030): P park,
//! g group, b progress source, s summarize, l link. Each one runs the command
//! of the same name.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use claude_monitor::env::Env;
use claude_monitor::model::{OwnerKind, Session, State};
use claude_monitor::progress::binding::{view, ProgressView};
use claude_monitor::store::queries::{Flag, Overview, OverviewEntry, Section, SectionName};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::summary::{MODEL_VAR, URL_VAR};
use claude_monitor::testing::FakeEnv;
use claude_monitor::tui::app::{update, App, Effect, Msg, Prompt, Snapshot};
use claude_monitor::tui::bind::{self, BindOption};
use claude_monitor::tui::organise::{self, Organise};
use claude_monitor::tui::summarize;
use claude_monitor::tui::view::view as draw;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;
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
    terminal.draw(|frame| draw(app, frame)).unwrap();
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

fn entry(id: &str, flags: Vec<Flag>) -> OverviewEntry {
    OverviewEntry {
        id: id.to_owned(),
        agent_kind: "claude".to_owned(),
        agent_session_id: format!("agent-{id}"),
        repo: None,
        branch: None,
        cwd: "/work".to_owned(),
        state: State::Idle,
        running: false,
        last_activity_at: EARLIER.to_owned(),
        task_description: Some(format!("task of {id}")),
        left_off: None,
        project: None,
        flags,
        section: SectionName::Active,
        error: None,
    }
}

fn single(id: &str, flags: Vec<Flag>) -> App {
    let snapshot = Snapshot {
        overview: Overview {
            sections: vec![Section {
                name: SectionName::Active,
                sessions: vec![entry(id, flags)],
            }],
        },
        panes: HashMap::new(),
        figures: HashMap::new(),
        now: NOW.to_owned(),
    };
    update(App::new(), Msg::Refreshed(snapshot)).0
}

fn organise_effect(action: Organise) -> Vec<Effect> {
    vec![Effect::Organise(action)]
}

fn id(text: &str) -> String {
    text.to_owned()
}

mod park {
    use super::*;

    #[test]
    fn p_upper_parks_a_row_that_is_not_parked() {
        let (_, effects) = press(single("s1", vec![]), KeyCode::Char('P'));

        assert_eq!(effects, organise_effect(Organise::Park { id: id("s1") }));
    }

    #[test]
    fn p_upper_unparks_a_parked_row() {
        let (_, effects) = press(single("s1", vec![Flag::Parked]), KeyCode::Char('P'));

        assert_eq!(effects, organise_effect(Organise::Unpark { id: id("s1") }));
    }
}

mod group {
    use super::*;

    fn typing(text: &str) -> App {
        keys(keys(single("s1", vec![]), "g"), text)
    }

    #[test]
    fn g_opens_a_project_prompt_on_the_selected_row() {
        let app = keys(single("s1", vec![]), "g");

        assert_eq!(
            app.prompt(),
            Some(&Prompt::Group {
                id: id("s1"),
                text: String::new()
            })
        );
    }

    #[test]
    fn enter_groups_the_session_into_the_typed_project() {
        let (app, effects) = press(typing(" alpha "), KeyCode::Enter);

        assert_eq!(
            effects,
            organise_effect(Organise::Group {
                id: id("s1"),
                project: id("alpha")
            })
        );
        assert_eq!(app.prompt(), None);
    }

    #[test]
    fn a_dash_takes_the_session_out_of_its_project() {
        let (_, effects) = press(typing("-"), KeyCode::Enter);

        assert_eq!(
            effects,
            organise_effect(Organise::Group {
                id: id("s1"),
                project: id("-")
            })
        );
    }

    #[test]
    fn an_empty_name_or_esc_changes_nothing() {
        let (app, effects) = press(typing("  "), KeyCode::Enter);
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.prompt(), None);

        let (app, effects) = press(typing("draft"), KeyCode::Esc);
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.prompt(), None);
    }
}

mod link {
    use super::*;

    fn typing(text: &str) -> App {
        keys(keys(single("s1", vec![]), "l"), text)
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn l_opens_a_link_prompt() {
        assert_eq!(
            keys(single("s1", vec![]), "l").prompt(),
            Some(&Prompt::Link {
                id: id("s1"),
                text: String::new()
            })
        );
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn a_node_and_a_doc_are_linked() {
        let (app, effects) = press(typing("123 45"), KeyCode::Enter);

        assert_eq!(
            effects,
            organise_effect(Organise::Link {
                id: id("s1"),
                node_id: id("123"),
                doc: Some(id("45"))
            })
        );
        assert_eq!(app.prompt(), None);
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn the_doc_is_optional() {
        let (_, effects) = press(typing("123"), KeyCode::Enter);

        assert_eq!(
            effects,
            organise_effect(Organise::Link {
                id: id("s1"),
                node_id: id("123"),
                doc: None
            })
        );
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn empty_input_or_esc_links_nothing() {
        let (_, effects) = press(typing(""), KeyCode::Enter);
        assert!(effects.is_empty(), "{effects:?}");

        let (app, effects) = press(typing("12"), KeyCode::Esc);
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.prompt(), None);
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn more_than_a_node_and_a_doc_is_refused_in_the_footer() {
        let (app, effects) = press(typing("1 2 3"), KeyCode::Enter);

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.status(), Some("link takes <node-id> [doc]"));
    }
}

mod bind_chooser {
    use super::*;

    fn options() -> Vec<BindOption> {
        vec![
            BindOption {
                label: id("tasks.md (1 of 3)"),
                path: Some(PathBuf::from("/work/tasks.md")),
            },
            BindOption {
                label: id("none"),
                path: None,
            },
        ]
    }

    fn open() -> App {
        let (app, effects) = press(single("s1", vec![]), KeyCode::Char('b'));
        assert_eq!(effects, vec![Effect::LoadCandidates { id: id("s1") }]);
        update(
            app,
            Msg::Candidates {
                id: id("s1"),
                options: options(),
            },
        )
        .0
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn the_candidates_open_a_chooser_listing_each_plan_and_none() {
        let app = open();

        let chooser = app.chooser().unwrap();
        assert_eq!(chooser.id(), "s1");
        assert_eq!(chooser.options(), options().as_slice());
        assert_eq!(chooser.cursor(), 0);
        let text = render(&app);
        assert!(text.contains("tasks.md (1 of 3)"), "{text}");
        assert!(text.contains("none"), "{text}");
    }

    #[test]
    fn candidates_for_another_session_are_ignored() {
        let (app, _) = press(single("s1", vec![]), KeyCode::Char('b'));

        let (app, _) = update(
            app,
            Msg::Candidates {
                id: id("other"),
                options: options(),
            },
        );

        assert!(app.chooser().is_none());
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn j_and_k_move_the_cursor_within_the_rows() {
        let app = keys(open(), "j");
        assert_eq!(app.chooser().unwrap().cursor(), 1);
        let app = keys(app, "j");
        assert_eq!(app.chooser().unwrap().cursor(), 1, "stops at the last row");
        let app = keys(app, "kk");
        assert_eq!(app.chooser().unwrap().cursor(), 0, "stops at the first row");
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn enter_binds_the_highlighted_plan_and_closes() {
        let (app, effects) = press(open(), KeyCode::Enter);

        assert_eq!(
            effects,
            organise_effect(Organise::ProgressBind {
                id: id("s1"),
                path: Some(PathBuf::from("/work/tasks.md"))
            })
        );
        assert!(app.chooser().is_none());
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn enter_on_none_binds_no_plan() {
        let (_, effects) = press(keys(open(), "j"), KeyCode::Enter);

        assert_eq!(
            effects,
            organise_effect(Organise::ProgressBind {
                id: id("s1"),
                path: None
            })
        );
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn esc_closes_without_binding_or_quitting() {
        let (app, effects) = press(open(), KeyCode::Esc);

        assert!(effects.is_empty(), "{effects:?}");
        assert!(app.chooser().is_none());
    }
}

mod summarize_key {
    use super::*;

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn s_opens_the_detail_page_and_asks_for_a_summary() {
        let (app, effects) = press(single("s1", vec![]), KeyCode::Char('s'));

        assert_eq!(
            effects,
            vec![
                Effect::LoadDetail { id: id("s1") },
                Effect::Summarize { id: id("s1") }
            ]
        );
        assert!(app.detail().is_some());
    }

    #[test]
    fn nothing_but_s_ever_asks_for_a_summary() {
        for key in [
            KeyCode::Char('i'),
            KeyCode::Char('L'),
            KeyCode::Enter,
            KeyCode::Char('j'),
        ] {
            let (_, effects) = press(single("s1", vec![]), key);
            assert!(
                !effects
                    .iter()
                    .any(|e| matches!(e, Effect::Summarize { .. })),
                "{key:?}: {effects:?}"
            );
        }
        let (_, effects) = update(
            single("s1", vec![]),
            Msg::Refreshed(Snapshot {
                overview: Overview { sections: vec![] },
                panes: HashMap::new(),
                figures: HashMap::new(),
                now: NOW.to_owned(),
            }),
        );
        assert!(effects.is_empty(), "{effects:?}");
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn a_stored_summary_reloads_the_open_detail() {
        let (app, _) = press(single("s1", vec![]), KeyCode::Char('s'));

        let (_, effects) = update(
            app,
            Msg::Summarized {
                id: id("s1"),
                result: Ok(()),
            },
        );

        assert_eq!(effects, vec![Effect::LoadDetail { id: id("s1") }]);
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn a_refusal_is_shown_in_the_footer() {
        let (app, _) = press(single("s1", vec![]), KeyCode::Char('s'));

        let (app, effects) = update(
            app,
            Msg::Summarized {
                id: id("s1"),
                result: Err(id("no summary model is configured")),
            },
        );

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.status(), Some("no summary model is configured"));
    }
}

struct Rig {
    _state: TempDir,
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
    Rig {
        _state: state,
        fake,
        store,
    }
}

impl Rig {
    fn session(&self, id: &str, cwd: &str) -> Session {
        let mut session = Session::new(id, "claude", &format!("agent-{id}"), cwd, EARLIER);
        session.state = State::Idle;
        session.task_description = Some("Fix the flaky test".to_owned());
        session.left_off = Some("Test now passes".to_owned());
        self.store.upsert_session(&session).unwrap();
        session
    }

    fn app(&self) -> App {
        let snapshot = Snapshot::load(&self.store, NOW).unwrap();
        update(App::new(), Msg::Refreshed(snapshot)).0
    }

    fn perform(&self, app: App, effects: Vec<Effect>) -> App {
        effects.into_iter().fold(app, |app, effect| match effect {
            Effect::Organise(action) => {
                update(app, organise::run(&self.fake.env, &self.store, &action)).0
            }
            other => panic!("unexpected effect {other:?}"),
        })
    }

    fn act(&self, app: App, key: char) -> App {
        let (app, effects) = press(app, KeyCode::Char(key));
        self.perform(app, effects)
    }
}

mod with_a_real_store {
    use super::*;

    #[test]
    fn p_upper_parks_then_unparks() {
        let rig = rig();
        rig.session("act", "/work");

        let app = rig.act(rig.app(), 'P');
        assert!(rig.store.get_session("act").unwrap().unwrap().parked);

        rig.act(app, 'P');
        assert!(!rig.store.get_session("act").unwrap().unwrap().parked);
    }

    #[test]
    fn g_groups_into_a_new_project_and_a_dash_ungroups() {
        let rig = rig();
        rig.session("act", "/work");

        let app = keys(rig.app(), "gQ");
        let (app, effects) = press(app, KeyCode::Enter);
        rig.perform(app, effects);
        let grouped = rig.store.get_session("act").unwrap().unwrap();
        let project = rig.store.project_by_name("Q").unwrap().unwrap();
        assert_eq!(grouped.project_id.as_deref(), Some(project.id.as_str()));

        let app = keys(rig.app(), "g-");
        let (app, effects) = press(app, KeyCode::Enter);
        rig.perform(app, effects);
        assert_eq!(
            rig.store.get_session("act").unwrap().unwrap().project_id,
            None
        );
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn l_links_the_session_and_a_bad_node_id_is_refused() {
        let rig = rig();
        rig.session("act", "/work");

        let app = keys(rig.app(), "l123 45");
        let (app, effects) = press(app, KeyCode::Enter);
        rig.perform(app, effects);
        let links = rig.store.links_for(OwnerKind::Session, "act").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].cortex_node_id, "123");
        assert_eq!(links[0].cortex_doc_id.as_deref(), Some("45"));

        let app = keys(rig.app(), "labc");
        let (app, effects) = press(app, KeyCode::Enter);
        let app = rig.perform(app, effects);
        assert!(
            app.status().is_some_and(|s| s.contains("decimal number")),
            "{:?}",
            app.status()
        );
        assert_eq!(
            rig.store
                .links_for(OwnerKind::Session, "act")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn b_lists_the_plans_in_the_session_directory_and_binds_the_pick() {
        let tree = TempDir::new().unwrap();
        fs::write(tree.path().join("a.md"), "- [x] one\n- [ ] two\n").unwrap();
        fs::write(tree.path().join("b.md"), "- [ ] three\n").unwrap();
        let rig = rig();
        let session = rig.session("act", tree.path().to_str().unwrap());
        let env = Env::real();

        let Msg::Candidates {
            id: for_id,
            options,
        } = bind::load(&env, &rig.store, "act")
        else {
            panic!("expected candidates");
        };

        assert_eq!(for_id, "act");
        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        assert!(
            labels[0].contains("a.md") && labels[0].contains("1 of 2"),
            "{labels:?}"
        );
        assert!(labels[1].contains("b.md"), "{labels:?}");
        assert_eq!(options.last().unwrap().path, None, "none comes last");

        let chosen = options[0].path.clone();
        let msg = organise::run(
            &env,
            &rig.store,
            &Organise::ProgressBind {
                id: id("act"),
                path: chosen,
            },
        );
        assert!(matches!(msg, Msg::Refreshed(_)), "{msg:?}");
        let ProgressView::Bound(bound) = view(&rig.store, &env, &session).unwrap() else {
            panic!("expected a bound plan");
        };
        assert_eq!(bound.progress.unwrap().done, 1);
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn binding_none_opts_the_session_out_of_progress() {
        let tree = TempDir::new().unwrap();
        fs::write(tree.path().join("a.md"), "- [x] one\n").unwrap();
        let rig = rig();
        let session = rig.session("act", tree.path().to_str().unwrap());
        let env = Env::real();

        let msg = organise::run(
            &env,
            &rig.store,
            &Organise::ProgressBind {
                id: id("act"),
                path: None,
            },
        );

        assert!(matches!(msg, Msg::Refreshed(_)), "{msg:?}");
        let ProgressView::Bound(bound) = view(&rig.store, &env, &session).unwrap() else {
            panic!("expected an opted-out binding");
        };
        assert!(bound.progress.is_none());
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn summarize_without_an_endpoint_says_it_is_not_configured() {
        let rig = rig();
        rig.session("act", "/work");

        let msg = summarize::run(&rig.fake.env, rig.store.path(), "act");

        let Msg::Summarized { id: for_id, result } = msg else {
            panic!("expected a summarize answer");
        };
        assert_eq!(for_id, "act");
        let message = result.unwrap_err();
        assert!(
            message.contains("no summary model is configured"),
            "{message}"
        );
        assert!(rig
            .store
            .list_summaries("session", "act")
            .unwrap()
            .is_empty());
    }

    #[test]
    #[ignore = "I1: dashboard actions"]
    fn a_summary_shows_labelled_generated_on_the_detail_page() {
        let rig = rig();
        let session = rig.session("act", "/work");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        rig.fake.vars.set(
            URL_VAR,
            &format!("http://{}", listener.local_addr().unwrap()),
        );
        rig.fake.vars.set(MODEL_VAR, "tiny-model");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                raw.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if raw.len() >= end + 4 + length {
                        break;
                    }
                }
                assert!(n > 0, "client closed early");
            }
            let body =
                json!({"choices": [{"message": {"content": "Fixed the flaky test."}}]}).to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
        });

        let Msg::Summarized { result, .. } = summarize::run(&rig.fake.env, rig.store.path(), "act")
        else {
            panic!("expected a summarize answer");
        };
        server.join().unwrap();
        assert_eq!(result, Ok(()));

        let detail =
            claude_monitor::cli::detail::SessionDetail::load(&rig.store, &rig.fake.env, &session)
                .unwrap();
        let (app, _) = press(rig.app(), KeyCode::Char('i'));
        let app = update(app, Msg::Detail(detail)).0;
        let text = render(&app);
        assert!(text.contains("generated"), "{text}");
        assert!(text.contains("Fixed the flaky test."), "{text}");
    }
}
