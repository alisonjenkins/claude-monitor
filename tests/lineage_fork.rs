#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! Lineage: a fork is its own session with its own chain, while a respawn
//! linked through a `RespawnRequest` stays in the original's chain.

use std::path::PathBuf;

use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::Session;
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

    fn feed(&self, pane: &str, fixture: &str, id: &str) {
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
            tmux_pane: Some(pane),
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
}

#[test]
fn a_fork_is_a_separate_session_outside_the_chain() {
    let h = Harness::new();
    h.feed("%1", "SessionStart-startup", "a");
    h.feed("%1", "SessionEnd-clear", "a");
    h.feed("%1", "SessionStart-clear", "b");
    h.feed("%2", "SessionStart-startup", "fork");
    let (a, b, fork) = (h.session("a"), h.session("b"), h.session("fork"));
    let store = h.store();

    assert!(store.record_fork(&b.id, &fork.id).unwrap());

    let fork = h.session("fork");
    assert_eq!(fork.forked_from.as_deref(), Some(b.id.as_str()));
    assert_eq!(fork.resumed_from, None);
    assert_eq!(
        store.chain_head(&a.id).unwrap(),
        b.id,
        "the fork is no head"
    );
    assert_eq!(store.chain_head(&b.id).unwrap(), b.id);
    assert_eq!(store.chain_head(&fork.id).unwrap(), fork.id);
    assert_eq!(store.resumed_by(&b.id).unwrap(), Vec::<String>::new());
}

#[test]
fn a_fork_starts_its_own_chain() {
    let h = Harness::new();
    h.feed("%1", "SessionStart-startup", "a");
    h.feed("%2", "SessionStart-startup", "fork");
    h.feed("%2", "SessionEnd-clear", "fork");
    h.feed("%2", "SessionStart-clear", "fork-next");
    let (a, fork, next) = (h.session("a"), h.session("fork"), h.session("fork-next"));
    let store = h.store();
    store.record_fork(&a.id, &fork.id).unwrap();

    assert_eq!(store.chain_head(&fork.id).unwrap(), next.id);
    assert_eq!(store.chain_head(&a.id).unwrap(), a.id);
}

#[test]
fn chain_head_follows_several_clears() {
    let h = Harness::new();
    h.feed("%1", "SessionStart-startup", "a");
    h.feed("%1", "SessionEnd-clear", "a");
    h.feed("%1", "SessionStart-clear", "b");
    h.feed("%1", "SessionEnd-clear", "b");
    h.feed("%1", "SessionStart-clear", "c");

    assert_eq!(
        h.store().chain_head(&h.session("a").id).unwrap(),
        h.session("c").id
    );
}

#[test]
#[ignore = "T071: an unexpected new id is linked through the pane's request"]
fn a_respawn_linked_through_a_request_still_collapses() {
    let h = Harness::new();
    h.feed("%1", "SessionStart-startup", "a");
    h.feed("%1", "SessionEnd-prompt_input_exit", "a");
    let a = h.session("a");
    h.store().create_respawn_request(&a.id, "%1", T0).unwrap();

    h.clock.set(T1);
    h.feed("%1", "SessionStart-startup", "a-new-id");

    let minted = h.session("a-new-id");
    assert_eq!(minted.resumed_from.as_deref(), Some(a.id.as_str()));
    assert_eq!(minted.forked_from, None);
    assert_eq!(h.store().chain_head(&a.id).unwrap(), minted.id);
}
