//! Links between sessions or projects and external graph nodes (T096, T100,
//! T101, FR-026, FR-028): rows and events, the `link`/`unlink` commands, and
//! `links check` against a fake `cortexiphan` executable on a temporary PATH.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use clap::Parser;
use claude_monitor::cli::{run_in, Cli, Command, Output, ProjectAction};
use claude_monitor::error::Error;
use claude_monitor::model::{Author, OwnerKind, Session};
use claude_monitor::store::links::NewLink;
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::Value;
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T11:00:00Z";

fn user() -> Author {
    Author::User {
        name: "ali".to_owned(),
    }
}

fn open(dir: &TempDir) -> Store {
    Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap()
}

fn new_link<'a>(kind: OwnerKind, owner: &'a str, node: &'a str) -> NewLink<'a> {
    NewLink {
        owner_kind: kind,
        owner_id: owner,
        node_id: node,
        doc_id: Some("7"),
        cortex_kind: "task",
    }
}

fn link_events(store: &Store) -> Vec<(String, String, Author)> {
    store
        .events_since(0, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind.starts_with("link."))
        .map(|e| (e.kind, e.at, e.author))
        .collect()
}

mod rows {
    use super::*;

    #[test]
    fn a_link_records_owner_node_kind_and_an_event_with_author_and_time() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);

        let added = store
            .add_link(&new_link(OwnerKind::Session, "s1", "42"), T0, &user())
            .unwrap();

        assert!(added.created);
        assert_eq!(added.link.owner_kind, OwnerKind::Session);
        assert_eq!(added.link.owner_id, "s1");
        assert_eq!(added.link.cortex_node_id, "42");
        assert_eq!(added.link.cortex_doc_id.as_deref(), Some("7"));
        assert_eq!(added.link.cortex_kind, "task");
        assert_eq!(added.link.last_agreed_at, None);
        assert_eq!(added.link.last_remote_status, None);
        assert_eq!(
            store.links_for(OwnerKind::Session, "s1").unwrap(),
            vec![added.link]
        );
        assert_eq!(
            link_events(&store),
            vec![("link.added".to_owned(), T0.to_owned(), user())]
        );
    }

    #[test]
    fn linking_the_same_pair_again_changes_nothing_and_adds_no_event() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);
        let first = store
            .add_link(&new_link(OwnerKind::Project, "p1", "42"), T0, &user())
            .unwrap();

        let again = store
            .add_link(&new_link(OwnerKind::Project, "p1", "42"), T1, &user())
            .unwrap();

        assert!(!again.created);
        assert_eq!(again.link, first.link);
        assert_eq!(store.all_links().unwrap().len(), 1);
        assert_eq!(link_events(&store).len(), 1);
    }

    #[test]
    fn one_owner_can_have_several_nodes_and_one_node_several_owners() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);
        for (kind, owner, node) in [
            (OwnerKind::Session, "s1", "1"),
            (OwnerKind::Session, "s1", "2"),
            (OwnerKind::Project, "p1", "1"),
        ] {
            store
                .add_link(&new_link(kind, owner, node), T0, &user())
                .unwrap();
        }

        assert_eq!(store.links_for(OwnerKind::Session, "s1").unwrap().len(), 2);
        assert_eq!(store.links_for(OwnerKind::Project, "p1").unwrap().len(), 1);
        assert_eq!(store.all_links().unwrap().len(), 3);
    }

    #[test]
    fn unlink_removes_only_that_pair_and_records_an_event() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);
        store
            .add_link(&new_link(OwnerKind::Session, "s1", "1"), T0, &user())
            .unwrap();
        store
            .add_link(&new_link(OwnerKind::Session, "s1", "2"), T0, &user())
            .unwrap();

        assert!(store
            .remove_link(OwnerKind::Session, "s1", "1", T1, &user())
            .unwrap());

        let left = store.links_for(OwnerKind::Session, "s1").unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].cortex_node_id, "2");
        let kinds: Vec<_> = link_events(&store).into_iter().map(|e| e.0).collect();
        assert_eq!(kinds, ["link.added", "link.added", "link.removed"]);
    }

    #[test]
    fn unlinking_a_pair_that_is_not_linked_is_a_quiet_no_op() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);

        assert!(!store
            .remove_link(OwnerKind::Session, "s1", "1", T1, &user())
            .unwrap());

        assert!(link_events(&store).is_empty());
    }
}

struct Fixture {
    state: TempDir,
    fake: FakeEnv,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let state = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.vars.set("USER", "ali");
        let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
        let session = Session::new("s1", "claude", "agent-s1", "/work/s1", T0);
        store.upsert_session(&session).unwrap();
        let fixture = Self { state, fake, store };
        fixture
            .run(Command::Project {
                action: ProjectAction::Add,
                name: "p1".to_owned(),
                value: None,
            })
            .unwrap();
        fixture
    }

    fn run(&self, command: Command) -> Result<Output, Error> {
        run_in(command, &self.fake.env, Some(self.state.path()))
    }

    fn link(&self, owner: &str, node: &str) -> Result<Output, Error> {
        self.run(Command::Link {
            owner: owner.to_owned(),
            cortex: node.to_owned(),
            kind: "task".to_owned(),
            doc: Some("7".to_owned()),
        })
    }

    fn unlink(&self, owner: &str, node: &str) -> Result<Output, Error> {
        self.run(Command::Unlink {
            owner: owner.to_owned(),
            cortex: node.to_owned(),
        })
    }
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&output.render(true)).unwrap()
}

mod commands {
    use super::*;

    #[test]
    fn link_and_unlink_parse_into_commands() {
        let cli = Cli::try_parse_from(["claude-monitor", "link", "s1", "42"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Link {
                owner: "s1".to_owned(),
                cortex: "42".to_owned(),
                kind: "task".to_owned(),
                doc: None,
            }
        );
        let cli = Cli::try_parse_from([
            "claude-monitor",
            "link",
            "p1",
            "42",
            "--kind",
            "memory",
            "--doc",
            "7",
        ])
        .unwrap();
        assert_eq!(
            cli.command(),
            Command::Link {
                owner: "p1".to_owned(),
                cortex: "42".to_owned(),
                kind: "memory".to_owned(),
                doc: Some("7".to_owned()),
            }
        );
        let cli = Cli::try_parse_from(["claude-monitor", "unlink", "s1", "42"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Unlink {
                owner: "s1".to_owned(),
                cortex: "42".to_owned(),
            }
        );
        assert!(
            Cli::try_parse_from(["claude-monitor", "link", "s1", "42", "--kind", "x"]).is_err()
        );
    }

    #[test]
    fn linking_a_session_stores_the_row_with_the_user_as_author() {
        let fix = Fixture::new();

        let output = fix.link("s1", "42").unwrap();

        let value = json(&output);
        assert_eq!(value["ok"], true);
        assert_eq!(value["created"], true);
        assert_eq!(value["link"]["owner"]["kind"], "session");
        assert_eq!(value["link"]["owner"]["id"], "s1");
        assert_eq!(value["link"]["node_id"], "42");
        let links = fix.store.links_for(OwnerKind::Session, "s1").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].cortex_doc_id.as_deref(), Some("7"));
        assert_eq!(
            link_events(&fix.store),
            vec![(
                "link.added".to_owned(),
                "2026-10-12T00:00:00Z".to_owned(),
                user()
            )]
        );
    }

    #[test]
    fn linking_a_project_by_name_and_linking_twice_is_idempotent() {
        let fix = Fixture::new();
        fix.link("p1", "42").unwrap();

        let again = fix.link("p1", "42").unwrap();

        assert_eq!(json(&again)["created"], false);
        let project = fix.store.project_by_name("p1").unwrap().unwrap();
        assert_eq!(
            fix.store
                .links_for(OwnerKind::Project, &project.id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(link_events(&fix.store).len(), 1);
    }

    #[test]
    fn an_unknown_owner_is_refused_and_nothing_is_written() {
        let fix = Fixture::new();

        let err = fix.link("nobody", "42").unwrap_err();

        assert!(matches!(err, Error::Refused { .. }), "{err:?}");
        assert!(fix.store.all_links().unwrap().is_empty());
    }

    #[test]
    fn a_node_id_that_is_not_a_number_is_a_usage_error() {
        let fix = Fixture::new();

        let err = fix.link("s1", "not-a-number").unwrap_err();

        assert!(matches!(err, Error::Usage(_)), "{err:?}");
        let err = fix
            .run(Command::Link {
                owner: "s1".to_owned(),
                cortex: "42".to_owned(),
                kind: "task".to_owned(),
                doc: Some("x".to_owned()),
            })
            .unwrap_err();
        assert!(matches!(err, Error::Usage(_)), "{err:?}");
        assert!(fix.store.all_links().unwrap().is_empty());
    }

    #[test]
    fn unlink_removes_the_row_and_a_second_unlink_reports_no_change() {
        let fix = Fixture::new();
        fix.link("s1", "42").unwrap();

        let first = fix.unlink("s1", "42").unwrap();
        let second = fix.unlink("s1", "42").unwrap();

        assert_eq!(json(&first)["changed"], true);
        assert_eq!(json(&second)["changed"], false);
        assert!(fix.store.all_links().unwrap().is_empty());
    }
}
