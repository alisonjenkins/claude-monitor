//! Links between sessions or projects and external graph nodes (T096, T100,
//! T101, FR-026, FR-028): rows and events, the `link`/`unlink` commands, and
//! `links check` against a fake `cortexiphan` executable on a temporary PATH.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use claude_monitor::model::{Author, OwnerKind};
use claude_monitor::store::links::NewLink;
use claude_monitor::store::{Store, StoreOptions};
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
