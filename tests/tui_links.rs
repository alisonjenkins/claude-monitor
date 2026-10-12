#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Link status in the TUI detail page (T103, FR-026, FR-028): the session's
//! links, and `L` running the check off the UI thread.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use claude_monitor::cli::detail::SessionDetail;
use claude_monitor::export::links::{Check, LinkCheck, Outcome};
use claude_monitor::model::{Author, OwnerKind, Session, State};
use claude_monitor::progress::binding::ProgressView;
use claude_monitor::store::links::{Link, NewLink};
use claude_monitor::store::queries::{Overview, OverviewEntry, Section, SectionName};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::tui::app::{update, App, Effect, Msg, Snapshot};
use claude_monitor::tui::links::{run_check, spawn_check, LinksState};
use claude_monitor::tui::view::view;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const EARLIER: &str = "2026-10-12T11:58:00Z";
const TIMEOUT: Duration = Duration::from_secs(5);

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

fn loaded(ids: &[&str]) -> App {
    let snapshot = Snapshot {
        overview: Overview {
            sections: vec![Section {
                name: SectionName::Active,
                sessions: ids.iter().map(|id| entry(id)).collect(),
            }],
        },
        panes: HashMap::new(),
        figures: HashMap::new(),
        now: NOW.to_owned(),
    };
    update(App::new(), Msg::Refreshed(snapshot)).0
}

fn press(app: App, code: KeyCode) -> (App, Vec<Effect>) {
    update(app, Msg::Key(code, KeyModifiers::NONE))
}

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
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

fn link(node: &str, doc: Option<&str>, kind: &str) -> Link {
    Link {
        id: format!("link-{node}"),
        owner_kind: OwnerKind::Session,
        owner_id: "s1".to_owned(),
        cortex_node_id: node.to_owned(),
        cortex_doc_id: doc.map(str::to_owned),
        cortex_kind: kind.to_owned(),
        last_agreed_at: None,
        last_remote_status: None,
    }
}

fn detail(links: Vec<Link>) -> SessionDetail {
    SessionDetail {
        id: "s1".to_owned(),
        progress: ProgressView::None,
        task_description: Some("fix the build".to_owned()),
        left_off: None,
        generated: Vec::new(),
        links,
        root: PathBuf::from("/work/nix-config"),
    }
}

fn opened(links: Vec<Link>) -> App {
    let (app, _) = press(loaded(&["s1"]), KeyCode::Char('i'));
    update(app, Msg::Detail(detail(links))).0
}

fn checked(link: Link, local: &str, outcome: Outcome) -> LinkCheck {
    LinkCheck {
        link,
        local_status: Some(local.to_owned()),
        outcome,
    }
}

fn answer(app: App, result: Result<Check, String>) -> App {
    update(
        app,
        Msg::LinksChecked {
            id: "s1".to_owned(),
            result,
        },
    )
    .0
}

mod page {
    use super::*;

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn the_page_lists_the_sessions_links_and_offers_the_check() {
        let app = opened(vec![
            link("42", Some("7"), "task"),
            link("43", None, "memory"),
        ]);

        let text = render(&app);

        assert!(text.contains("Links"), "{text}");
        assert!(text.contains("task 42 (doc 7)"), "{text}");
        assert!(text.contains("memory 43"), "{text}");
        assert!(text.contains("L to check"), "{text}");
        assert_eq!(app.detail().unwrap().links(), &LinksState::Idle);
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn a_session_without_links_has_no_links_block() {
        let text = render(&opened(Vec::new()));

        assert!(!text.contains("Links"), "{text}");
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn l_on_the_page_asks_for_the_check_and_shows_checking_at_once() {
        let (app, effects) = press(
            opened(vec![link("42", Some("7"), "task")]),
            KeyCode::Char('L'),
        );

        assert_eq!(
            effects,
            vec![Effect::CheckLinks {
                id: "s1".to_owned()
            }]
        );
        assert_eq!(app.detail().unwrap().links(), &LinksState::Checking);
        assert!(render(&app).contains("checking"), "{}", render(&app));
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn l_on_a_row_opens_the_page_loads_it_and_starts_the_check() {
        let (app, effects) = press(loaded(&["s1", "s2"]), KeyCode::Char('L'));

        assert_eq!(
            effects,
            vec![
                Effect::LoadDetail {
                    id: "s1".to_owned()
                },
                Effect::CheckLinks {
                    id: "s1".to_owned()
                }
            ]
        );
        assert_eq!(app.detail().unwrap().id(), "s1");
        assert_eq!(app.detail().unwrap().links(), &LinksState::Checking);
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn l_while_a_check_runs_does_not_start_another() {
        let (app, _) = press(opened(vec![link("42", None, "task")]), KeyCode::Char('L'));

        let (_, effects) = press(app, KeyCode::Char('L'));

        assert!(effects.is_empty(), "{effects:?}");
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn l_with_no_links_runs_nothing_and_says_so() {
        let (app, effects) = press(opened(Vec::new()), KeyCode::Char('L'));

        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(app.detail().unwrap().links(), &LinksState::Idle);
        assert_eq!(app.status(), Some("this session has no links to check"));
    }
}

mod results {
    use super::*;

    fn after_check(checks: Vec<LinkCheck>) -> App {
        let (app, _) = press(
            opened(checks.iter().map(|c| c.link.clone()).collect()),
            KeyCode::Char('L'),
        );
        answer(app, Ok(Check::Checked(checks)))
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn each_link_shows_agree_disagree_with_both_statuses_or_unreachable() {
        let app = after_check(vec![
            checked(
                link("42", Some("7"), "task"),
                "idle",
                Outcome::Agree {
                    status: "idle".to_owned(),
                },
            ),
            checked(
                link("43", Some("7"), "task"),
                "finished",
                Outcome::Disagree {
                    local: "finished".to_owned(),
                    remote: "doing".to_owned(),
                },
            ),
            checked(
                link("44", Some("7"), "task"),
                "idle",
                Outcome::Unreachable {
                    reason: "timed out after 10s".to_owned(),
                },
            ),
        ]);

        let text = render(&app);
        let line = |node: &str| {
            text.lines()
                .find(|l| l.contains(&format!("task {node} ")))
                .unwrap_or_else(|| panic!("no line for {node}: {text}"))
                .to_owned()
        };

        assert!(line("42").contains("agree"), "{text}");
        assert!(!line("42").contains("disagree"), "{text}");
        let disagree = line("43");
        assert!(disagree.contains("disagree"), "{text}");
        assert!(disagree.contains("local finished"), "{text}");
        assert!(disagree.contains("remote doing"), "{text}");
        assert!(line("44").contains("unreachable"), "{text}");
        assert!(line("44").contains("timed out after 10s"), "{text}");
        assert!(matches!(
            app.detail().unwrap().links(),
            LinksState::Checked(_)
        ));
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn a_skipped_check_says_why_beside_every_link() {
        let (app, _) = press(
            opened(vec![link("42", Some("7"), "task")]),
            KeyCode::Char('L'),
        );
        let app = answer(
            app,
            Ok(Check::Skipped {
                reason: "CORTEXIPHAN_SERVER is not set".to_owned(),
            }),
        );

        let text = render(&app);

        let line = text.lines().find(|l| l.contains("task 42 ")).unwrap();
        assert!(line.contains("skipped"), "{text}");
        assert!(line.contains("CORTEXIPHAN_SERVER is not set"), "{text}");
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn a_failed_check_is_shown_and_a_second_l_runs_it_again() {
        let (app, _) = press(opened(vec![link("42", None, "task")]), KeyCode::Char('L'));
        let app = answer(app, Err("cannot read the links to check: boom".to_owned()));

        assert!(render(&app).contains("boom"), "{}", render(&app));
        let (_, effects) = press(app, KeyCode::Char('L'));

        assert_eq!(
            effects,
            vec![Effect::CheckLinks {
                id: "s1".to_owned()
            }]
        );
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn an_answer_for_another_session_is_ignored() {
        let (app, _) = press(opened(vec![link("42", None, "task")]), KeyCode::Char('L'));

        let app = update(
            app,
            Msg::LinksChecked {
                id: "s2".to_owned(),
                result: Ok(Check::Checked(Vec::new())),
            },
        )
        .0;

        assert_eq!(app.detail().unwrap().links(), &LinksState::Checking);
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn closing_the_page_and_reopening_it_forgets_the_result() {
        let app = after_check(vec![checked(
            link("42", None, "task"),
            "idle",
            Outcome::NoRemoteStatus,
        )]);

        let (app, _) = press(app, KeyCode::Esc);
        let (app, _) = press(app, KeyCode::Char('i'));

        assert_eq!(app.detail().unwrap().links(), &LinksState::Idle);
    }
}

/// A `cortexiphan` stand-in on a private PATH: logs its arguments, then
/// prints `out.N` for `--id N`, or sleeps when `slow.N` exists.
struct FakeClient {
    dir: TempDir,
}

impl FakeClient {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let script = format!(
            r#"#!/bin/sh
echo "$*" >> "{dir}/calls.log"
id=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--id" ]; then id="$arg"; fi
  prev="$arg"
done
[ -e "{dir}/slow.$id" ] && exec sleep 5
[ -e "{dir}/out.$id" ] && exec cat "{dir}/out.$id"
exit 3
"#,
            dir = dir.path().display()
        );
        let program = dir.path().join("cortexiphan");
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn reply(&self, id: &str, status: &str) {
        let body = format!(r#"{{"node":{{"id":{id},"fields":{{"status":"{status}"}}}}}}"#);
        std::fs::write(self.dir.path().join(format!("out.{id}")), body).unwrap();
    }

    fn make_slow(&self, id: &str) {
        std::fs::write(self.dir.path().join(format!("slow.{id}")), "").unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls.log"))
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

struct Rig {
    _state: TempDir,
    fake: FakeEnv,
    client: FakeClient,
    store: Store,
}

fn user() -> Author {
    Author::User {
        name: "ali".to_owned(),
    }
}

fn rig() -> Rig {
    let state = TempDir::new().unwrap();
    let fake = FakeEnv::new();
    let client = FakeClient::new();
    fake.vars.set("PATH", client.dir.path().to_str().unwrap());
    fake.vars.set("CORTEXIPHAN_SERVER", "http://127.0.0.1:1");
    fake.vars.set("CORTEXIPHAN_TOKEN", "secret-token");
    let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
    let empty = TempDir::new().unwrap();
    legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
    for id in ["s1", "s2"] {
        let mut session = Session::new(id, "claude", &format!("agent-{id}"), "/work", EARLIER);
        session.state = State::Idle;
        store.upsert_session(&session).unwrap();
    }
    Rig {
        _state: state,
        fake,
        client,
        store,
    }
}

impl Rig {
    fn link(&self, owner: &str, node: &str) {
        self.store
            .add_link(
                &NewLink {
                    owner_kind: OwnerKind::Session,
                    owner_id: owner,
                    node_id: node,
                    doc_id: Some("7"),
                    cortex_kind: "task",
                },
                EARLIER,
                &user(),
            )
            .unwrap();
    }
}

mod worker {
    use super::*;

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn the_check_covers_only_this_sessions_links_and_changes_nothing() {
        let rig = rig();
        rig.link("s1", "42");
        rig.link("s2", "99");
        rig.client.reply("42", "doing");
        rig.client.reply("99", "done");
        let links_before = rig.store.all_links().unwrap();
        let events_before = rig.store.events_since(0, 1000).unwrap().len();

        let msg = run_check(&rig.store, rig.fake.vars.as_ref(), "s1", TIMEOUT);

        let Msg::LinksChecked { id, result } = msg else {
            panic!("not a LinksChecked message");
        };
        assert_eq!(id, "s1");
        let Ok(Check::Checked(checks)) = result else {
            panic!("not checked: {result:?}");
        };
        assert_eq!(checks.len(), 1, "{checks:?}");
        assert_eq!(checks[0].link.cortex_node_id, "42");
        assert!(
            matches!(&checks[0].outcome, Outcome::Agree { .. }),
            "{:?}",
            checks[0].outcome
        );
        assert_eq!(rig.client.calls().len(), 1, "{:?}", rig.client.calls());
        assert!(rig.client.calls()[0].contains("--id 42"));
        assert_eq!(rig.store.all_links().unwrap(), links_before);
        assert_eq!(
            rig.store.events_since(0, 1000).unwrap().len(),
            events_before
        );
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn without_server_and_token_the_answer_is_skipped_and_nothing_runs() {
        let rig = rig();
        rig.link("s1", "42");
        rig.fake.vars.set("CORTEXIPHAN_SERVER", "");

        let msg = run_check(&rig.store, rig.fake.vars.as_ref(), "s1", TIMEOUT);

        let Msg::LinksChecked { result, .. } = msg else {
            panic!("not a LinksChecked message");
        };
        assert!(matches!(result, Ok(Check::Skipped { .. })), "{result:?}");
        assert!(rig.client.calls().is_empty());
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn a_slow_remote_never_blocks_the_caller_and_the_answer_arrives_later() {
        let rig = rig();
        rig.link("s1", "42");
        rig.client.make_slow("42");
        let (tx, rx) = mpsc::channel();

        let started = Instant::now();
        spawn_check(
            &rig.fake.env,
            rig.store.path(),
            "s1",
            Duration::from_millis(300),
            tx,
        )
        .unwrap();
        let spawn_took = started.elapsed();
        let msg = rx.recv_timeout(Duration::from_secs(5)).unwrap();

        assert!(spawn_took < Duration::from_millis(150), "{spawn_took:?}");
        let Msg::LinksChecked {
            id,
            result: Ok(Check::Checked(checks)),
        } = msg
        else {
            panic!("unexpected answer");
        };
        assert_eq!(id, "s1");
        assert!(
            matches!(&checks[0].outcome, Outcome::Unreachable { .. }),
            "{:?}",
            checks[0].outcome
        );
    }

    #[test]
    #[ignore = "T103: implemented in the next commit"]
    fn a_disagreement_reaches_the_page_with_both_statuses() {
        let rig = rig();
        rig.link("s1", "42");
        rig.client.reply("42", "doing");
        let mut session = rig.store.get_session("s1").unwrap().unwrap();
        session.state = State::Finished;
        rig.store.upsert_session(&session).unwrap();
        let (app, _) = press(loaded(&["s1"]), KeyCode::Char('L'));
        let links = rig.store.links_for(OwnerKind::Session, "s1").unwrap();
        let app = update(app, Msg::Detail(detail(links))).0;

        let msg = run_check(&rig.store, rig.fake.vars.as_ref(), "s1", TIMEOUT);
        let text = render(&update(app, msg).0);

        let line = text.lines().find(|l| l.contains("task 42 ")).unwrap();
        assert!(line.contains("disagree"), "{text}");
        assert!(line.contains("local finished"), "{text}");
        assert!(line.contains("remote doing"), "{text}");
    }
}
