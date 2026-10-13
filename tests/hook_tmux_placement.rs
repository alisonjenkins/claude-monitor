#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! The hook asks tmux where its pane is and records the session, window name
//! and window index, so a respawn can put the session back. A tmux that
//! cannot answer changes nothing and never fails the hook.

use std::path::PathBuf;

use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::tmux::PaneLocation;
use serde_json::Value;
use tempfile::TempDir;

const SESSION: &str = "s1";
const PANE: &str = "%4";

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    fake: FakeEnv,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.fs.add_dir("/tmp/SCRATCH/r2/work");
        Self {
            state_dir: dir.path().join("state"),
            _dir: dir,
            fake,
        }
    }

    fn place_pane(&self, session: &str, window: &str, index: &str) {
        self.fake.tmux.add_pane(
            PANE,
            PaneLocation {
                session_name: session.to_owned(),
                window_name: window.to_owned(),
                window_index: index.to_owned(),
                pane_index: "0".to_owned(),
                window_panes: "1".to_owned(),
                window_layout: String::new(),
            },
        );
    }

    fn feed(&self, fixture: &str) {
        let ctx = HookContext {
            agent: "claude",
            state_dir: &self.state_dir,
            tmux_pane: Some(PANE),
            clock: self.fake.clock.as_ref(),
            git: &NoGit,
            agent_pid: None,
            env: Some(&self.fake.env),
        };
        ingest(&ctx, &payload(fixture));
    }

    fn session(&self) -> Session {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer())
            .unwrap()
            .get_session_by_agent_id("claude", SESSION)
            .unwrap()
            .expect("session row exists")
    }

    fn hook_log(&self) -> String {
        std::fs::read_to_string(self.state_dir.join("hook.log")).unwrap_or_default()
    }
}

fn payload(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/claude/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    value["session_id"] = Value::String(SESSION.to_owned());
    value.to_string()
}

fn placement(session: &Session) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        session.tmux_session.as_deref(),
        session.tmux_window_name.as_deref(),
        session.tmux_window_index.as_deref(),
    )
}

#[test]
fn a_hook_records_the_panes_tmux_session_and_window() {
    let h = Harness::new();
    h.place_pane("my project", "api build", "3");

    h.feed("SessionStart-startup");

    let session = h.session();
    assert_eq!(
        placement(&session),
        (Some("my project"), Some("api build"), Some("3"))
    );
    assert_eq!(session.tmux_pane.as_deref(), Some(PANE));
}

#[test]
fn a_later_hook_follows_a_moved_or_renamed_window() {
    let h = Harness::new();
    h.place_pane("work", "api", "1");
    h.feed("SessionStart-startup");
    h.place_pane("work", "api v2", "5");

    h.feed("UserPromptSubmit");

    assert_eq!(
        placement(&h.session()),
        (Some("work"), Some("api v2"), Some("5"))
    );
}

#[test]
fn a_failing_tmux_leaves_the_hook_silent_and_the_placement_alone() {
    let h = Harness::new();
    h.place_pane("work", "api", "1");
    h.feed("SessionStart-startup");
    h.fake.tmux.fail_pane_location("no server running");

    h.feed("UserPromptSubmit");

    let session = h.session();
    assert_eq!(session.state, State::Working);
    assert_eq!(placement(&session), (Some("work"), Some("api"), Some("1")));
    assert_eq!(h.hook_log(), "");
}

#[test]
fn an_unknown_pane_records_nothing() {
    let h = Harness::new();

    h.feed("SessionStart-startup");

    let session = h.session();
    assert_eq!(session.state, State::Idle);
    assert_eq!(placement(&session), (None, None, None));
}
