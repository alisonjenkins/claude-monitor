#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The `respawn_request` rows the respawn command writes and the hook answers.

use claude_monitor::model::Session;
use claude_monitor::store::respawn::OUTCOME_STARTED;
use claude_monitor::store::{Store, StoreOptions};
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";

fn store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    (dir, store)
}

fn session(store: &Store, id: &str) -> Session {
    let session = Session::new(id, "claude", &format!("agent-{id}"), "/w", T0);
    store.upsert_session(&session).unwrap();
    session
}

#[test]
fn a_created_request_is_open_for_its_pane() {
    let (_dir, store) = store();
    let s = session(&store, "s1");

    let created = store.create_respawn_request(&s.id, "%3", T0).unwrap();

    assert_eq!(created.session_id, s.id);
    assert_eq!(created.tmux_target.as_deref(), Some("%3"));
    assert_eq!(created.requested_at, T0);
    assert_eq!(created.outcome, OUTCOME_STARTED);
    assert_eq!(created.new_session_id, None);
    assert_eq!(
        store.find_open_respawn_request("%3").unwrap(),
        Some(created)
    );
    assert_eq!(store.find_open_respawn_request("%4").unwrap(), None);
}

#[test]
fn creating_the_same_request_twice_returns_the_open_one() {
    let (_dir, store) = store();
    let s = session(&store, "s1");

    let first = store.create_respawn_request(&s.id, "%3", T0).unwrap();
    let second = store.create_respawn_request(&s.id, "%3", T0).unwrap();

    assert_eq!(first.id, second.id);
    let rows: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM respawn_request", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn closing_a_request_records_the_new_session_and_ends_it() {
    let (_dir, store) = store();
    let old = session(&store, "s1");
    let new = session(&store, "s2");
    let request = store.create_respawn_request(&old.id, "%3", T0).unwrap();

    store.close_respawn_request(&request.id, &new.id).unwrap();

    assert_eq!(store.find_open_respawn_request("%3").unwrap(), None);
    let new_id: String = store
        .connection()
        .query_row(
            "SELECT new_session_id FROM respawn_request WHERE id = ?1",
            [&request.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_id, new.id);
}

#[test]
fn closing_an_unknown_request_is_an_error() {
    let (_dir, store) = store();
    let s = session(&store, "s1");
    assert!(store.close_respawn_request("nope", &s.id).is_err());
}

#[test]
fn the_newest_open_request_for_a_pane_wins() {
    let (_dir, store) = store();
    let a = session(&store, "s1");
    let b = session(&store, "s2");
    store.create_respawn_request(&a.id, "%3", T0).unwrap();
    let newer = store
        .create_respawn_request(&b.id, "%3", "2026-10-12T10:00:05Z")
        .unwrap();

    assert_eq!(store.find_open_respawn_request("%3").unwrap(), Some(newer));
}
