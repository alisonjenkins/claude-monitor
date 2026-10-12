#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! How a `SessionStart` is tied to the respawn that caused it: a resume keeps
//! the id and revives the row, an unexpected new id in a respawned pane is
//! linked through the pane's open request, and only `/clear` links otherwise.

use std::path::PathBuf;

use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeClock;
use serde_json::Value;
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T10:00:05Z";

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    clock: FakeClock,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        Self {
            state_dir: dir.path().join("state"),
            _dir: dir,
            clock: FakeClock::new(T0),
        }
    }

    fn feed(&self, pane: Option<&str>, fixture: &str, id: &str) {
        let path = format!(
            "{}/tests/fixtures/claude/{fixture}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let mut value: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        value["session_id"] = Value::String(id.to_owned());
        let ctx = HookContext {
            agent: "claude",
            state_dir: &self.state_dir,
            tmux_pane: pane,
            clock: &self.clock,
            git: &NoGit,
            agent_pid: None,
            env: None,
        };
        ingest(&ctx, &value.to_string());
    }

    fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::hook()).unwrap()
    }

    fn session(&self, agent_session_id: &str) -> Session {
        self.store()
            .get_session_by_agent_id("claude", agent_session_id)
            .unwrap()
            .expect("session row exists")
    }

    fn kinds_of(&self, session: &Session) -> Vec<String> {
        self.store()
            .events_since(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|row| row.entity_id == session.id)
            .map(|row| row.kind)
            .collect()
    }

    fn ended_session(&self, id: &str, pane: &str) -> Session {
        self.feed(Some(pane), "SessionStart-startup", id);
        self.feed(Some(pane), "SessionEnd-prompt_input_exit", id);
        self.session(id)
    }
}

fn new_ids(store: &Store) -> usize {
    store.list_sessions().unwrap().len()
}

#[test]
fn resume_of_a_known_id_revives_the_row_without_a_link() {
    let h = Harness::new();
    let ended = h.ended_session("a", "%1");

    h.clock.set(T1);
    h.feed(Some("%1"), "SessionStart-resume", "a");

    let revived = h.session("a");
    assert_eq!(revived.id, ended.id);
    assert_eq!(revived.state, State::Idle);
    assert_eq!(revived.resumed_from, None);
    assert_eq!(new_ids(&h.store()), 1);
}

#[test]
#[ignore = "T071: session.resumed event"]
fn resume_of_a_known_id_appends_a_resumed_event() {
    let h = Harness::new();
    let ended = h.ended_session("a", "%1");

    h.clock.set(T1);
    h.feed(Some("%1"), "SessionStart-resume", "a");

    assert!(
        h.kinds_of(&ended).contains(&"session.resumed".to_owned()),
        "{:?}",
        h.kinds_of(&ended)
    );
}

#[test]
fn a_first_start_is_not_a_resumed_event() {
    let h = Harness::new();
    h.feed(Some("%1"), "SessionStart-startup", "a");
    assert!(!h
        .kinds_of(&h.session("a"))
        .contains(&"session.resumed".to_owned()));
}

#[test]
#[ignore = "T071: answer the open request on resume"]
fn resume_in_the_requested_pane_closes_the_request_with_the_same_session() {
    let h = Harness::new();
    let ended = h.ended_session("a", "%1");
    let request = h
        .store()
        .create_respawn_request(&ended.id, "%1", T0)
        .unwrap();

    h.clock.set(T1);
    h.feed(Some("%1"), "SessionStart-resume", "a");

    assert_eq!(h.store().find_open_respawn_request("%1").unwrap(), None);
    let answered: String = h
        .store()
        .connection()
        .query_row(
            "SELECT new_session_id FROM respawn_request WHERE id = ?1",
            [&request.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(answered, ended.id);
    assert_eq!(new_ids(&h.store()), 1);
}

#[test]
#[ignore = "T071: link an unexpected id through the open request"]
fn an_unexpected_new_id_in_a_respawned_pane_is_linked_by_the_request() {
    let h = Harness::new();
    let old = h.ended_session("a", "%1");
    let request = h.store().create_respawn_request(&old.id, "%1", T0).unwrap();

    h.clock.set(T1);
    h.feed(Some("%1"), "SessionStart-startup", "minted");

    let minted = h.session("minted");
    assert_eq!(minted.resumed_from.as_deref(), Some(old.id.as_str()));
    assert_eq!(h.store().find_open_respawn_request("%1").unwrap(), None);
    let answered: String = h
        .store()
        .connection()
        .query_row(
            "SELECT new_session_id FROM respawn_request WHERE id = ?1",
            [&request.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(answered, minted.id);
}

#[test]
fn a_new_id_in_another_pane_is_not_linked() {
    let h = Harness::new();
    let old = h.ended_session("a", "%1");
    h.store().create_respawn_request(&old.id, "%1", T0).unwrap();

    h.feed(Some("%2"), "SessionStart-startup", "other");

    assert_eq!(h.session("other").resumed_from, None);
    assert!(h.store().find_open_respawn_request("%1").unwrap().is_some());
}

#[test]
fn a_new_id_with_no_request_is_not_linked() {
    let h = Harness::new();
    h.ended_session("a", "%1");

    h.feed(Some("%1"), "SessionStart-startup", "fresh");

    assert_eq!(h.session("fresh").resumed_from, None);
}

#[test]
fn clear_links_by_pane_even_with_an_open_request_elsewhere() {
    let h = Harness::new();
    h.feed(Some("%1"), "SessionStart-startup", "a");
    h.feed(Some("%1"), "SessionEnd-clear", "a");
    let other = h.ended_session("z", "%9");
    h.store()
        .create_respawn_request(&other.id, "%2", T0)
        .unwrap();

    h.feed(Some("%1"), "SessionStart-clear", "b");

    assert_eq!(
        h.session("b").resumed_from.as_deref(),
        Some(h.session("a").id.as_str())
    );
}
