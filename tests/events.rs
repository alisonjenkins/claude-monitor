// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::thread;

use claude_monitor::model::Author;
use claude_monitor::store::events::{EventRow, NewEvent};
use claude_monitor::store::{Store, StoreOptions};
use serde_json::{json, Value};
use tempfile::TempDir;

const AT: &str = "2026-10-12T14:03:22Z";

fn db_path(dir: &TempDir) -> PathBuf {
    dir.path().join("state.db")
}

fn open(dir: &TempDir) -> Store {
    Store::open(&db_path(dir), &StoreOptions::default()).unwrap()
}

fn system() -> Author {
    Author::System {
        component: "hook".to_owned(),
    }
}

fn append(store: &Store, id: &str, kind: &str, payload: &Value, author: &Author) -> i64 {
    store
        .append_event(&NewEvent {
            at: AT,
            entity_kind: "session",
            entity_id: id,
            kind,
            payload,
            author,
        })
        .unwrap()
}

fn append_n(store: &Store, count: usize) -> Vec<i64> {
    (0..count)
        .map(|i| {
            append(
                store,
                &format!("s-{i}"),
                "state",
                &json!({ "i": i }),
                &system(),
            )
        })
        .collect()
}

#[test]
fn append_returns_a_monotonically_increasing_seq() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);

    let seqs = append_n(&store, 5);

    assert_eq!(seqs.first(), Some(&1));
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    assert_eq!(store.last_seq().unwrap(), *seqs.last().unwrap());
}

#[test]
fn last_seq_of_an_empty_table_is_zero() {
    let dir = TempDir::new().unwrap();

    assert_eq!(open(&dir).last_seq().unwrap(), 0);
}

#[test]
fn events_since_returns_only_newer_rows_in_order() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let seqs = append_n(&store, 5);

    let rows = store.events_since(seqs[1], 100).unwrap();

    let got: Vec<i64> = rows.iter().map(|row| row.seq).collect();
    assert_eq!(got, seqs[2..].to_vec());
    assert_eq!(store.events_since(0, 100).unwrap().len(), 5);
}

#[test]
fn events_since_honours_the_limit_and_resumes_from_the_last_row() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let seqs = append_n(&store, 5);

    let first = store.events_since(0, 2).unwrap();
    let next = store.events_since(first.last().unwrap().seq, 2).unwrap();
    let rest = store.events_since(next.last().unwrap().seq, 2).unwrap();

    let got: Vec<i64> = first
        .iter()
        .chain(&next)
        .chain(&rest)
        .map(|row| row.seq)
        .collect();
    assert_eq!((first.len(), next.len(), rest.len()), (2, 2, 1));
    assert_eq!(got, seqs);
}

#[test]
fn events_since_a_cursor_beyond_the_end_is_empty() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let seqs = append_n(&store, 3);

    assert!(store.events_since(seqs[2], 10).unwrap().is_empty());
    assert!(store.events_since(9_999, 10).unwrap().is_empty());
}

#[test]
fn a_row_carries_every_column() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let payload = json!({ "from": "idle", "to": "working", "nested": { "n": [1, 2, 3] } });
    let author = system();

    let seq = append(&store, "s-42", "state_changed", &payload, &author);

    let rows = store.events_since(0, 10).unwrap();
    assert_eq!(
        rows,
        vec![EventRow {
            seq,
            at: AT.to_owned(),
            entity_kind: "session".to_owned(),
            entity_id: "s-42".to_owned(),
            kind: "state_changed".to_owned(),
            payload,
            author,
        }]
    );
}

#[test]
fn every_author_kind_round_trips() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let authors = [
        Author::User {
            name: "ali".to_owned(),
        },
        Author::Agent {
            kind: "claude".to_owned(),
            model: Some("claude-opus-5-5".to_owned()),
        },
        Author::Agent {
            kind: "copilot".to_owned(),
            model: None,
        },
        Author::System {
            component: "reconcile".to_owned(),
        },
    ];
    for author in &authors {
        append(&store, "s-1", "note_added", &json!({}), author);
    }

    let got: Vec<Author> = store
        .events_since(0, 10)
        .unwrap()
        .into_iter()
        .map(|row| row.author)
        .collect();

    assert_eq!(got, authors.to_vec());
}

#[test]
fn author_columns_hold_kind_and_name() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    append(
        &store,
        "s-1",
        "x",
        &json!({}),
        &Author::Agent {
            kind: "claude".to_owned(),
            model: Some("claude-opus-5-5".to_owned()),
        },
    );

    let (kind, name): (String, String) = store
        .connection()
        .query_row("SELECT author_kind, author_name FROM events", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();

    assert_eq!(kind, "agent");
    assert!(name.contains("claude-opus-5-5"), "{name}");
    assert!(name.starts_with("claude"), "{name}");
}

#[test]
fn update_on_events_is_rejected() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    append_n(&store, 1);

    let err = store
        .connection()
        .execute("UPDATE events SET kind = 'tampered'", [])
        .unwrap_err();

    assert!(err.to_string().contains("append-only"), "{err}");
    let kind: String = store
        .connection()
        .query_row("SELECT kind FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kind, "state");
}

#[test]
fn delete_on_events_is_rejected() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    append_n(&store, 2);

    let err = store
        .connection()
        .execute("DELETE FROM events", [])
        .unwrap_err();

    assert!(err.to_string().contains("append-only"), "{err}");
    assert_eq!(store.last_seq().unwrap(), 2);
    assert_eq!(store.events_since(0, 10).unwrap().len(), 2);
}

#[test]
fn unchanged_payload_records_nothing() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let payload = json!({ "state": "idle" });
    let author = system();

    let recorded = store
        .append_event_if_changed(
            &payload,
            &NewEvent {
                at: AT,
                entity_kind: "session",
                entity_id: "s-1",
                kind: "state_changed",
                payload: &payload,
                author: &author,
            },
        )
        .unwrap();

    assert_eq!(recorded, None);
    assert_eq!(store.last_seq().unwrap(), 0);
}

#[test]
fn changed_payload_records_one_row() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let old = json!({ "state": "idle" });
    let new = json!({ "state": "working" });
    let author = system();

    let recorded = store
        .append_event_if_changed(
            &old,
            &NewEvent {
                at: AT,
                entity_kind: "session",
                entity_id: "s-1",
                kind: "state_changed",
                payload: &new,
                author: &author,
            },
        )
        .unwrap();

    assert_eq!(recorded, Some(1));
    let rows = store.events_since(0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].payload, new);
}

#[test]
fn seq_survives_close_and_reopen() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let before = append_n(&store, 3);
    drop(store);

    let reopened = open(&dir);
    let next = append(&reopened, "s-9", "state", &json!({}), &system());

    assert_eq!(reopened.last_seq().unwrap(), next);
    assert_eq!(next, before.last().unwrap() + 1);
    assert_eq!(reopened.events_since(0, 10).unwrap().len(), 4);
}

#[test]
fn concurrent_writers_never_lose_or_duplicate_a_seq() {
    const THREADS: usize = 4;
    const PER_THREAD: usize = 50;
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    drop(open(&dir));

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let path = path.clone();
            thread::spawn(move || {
                let store = Store::open(&path, &StoreOptions::default()).unwrap();
                (0..PER_THREAD)
                    .map(|i| {
                        append(
                            &store,
                            &format!("t{t}-{i}"),
                            "state",
                            &json!({ "t": t, "i": i }),
                            &system(),
                        )
                    })
                    .collect::<Vec<i64>>()
            })
        })
        .collect();
    let seqs: Vec<i64> = handles
        .into_iter()
        .flat_map(|handle| handle.join().unwrap())
        .collect();

    let distinct: BTreeSet<i64> = seqs.iter().copied().collect();
    assert_eq!(seqs.len(), THREADS * PER_THREAD);
    assert_eq!(distinct.len(), THREADS * PER_THREAD);
    let store = open(&dir);
    assert_eq!(store.last_seq().unwrap(), 200);
    assert_eq!(store.events_since(0, 1_000).unwrap().len(), 200);
}
