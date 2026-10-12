#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use claude_monitor::agents::claude::ClaudeKind;
use claude_monitor::agents::{AgentKind, ParseError, ParsedHook};
use claude_monitor::model::{next_state, Event, NotificationKind, StartSource, State};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/claude/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn parse(raw: &Value) -> Vec<ParsedHook> {
    ClaudeKind.parse_hook(raw).unwrap()
}

fn one(name: &str) -> ParsedHook {
    let mut hooks = parse(&fixture(name));
    assert_eq!(hooks.len(), 1, "{name}: expected exactly one event");
    hooks.remove(0)
}

fn state_after(hook: &ParsedHook) -> State {
    next_state(State::Idle, &hook.event).0
}

#[test]
fn session_start_sources() {
    for (name, source) in [
        ("SessionStart-startup", StartSource::Startup),
        ("SessionStart-resume", StartSource::Resume),
        ("SessionStart-clear", StartSource::Clear),
    ] {
        let hook = one(name);
        assert_eq!(hook.event, Event::SessionStart { source }, "{name}");
        assert_eq!(hook.cwd.as_deref(), Some("/tmp/SCRATCH/r2/work"), "{name}");
        assert!(!hook.session_id.is_empty(), "{name}");
    }
}

#[test]
fn user_prompt_submit_is_working_with_prompt_text() {
    let hook = one("UserPromptSubmit");
    assert_eq!(hook.event, Event::UserPromptSubmit);
    assert_eq!(hook.prompt.as_deref(), Some("Say hi"));
    assert_eq!(state_after(&hook), State::Working);
}

#[test]
fn tool_events_are_working() {
    for (name, event) in [
        ("PreToolUse-Read", Event::PreToolUse),
        ("PostToolUse-Read", Event::PostToolUse),
    ] {
        let hook = one(name);
        assert_eq!(hook.event, event, "{name}");
        assert_eq!(hook.tool_name.as_deref(), Some("Read"), "{name}");
        assert_eq!(state_after(&hook), State::Working, "{name}");
    }
}

#[test]
fn tool_search_is_not_work() {
    for name in ["PreToolUse-Read", "PostToolUse-Read"] {
        let mut raw = fixture(name);
        raw["tool_name"] = json!("ToolSearch");
        assert!(parse(&raw).is_empty(), "{name}");
    }
}

#[test]
fn permission_request_needs_permission_without_tool_use_id() {
    let raw = fixture("PermissionRequest");
    assert!(raw.get("tool_use_id").is_none());
    let hook = one("PermissionRequest");
    assert_eq!(hook.event, Event::PermissionRequest);
    assert_eq!(hook.tool_name.as_deref(), Some("Bash"));
    assert_eq!(state_after(&hook), State::NeedsPermission);
}

#[test]
fn notification_permission_prompt_needs_permission() {
    let hook = one("Notification-permission_prompt");
    assert_eq!(
        hook.event,
        Event::Notification(NotificationKind::PermissionPrompt)
    );
    assert_eq!(state_after(&hook), State::NeedsPermission);
    assert!(hook.message.is_some());
}

#[test]
fn notification_idle_prompt_is_idle() {
    let hook = one("Notification-idle_prompt");
    assert_eq!(
        hook.event,
        Event::Notification(NotificationKind::IdlePrompt)
    );
    assert_eq!(state_after(&hook), State::Idle);
    assert_eq!(
        hook.message.as_deref(),
        Some("Claude is waiting for your input")
    );
}

#[test]
fn other_notification_types_are_ignored() {
    let mut raw = fixture("Notification-idle_prompt");
    raw["notification_type"] = json!("auth_success");
    assert!(parse(&raw).is_empty());
}

#[test]
fn stop_is_idle_with_full_last_assistant_message() {
    let raw = fixture("Stop");
    let expected = raw["last_assistant_message"].as_str().unwrap().to_owned();
    let hook = one("Stop");
    assert_eq!(hook.event, Event::Stop);
    assert_eq!(hook.last_assistant_message, Some(expected));
    assert_eq!(state_after(&hook), State::Idle);
}

#[test]
fn session_end_keeps_raw_reason() {
    for (name, reason) in [
        ("SessionEnd-other", "other"),
        ("SessionEnd-clear", "clear"),
        ("SessionEnd-prompt_input_exit", "prompt_input_exit"),
    ] {
        let hook = one(name);
        assert_eq!(
            hook.event,
            Event::SessionEnd {
                reason: reason.to_owned()
            },
            "{name}"
        );
        assert_eq!(state_after(&hook), State::Finished, "{name}");
    }
}

#[test]
fn only_reason_clear_is_a_lineage_continuation() {
    for (name, continuation) in [
        ("SessionEnd-other", false),
        ("SessionEnd-clear", true),
        ("SessionEnd-prompt_input_exit", false),
    ] {
        let hook = one(name);
        assert_eq!(
            next_state(State::Idle, &hook.event).1,
            continuation,
            "{name}"
        );
    }
}

#[test]
fn unknown_extra_fields_are_ignored() {
    let mut raw = fixture("UserPromptSubmit");
    raw["brand_new_field"] = json!({"nested": [1, 2, 3]});
    let hooks = parse(&raw);
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, Event::UserPromptSubmit);
}

#[test]
fn missing_session_id_or_event_name_is_a_silent_no_op() {
    let mut no_id = fixture("Stop");
    no_id.as_object_mut().unwrap().remove("session_id");
    assert!(ClaudeKind.parse_hook(&no_id).unwrap().is_empty());

    let mut no_name = fixture("Stop");
    no_name.as_object_mut().unwrap().remove("hook_event_name");
    assert!(ClaudeKind.parse_hook(&no_name).unwrap().is_empty());

    assert!(ClaudeKind.parse_hook(&json!({})).unwrap().is_empty());
    assert!(ClaudeKind
        .parse_hook(&json!("not an object"))
        .unwrap()
        .is_empty());
}

#[test]
fn unknown_event_name_is_ignored() {
    let mut raw = fixture("Stop");
    raw["hook_event_name"] = json!("SomethingNew");
    assert!(parse(&raw).is_empty());
}

#[test]
fn malformed_session_id_names_the_field() {
    let mut raw = fixture("Stop");
    raw["session_id"] = json!(42);
    let err = ClaudeKind.parse_hook(&raw).unwrap_err();
    assert!(matches!(
        err,
        ParseError::WrongType {
            field: "session_id",
            ..
        }
    ));
    assert!(err.to_string().contains("session_id"));
}

#[test]
fn malformed_optional_field_names_the_field() {
    let mut raw = fixture("UserPromptSubmit");
    raw["prompt"] = json!(["not", "text"]);
    let err = ClaudeKind.parse_hook(&raw).unwrap_err();
    assert!(err.to_string().contains("prompt"));
}
