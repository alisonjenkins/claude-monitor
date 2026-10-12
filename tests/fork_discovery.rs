#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! Claude writes no parent link into a forked transcript; the only trace is
//! the message uuids it copied. Discovery records `forked_from` from them.

use std::path::PathBuf;
use std::sync::Arc;

use claude_monitor::agents::claude_discover::{discover_and_store, READ_WINDOW_BYTES};
use claude_monitor::env::Env;
use claude_monitor::model::Session;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeVars;
use serde_json::{json, Value};
use tempfile::TempDir;

const CWD: &str = "/work/proj";

struct Fx {
    _dir: TempDir,
    project: PathBuf,
    state_dir: PathBuf,
    env: Env,
}

impl Fx {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("claude-config");
        let project = config.join("projects").join("-work-proj");
        std::fs::create_dir_all(&project).unwrap();
        let vars = Arc::new(FakeVars::default());
        vars.set("CLAUDE_CONFIG_DIR", config.to_str().unwrap());
        let mut env = Env::real();
        env.vars = vars;
        Self {
            state_dir: dir.path().join("state"),
            _dir: dir,
            project,
            env,
        }
    }

    fn write(&self, id: &str, lines: &[Value]) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(self.project.join(format!("{id}.jsonl")), text).unwrap();
    }

    fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::default()).unwrap()
    }

    fn discover(&self) {
        discover_and_store(&self.env, &self.store()).unwrap();
    }

    fn session(&self, id: &str) -> Session {
        self.store()
            .get_session_by_agent_id("claude", id)
            .unwrap()
            .expect("session discovered")
    }
}

/// `uuids.len()` message lines for session `id`, one second apart from `start`.
fn transcript(id: &str, uuids: &[String], start: u32) -> Vec<Value> {
    uuids
        .iter()
        .enumerate()
        .map(|(n, uuid)| {
            let at = format!("2026-10-12T09:{:02}:{:02}.000Z", start, n);
            json!({
                "type": if n % 2 == 0 { "user" } else { "assistant" },
                "uuid": uuid, "sessionId": id, "timestamp": at, "cwd": CWD,
                "isSidechain": false,
                "message": {"role": "user", "content": format!("line {n}")},
            })
        })
        .collect()
}

fn uuids(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|n| format!("{prefix}-{n}")).collect()
}

fn fork_of(parent: &[String], shared: usize, own: &str) -> Vec<String> {
    let mut all: Vec<String> = parent.iter().take(shared).cloned().collect();
    all.extend(uuids(own, 4));
    all
}

#[test]
fn a_new_transcript_sharing_leading_uuids_is_recorded_as_a_fork() {
    let fx = Fx::new();
    let parent = uuids("p", 8);
    fx.write("parent", &transcript("parent", &parent, 0));
    fx.discover();

    fx.write("child", &transcript("child", &fork_of(&parent, 6, "c"), 10));
    fx.discover();

    let (parent_row, child_row) = (fx.session("parent"), fx.session("child"));
    assert_eq!(
        child_row.forked_from.as_deref(),
        Some(parent_row.id.as_str())
    );
    assert_eq!(child_row.resumed_from, None);
    assert_eq!(parent_row.forked_from, None);
    let kinds: Vec<String> = fx
        .store()
        .events_since(0, 1000)
        .unwrap()
        .into_iter()
        .filter(|e| e.entity_id == child_row.id)
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&"session.forked".to_owned()), "{kinds:?}");
}

#[test]
fn rediscovery_records_nothing_more() {
    let fx = Fx::new();
    let parent = uuids("p", 8);
    fx.write("parent", &transcript("parent", &parent, 0));
    fx.discover();
    fx.write("child", &transcript("child", &fork_of(&parent, 6, "c"), 10));
    fx.discover();
    let before = fx.store().last_seq().unwrap();

    fx.discover();

    assert_eq!(fx.store().last_seq().unwrap(), before);
}

#[test]
fn the_older_transcript_is_the_parent_when_both_are_new() {
    let fx = Fx::new();
    let parent = uuids("p", 8);
    fx.write("parent", &transcript("parent", &parent, 0));
    fx.write("child", &transcript("child", &fork_of(&parent, 6, "c"), 10));

    fx.discover();

    assert_eq!(
        fx.session("child").forked_from.as_deref(),
        Some(fx.session("parent").id.as_str())
    );
    assert_eq!(fx.session("parent").forked_from, None);
}

#[test]
fn unrelated_transcripts_stay_independent() {
    let fx = Fx::new();
    fx.write("one", &transcript("one", &uuids("a", 8), 0));
    fx.discover();
    fx.write("two", &transcript("two", &uuids("b", 8), 10));
    fx.discover();

    assert_eq!(fx.session("one").forked_from, None);
    assert_eq!(fx.session("two").forked_from, None);
}

#[test]
fn a_single_shared_uuid_is_not_enough() {
    let fx = Fx::new();
    let parent = uuids("p", 8);
    fx.write("parent", &transcript("parent", &parent, 0));
    fx.discover();
    fx.write("child", &transcript("child", &fork_of(&parent, 1, "c"), 10));
    fx.discover();

    assert_eq!(fx.session("child").forked_from, None);
}

#[test]
fn identical_start_times_with_both_new_is_ambiguous_so_independent() {
    let fx = Fx::new();
    let parent = uuids("p", 8);
    fx.write("a", &transcript("a", &parent, 0));
    fx.write("b", &transcript("b", &fork_of(&parent, 6, "c"), 0));
    fx.discover();

    assert_eq!(fx.session("a").forked_from, None);
    assert_eq!(fx.session("b").forked_from, None);
}

#[test]
fn a_fork_deep_in_a_huge_transcript_is_found_from_the_head_only() {
    let fx = Fx::new();
    let parent = uuids("p", 8);
    let mut long = transcript("parent", &parent, 0);
    let pad = "x".repeat(READ_WINDOW_BYTES);
    long.push(json!({
        "type": "user", "uuid": "tail-only", "sessionId": "parent", "timestamp":
        "2026-10-12T09:50:00.000Z", "cwd": CWD, "message": {"content": pad}
    }));
    fx.write("parent", &long);
    fx.discover();
    fx.write("child", &transcript("child", &fork_of(&parent, 6, "c"), 10));
    fx.discover();

    assert_eq!(
        fx.session("child").forked_from.as_deref(),
        Some(fx.session("parent").id.as_str())
    );
}

#[test]
fn a_clear_chain_member_is_not_taken_for_a_fork_of_itself() {
    let fx = Fx::new();
    let parent = uuids("p", 8);
    fx.write("parent", &transcript("parent", &parent, 0));
    fx.discover();
    let store = fx.store();
    let parent_row = fx.session("parent");
    let mut next = Session::new("manual", "claude", "next", CWD, "2026-10-12T09:20:00Z");
    next.resumed_from = Some(parent_row.id.clone());
    store.upsert_session(&next).unwrap();
    fx.write("next", &transcript("next", &fork_of(&parent, 6, "n"), 20));

    fx.discover();

    assert_eq!(fx.session("next").forked_from, None);
}
