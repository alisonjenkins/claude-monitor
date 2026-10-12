//! Summaries (FR-017, FR-017a, FR-020): verbatim text from the session,
//! model-written text kept apart and labelled, loopback-only model calls.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};

use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::Session;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::summary;
use claude_monitor::testing::FakeClock;
use serde_json::{json, Value};
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const AGENT_SESSION: &str = "sess-verbatim";

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    transcript: PathBuf,
    clock: FakeClock,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        Self {
            state_dir: dir.path().join("state"),
            transcript: dir.path().join("transcript.jsonl"),
            _dir: dir,
            clock: FakeClock::new(T0),
        }
    }

    fn feed(&self, payload: Value) {
        let ctx = HookContext {
            agent: "claude",
            state_dir: &self.state_dir,
            tmux_pane: None,
            clock: &self.clock,
            git: &NoGit,
            agent_pid: None,
            env: None,
        };
        ingest(&ctx, &payload.to_string());
    }

    fn event(&self, name: &str, extra: Value) {
        let mut payload = json!({
            "session_id": AGENT_SESSION,
            "cwd": "/tmp/work",
            "hook_event_name": name,
        });
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        self.feed(payload);
    }

    fn session(&self) -> Session {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer())
            .unwrap()
            .get_session_by_agent_id("claude", AGENT_SESSION)
            .unwrap()
            .expect("session row exists")
    }
}

fn assistant_line(text: &str) -> String {
    json!({
        "type": "assistant",
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
    })
    .to_string()
}

fn write_transcript(path: &Path, lines: &[String]) {
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

mod verbatim {
    use super::*;

    const PROMPT: &str = "  Fix the flaky test in\n  src/foo.rs  \n\n  then open a PR  ";

    #[test]
    fn first_prompt_is_stored_untouched_and_never_replaced() {
        let h = Harness::new();
        h.event("UserPromptSubmit", json!({"prompt": PROMPT}));
        h.event("UserPromptSubmit", json!({"prompt": "a later request"}));
        assert_eq!(h.session().task_description.as_deref(), Some(PROMPT));
    }

    #[test]
    fn stop_message_is_stored_untouched() {
        let h = Harness::new();
        let message = "Done.\n\n  - indented  \n";
        h.event("Stop", json!({"last_assistant_message": message}));
        assert_eq!(h.session().left_off.as_deref(), Some(message));
    }

    #[test]
    fn stop_without_a_message_falls_back_to_the_transcript_tail() {
        let h = Harness::new();
        write_transcript(
            &h.transcript,
            &[
                assistant_line("an older answer"),
                json!({"type": "user", "message": {"role": "user", "content": "go on"}})
                    .to_string(),
                assistant_line("the  latest\nanswer "),
            ],
        );
        h.event(
            "Stop",
            json!({"transcript_path": h.transcript.to_str().unwrap()}),
        );
        assert_eq!(
            h.session().left_off.as_deref(),
            Some("the  latest\nanswer ")
        );
    }

    #[test]
    fn stop_message_beats_the_transcript() {
        let h = Harness::new();
        write_transcript(&h.transcript, &[assistant_line("from the transcript")]);
        h.event(
            "Stop",
            json!({
                "last_assistant_message": "from the payload",
                "transcript_path": h.transcript.to_str().unwrap(),
            }),
        );
        assert_eq!(h.session().left_off.as_deref(), Some("from the payload"));
    }

    #[test]
    fn missing_transcript_leaves_left_off_empty() {
        let h = Harness::new();
        h.event(
            "Stop",
            json!({"transcript_path": h.transcript.to_str().unwrap()}),
        );
        assert_eq!(h.session().left_off, None);
    }

    #[test]
    fn tail_skips_tool_only_and_sidechain_messages() {
        let tool_only = json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "tool_use", "name": "Bash"}]},
        })
        .to_string();
        let sidechain = json!({
            "type": "assistant",
            "isSidechain": true,
            "message": {"role": "assistant", "content": [{"type": "text", "text": "sub-agent"}]},
        })
        .to_string();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                assistant_line("real answer"),
                sidechain,
                tool_only,
                "not json at all".to_owned(),
            ],
        );
        assert_eq!(
            summary::transcript_tail_message(&path, summary::TRANSCRIPT_TAIL_BYTES).as_deref(),
            Some("real answer")
        );
    }

    #[test]
    fn tail_read_is_bounded_by_the_cap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.jsonl");
        let filler =
            json!({"type": "user", "message": {"role": "user", "content": "x".repeat(900)}})
                .to_string();
        let mut lines = vec![assistant_line("outside the window")];
        lines.extend(std::iter::repeat_n(filler, 400));
        write_transcript(&path, &lines);
        assert!(std::fs::metadata(&path).unwrap().len() > 300_000);

        assert_eq!(summary::transcript_tail_message(&path, 64 * 1024), None);
        assert_eq!(
            summary::transcript_tail_message(&path, 1024 * 1024).as_deref(),
            Some("outside the window")
        );
    }

    #[test]
    fn tail_window_that_cuts_a_line_in_half_ignores_the_partial_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cut.jsonl");
        let last = assistant_line("kept");
        let first = assistant_line(&"y".repeat(5000));
        write_transcript(&path, &[first, last.clone()]);
        let cap = u64::try_from(last.len()).unwrap() + 10;
        assert_eq!(
            summary::transcript_tail_message(&path, cap).as_deref(),
            Some("kept")
        );
    }
}
