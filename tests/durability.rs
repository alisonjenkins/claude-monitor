//! Sessions that die without a `SessionEnd` are detected by `reconcile`
//! (FR-005..FR-008, SC-003, SC-005).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::PathBuf;

use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::{Author, Session, State};
use claude_monitor::reconcile::{reconcile, ReconcileReport, Scope};
use claude_monitor::store::queries::{Overview, OverviewQuery, SectionName};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::Value;
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T10:05:00Z";
const T2: &str = "2026-10-12T10:10:00Z";
const AGENT_PID: u32 = 4242;
const PID_START: u64 = 987_654;

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    fake: FakeEnv,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.clock.set(T0);
        Self {
            state_dir: dir.path().join("state"),
            _dir: dir,
            fake,
        }
    }

    fn feed(&self, pane: Option<&str>, pid: Option<u32>, raw: &str) {
        let ctx = HookContext {
            agent: "claude",
            state_dir: &self.state_dir,
            tmux_pane: pane,
            clock: self.fake.clock.as_ref(),
            git: &NoGit,
            agent_pid: pid,
            env: Some(&self.fake.env),
        };
        ingest(&ctx, raw);
    }

    fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer()).unwrap()
    }

    fn session(&self, agent_session_id: &str) -> Session {
        self.store()
            .get_session_by_agent_id("claude", agent_session_id)
            .unwrap()
            .expect("session row exists")
    }

    fn reconcile(&self, scope: Scope, now: &str) -> ReconcileReport {
        reconcile(&self.store(), &self.fake.env, scope, now).unwrap()
    }
}

fn payload(name: &str, id: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/claude/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    value["session_id"] = Value::String(id.to_owned());
    value.to_string()
}

fn section_ids(overview: &Overview, name: SectionName) -> Vec<String> {
    overview
        .sections
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.sessions.iter().map(|e| e.id.clone()).collect())
        .unwrap_or_default()
}

#[test]
#[ignore = "T051"]
fn hook_records_pid_start_time_and_boot_id() {
    let h = Harness::new();
    h.fake.processes.spawn(AGENT_PID, PID_START);

    h.feed(
        Some("%1"),
        Some(AGENT_PID),
        &payload("SessionStart-startup", "s1"),
    );

    let s = h.session("s1");
    assert_eq!(s.pid, Some(AGENT_PID));
    assert_eq!(s.pid_start, Some(PID_START));
    assert_eq!(s.boot_id.as_deref(), Some("boot-1"));
}

#[test]
#[ignore = "T052"]
fn killed_agent_without_session_end_becomes_interrupted() {
    let h = Harness::new();
    h.fake.processes.spawn(AGENT_PID, PID_START);
    h.feed(
        Some("%1"),
        Some(AGENT_PID),
        &payload("SessionStart-startup", "s1"),
    );
    h.feed(
        Some("%1"),
        Some(AGENT_PID),
        &payload("UserPromptSubmit", "s1"),
    );
    let before = h.session("s1");
    assert_eq!(before.state, State::Working);
    let events_before = h.store().events_since(0, 1000).unwrap().len();

    h.fake.processes.kill(AGENT_PID);
    let report = h.reconcile(Scope::All, T1);

    assert_eq!((report.checked, report.interrupted), (1, 1));
    let after = h.session("s1");
    assert_eq!(after.state, State::Interrupted);
    assert_eq!(after.state_since, T1);

    let events = h.store().events_since(0, 1000).unwrap();
    assert_eq!(events.len(), events_before + 1);
    let event = events.last().unwrap();
    assert_eq!(event.kind, "session.state_changed");
    assert_eq!(event.entity_id, after.id);
    assert_eq!(
        event.author,
        Author::System {
            component: "reconcile".to_owned()
        }
    );
    assert_eq!(event.payload["state"], "interrupted");
    assert_eq!(event.payload["reason"], "process_gone");

    let overview = h.store().overview(&OverviewQuery::default(), T1).unwrap();
    assert_eq!(
        section_ids(&overview, SectionName::Interrupted),
        std::slice::from_ref(&after.id)
    );
    assert!(section_ids(&overview, SectionName::Active).is_empty());
}

#[test]
#[ignore = "T052"]
fn second_reconcile_changes_and_appends_nothing() {
    let h = Harness::new();
    h.fake.processes.spawn(AGENT_PID, PID_START);
    h.feed(
        Some("%1"),
        Some(AGENT_PID),
        &payload("SessionStart-startup", "s1"),
    );
    h.fake.processes.kill(AGENT_PID);
    h.reconcile(Scope::All, T1);
    let after_first = h.session("s1");
    let events_after_first = h.store().events_since(0, 1000).unwrap().len();

    let report = h.reconcile(Scope::All, T2);

    assert_eq!(report.interrupted, 0);
    assert_eq!(report.unchanged, 1);
    assert_eq!(h.session("s1"), after_first);
    assert_eq!(
        h.store().events_since(0, 1000).unwrap().len(),
        events_after_first
    );
}

#[test]
#[ignore = "T052"]
fn recycled_pid_counts_as_gone() {
    let h = Harness::new();
    h.fake.processes.spawn(AGENT_PID, PID_START);
    h.feed(
        Some("%1"),
        Some(AGENT_PID),
        &payload("SessionStart-startup", "s1"),
    );
    h.fake.processes.spawn(AGENT_PID, PID_START + 1);

    let report = h.reconcile(Scope::All, T1);

    assert_eq!(report.interrupted, 1);
    assert_eq!(h.session("s1").state, State::Interrupted);
}

#[test]
#[ignore = "T052"]
fn changed_boot_id_interrupts_every_unfinished_session_in_one_pass() {
    let h = Harness::new();
    for (id, pane, pid, event) in [
        ("working", "%1", 100, "UserPromptSubmit"),
        ("idle", "%2", 101, "SessionStart-startup"),
        ("done", "%3", 102, "SessionEnd-other"),
    ] {
        h.fake.processes.spawn(pid, PID_START);
        h.feed(Some(pane), Some(pid), &payload("SessionStart-startup", id));
        h.feed(Some(pane), Some(pid), &payload(event, id));
    }
    h.store()
        .upsert_session(&Session::new(
            "passive",
            "claude",
            "agent-passive",
            "/work/passive",
            T0,
        ))
        .unwrap();
    assert_eq!(h.session("working").state, State::Working);
    assert_eq!(h.session("idle").state, State::Idle);
    assert_eq!(h.session("done").state, State::Finished);
    let events_before = h.store().events_since(0, 1000).unwrap().len();

    h.fake.boot_id.set("boot-2");
    let report = h.reconcile(Scope::All, T1);

    assert_eq!(report.interrupted, 2);
    assert_eq!(h.session("working").state, State::Interrupted);
    assert_eq!(h.session("idle").state, State::Interrupted);
    assert_eq!(h.session("done").state, State::Finished);
    let passive = h.store().get_session("passive").unwrap().unwrap();
    assert_eq!(passive.state, State::Unknown);

    let events = h.store().events_since(0, 1000).unwrap();
    let new_events = &events[events_before..];
    assert_eq!(new_events.len(), 2);
    for event in new_events {
        assert_eq!(event.kind, "session.state_changed");
        assert_eq!(event.payload["reason"], "boot_changed");
    }
}

#[test]
#[ignore = "T052"]
fn reboot_reconcile_is_idempotent() {
    let h = Harness::new();
    h.fake.processes.spawn(AGENT_PID, PID_START);
    h.feed(
        Some("%1"),
        Some(AGENT_PID),
        &payload("SessionStart-startup", "s1"),
    );
    h.fake.boot_id.set("boot-2");
    h.reconcile(Scope::All, T1);
    let after_first = h.session("s1");
    let events_after_first = h.store().events_since(0, 1000).unwrap().len();

    let report = h.reconcile(Scope::All, T2);

    assert_eq!((report.interrupted, report.unchanged), (0, 1));
    assert_eq!(h.session("s1"), after_first);
    assert_eq!(
        h.store().events_since(0, 1000).unwrap().len(),
        events_after_first
    );
}

#[test]
#[ignore = "T052"]
fn resumed_session_after_reboot_is_not_interrupted_again() {
    let h = Harness::new();
    h.fake.processes.spawn(AGENT_PID, PID_START);
    h.feed(
        Some("%1"),
        Some(AGENT_PID),
        &payload("SessionStart-startup", "s1"),
    );
    h.fake.boot_id.set("boot-2");
    h.reconcile(Scope::All, T1);
    assert_eq!(h.session("s1").state, State::Interrupted);

    h.fake.processes.spawn(777, 31);
    h.fake.clock.set(T2);
    h.feed(Some("%1"), Some(777), &payload("SessionStart-resume", "s1"));

    let s = h.session("s1");
    assert_eq!(s.state, State::Idle);
    assert_eq!(
        (s.pid, s.pid_start, s.boot_id.as_deref()),
        (Some(777), Some(31), Some("boot-2"))
    );
    let report = h.reconcile(Scope::All, T2);
    assert_eq!((report.interrupted, report.unchanged), (0, 1));
}
