#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! `hook --event <name>`: Copilot's native payloads carry no event name, so
//! the hooks file passes one, and it is used when the payload lacks its own.

use claude_monitor::hook::{ingest, inject_event, HookContext, NoGit};
use claude_monitor::model::State;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::{json, Value};
use tempfile::TempDir;

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
#[ignore = "T062c: inject_event is a stub"]
fn the_event_is_injected_when_the_payload_has_no_name() {
    let out = inject_event(r#"{"sessionId":"s1"}"#, Some("agentStop"));

    assert_eq!(
        parsed(&out),
        json!({"sessionId": "s1", "hook_event_name": "agentStop"})
    );
}

#[test]
#[ignore = "T062c: inject_event is a stub"]
fn a_null_name_counts_as_missing() {
    let out = inject_event(r#"{"hook_event_name":null}"#, Some("agentStop"));

    assert_eq!(parsed(&out)["hook_event_name"], "agentStop");
}

#[test]
fn a_name_in_the_payload_wins() {
    let input = r#"{"hook_event_name":"Notification"}"#;

    let out = inject_event(input, Some("agentStop"));

    assert_eq!(parsed(&out)["hook_event_name"], "Notification");
}

#[test]
fn no_event_leaves_the_input_untouched() {
    let input = r#"{"sessionId":"s1"}"#;

    assert_eq!(inject_event(input, None), input);
}

#[test]
fn input_that_is_not_a_json_object_is_returned_as_is() {
    assert_eq!(inject_event("not json", Some("agentStop")), "not json");
    assert_eq!(inject_event("[1]", Some("agentStop")), "[1]");
}

#[test]
#[ignore = "T062c: inject_event is a stub"]
fn an_explicit_event_beats_the_wrong_inference() {
    // `toolName` makes field inference say preToolUse, which ignores `prompt`.
    let payload = json!({
        "sessionId": "c1", "timestamp": 1_791_800_000_000_u64, "cwd": "/w",
        "toolName": "bash", "prompt": "fix the build",
    })
    .to_string();
    let dir = TempDir::new().unwrap();
    let state_dir = dir.path().join("state");
    let fake = FakeEnv::new();
    fake.fs.add_dir("/w");
    let ctx = HookContext {
        agent: "copilot",
        state_dir: &state_dir,
        tmux_pane: None,
        clock: fake.clock.as_ref(),
        git: &NoGit,
        agent_pid: None,
        env: Some(&fake.env),
    };
    ingest(&ctx, &inject_event(&payload, Some("userPromptSubmitted")));

    let store = Store::open_in_dir(&state_dir, &StoreOptions::viewer()).unwrap();
    let session = store
        .get_session_by_agent_id("copilot", "c1")
        .unwrap()
        .expect("session row exists");
    assert_eq!(session.state, State::Working);
    assert_eq!(session.task_description.as_deref(), Some("fix the build"));
}
