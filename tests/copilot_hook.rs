#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use claude_monitor::agents::copilot::CopilotKind;
use claude_monitor::agents::{AgentKind, ParseError, ParsedHook};
use claude_monitor::model::{next_state, Event, NotificationKind, StartSource, State};
use serde_json::{json, Value};

const SESSION_ID: &str = "995f9c8a-62f5-4bc1-9802-e52ccb0c2cea";
const CWD: &str = "/tmp/SCRATCH/r3/repo1";

/// `name` is relative to `tests/fixtures/copilot/`, without `.json`.
fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/copilot/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn parse(raw: &Value) -> Vec<ParsedHook> {
    CopilotKind.parse_hook(raw).unwrap()
}

fn one_of(raw: &Value) -> ParsedHook {
    let mut hooks = parse(raw);
    assert_eq!(hooks.len(), 1, "expected exactly one event for {raw}");
    hooks.remove(0)
}

fn one(name: &str) -> ParsedHook {
    one_of(&fixture(name))
}

fn state_after(hook: &ParsedHook) -> State {
    next_state(State::Idle, &hook.event).0
}

fn notification(kind: &str) -> Value {
    let mut raw = fixture("native/notification");
    raw["notification_type"] = json!(kind);
    raw
}

#[test]
#[ignore = "T038"]
fn kind_id_is_copilot() {
    assert_eq!(CopilotKind.id(), "copilot");
}

#[test]
#[ignore = "T038"]
fn session_start_new_is_startup() {
    let hook = one("native/sessionStart");
    assert_eq!(
        hook.event,
        Event::SessionStart {
            source: StartSource::Startup
        }
    );
    assert_eq!(hook.session_id, SESSION_ID);
    assert_eq!(hook.cwd.as_deref(), Some(CWD));
    assert_eq!(state_after(&hook), State::Idle);
}

#[test]
#[ignore = "T038"]
fn session_start_startup_source_is_startup() {
    let mut raw = fixture("native/sessionStart");
    raw["source"] = json!("startup");
    assert_eq!(
        one_of(&raw).event,
        Event::SessionStart {
            source: StartSource::Startup
        }
    );
}

#[test]
#[ignore = "T038"]
fn session_start_resume_keeps_the_session_id() {
    for dir in ["native/resume-same", "native/resume-other"] {
        let start = one(&format!("{dir}/sessionStart"));
        let prompt = one(&format!("{dir}/userPromptSubmitted"));
        assert_eq!(
            start.event,
            Event::SessionStart {
                source: StartSource::Resume
            },
            "{dir}"
        );
        assert_eq!(start.session_id, SESSION_ID, "{dir}");
        assert_eq!(start.session_id, prompt.session_id, "{dir}");
    }
}

#[test]
#[ignore = "T038"]
fn user_prompt_submitted_is_working_with_prompt_text() {
    let hook = one("native/userPromptSubmitted");
    assert_eq!(hook.event, Event::UserPromptSubmit);
    assert_eq!(
        hook.prompt.as_deref(),
        Some("Run the shell command: touch y . Then say done.")
    );
    assert_eq!(state_after(&hook), State::Working);
}

#[test]
#[ignore = "T038"]
fn tool_events_are_working() {
    for (name, event) in [
        ("native/preToolUse", Event::PreToolUse),
        ("native/postToolUse", Event::PostToolUse),
    ] {
        let hook = one(name);
        assert_eq!(hook.event, event, "{name}");
        assert_eq!(hook.tool_name.as_deref(), Some("bash"), "{name}");
        assert_eq!(state_after(&hook), State::Working, "{name}");
    }
}

#[test]
#[ignore = "T038"]
fn permission_request_needs_permission() {
    let hook = one("native/permissionRequest");
    assert_eq!(hook.event, Event::PermissionRequest);
    assert_eq!(hook.tool_name.as_deref(), Some("bash"));
    assert_eq!(state_after(&hook), State::NeedsPermission);
}

#[test]
#[ignore = "T038"]
fn notification_permission_prompt_needs_permission() {
    let hook = one("native/notification");
    assert_eq!(
        hook.event,
        Event::Notification(NotificationKind::PermissionPrompt)
    );
    assert_eq!(hook.message.as_deref(), Some("Run command: touch y ."));
    assert_eq!(state_after(&hook), State::NeedsPermission);
}

#[test]
#[ignore = "T038"]
fn notification_types_map_to_states() {
    for (kind, event, state) in [
        (
            "elicitation_dialog",
            NotificationKind::ElicitationDialog,
            State::NeedsPermission,
        ),
        ("agent_idle", NotificationKind::AgentIdle, State::Idle),
        (
            "agent_completed",
            NotificationKind::AgentCompleted,
            State::Idle,
        ),
    ] {
        let hook = one_of(&notification(kind));
        assert_eq!(hook.event, Event::Notification(event), "{kind}");
        assert_eq!(state_after(&hook), state, "{kind}");
    }
}

#[test]
#[ignore = "T038"]
fn other_notification_types_are_ignored() {
    for kind in ["shell_completed", "shell_detached_completed", "brand_new"] {
        assert!(parse(&notification(kind)).is_empty(), "{kind}");
    }
}

#[test]
#[ignore = "T038"]
fn agent_stop_is_idle_without_left_off_text() {
    let hook = one("native/agentStop");
    assert_eq!(hook.event, Event::Stop);
    assert_eq!(hook.last_assistant_message, None);
    assert_eq!(state_after(&hook), State::Idle);
}

#[test]
#[ignore = "T038"]
fn normal_session_end_is_finished_with_raw_reason() {
    for (name, reason) in [
        ("native/resume-same/sessionEnd", "complete"),
        ("native/sessionEnd", "user_exit"),
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
#[ignore = "T038"]
fn abnormal_session_end_is_interrupted_with_raw_reason() {
    for reason in ["error", "abort", "timeout"] {
        let mut raw = fixture("native/sessionEnd");
        raw["reason"] = json!(reason);
        let hook = one_of(&raw);
        assert_eq!(
            hook.event,
            Event::SessionEnd {
                reason: reason.to_owned()
            },
            "{reason}"
        );
        assert_eq!(state_after(&hook), State::Interrupted, "{reason}");
    }
}

#[test]
#[ignore = "T038"]
fn session_end_without_reason_is_stored_as_unknown() {
    let bare = json!({"sessionId": SESSION_ID, "hook_event_name": "SessionEnd"});
    assert_eq!(
        one_of(&bare).event,
        Event::SessionEnd {
            reason: "unknown".to_owned()
        }
    );
}

#[test]
#[ignore = "T038"]
fn claude_style_fixtures_parse_like_native_ones() {
    let start = one("claude-style/SessionStart");
    assert_eq!(
        start.event,
        Event::SessionStart {
            source: StartSource::Startup
        }
    );
    assert_eq!(start.session_id, "2fe49bea-0747-4424-87cb-13500abde88c");
    assert_eq!(start.cwd.as_deref(), Some("/tmp/SCRATCH/r3/repo2"));

    let prompt = one("claude-style/UserPromptSubmit");
    assert_eq!(prompt.event, Event::UserPromptSubmit);
    assert_eq!(prompt.prompt.as_deref(), Some("Say hi"));

    let stop = one("claude-style/Stop");
    assert_eq!(stop.event, Event::Stop);
    assert_eq!(stop.last_assistant_message, None);

    let end = one("claude-style/SessionEnd");
    assert_eq!(
        end.event,
        Event::SessionEnd {
            reason: "complete".to_owned()
        }
    );
    assert_eq!(state_after(&end), State::Finished);
}

#[test]
#[ignore = "T038"]
fn claude_style_event_names_are_mapped() {
    let base = |name: &str| {
        json!({
            "hook_event_name": name,
            "session_id": "s-1",
            "timestamp": "2026-10-12T10:52:19.038Z",
            "cwd": "/tmp/SCRATCH/r3/repo2",
        })
    };
    let with = |name: &str, extra: Value| {
        let mut raw = base(name);
        for (k, v) in extra.as_object().unwrap() {
            raw[k] = v.clone();
        }
        one_of(&raw)
    };

    assert_eq!(
        with("SessionStart", json!({"source": "resume"})).event,
        Event::SessionStart {
            source: StartSource::Resume
        }
    );
    assert_eq!(
        with("UserPromptSubmit", json!({"prompt": "hi"})).event,
        Event::UserPromptSubmit
    );
    let pre = with("PreToolUse", json!({"tool_name": "bash"}));
    assert_eq!(pre.event, Event::PreToolUse);
    assert_eq!(pre.tool_name.as_deref(), Some("bash"));
    assert_eq!(
        with("PostToolUse", json!({"tool_name": "bash"})).event,
        Event::PostToolUse
    );
    let permission = with("PermissionRequest", json!({"tool_name": "bash"}));
    assert_eq!(permission.event, Event::PermissionRequest);
    assert_eq!(permission.tool_name.as_deref(), Some("bash"));
    let note = with(
        "Notification",
        json!({"notification_type": "agent_idle", "message": "waiting"}),
    );
    assert_eq!(note.event, Event::Notification(NotificationKind::AgentIdle));
    assert_eq!(note.message.as_deref(), Some("waiting"));
    assert_eq!(with("Stop", json!({})).event, Event::Stop);
    assert_eq!(
        with("SessionEnd", json!({"reason": "abort"})).event,
        Event::SessionEnd {
            reason: "abort".to_owned()
        }
    );
}

#[test]
#[ignore = "T038"]
fn event_for_a_session_without_a_prior_session_start_is_fine() {
    for name in [
        "native/userPromptSubmitted",
        "native/preToolUse",
        "native/agentStop",
        "native/sessionEnd",
    ] {
        let hook = one(name);
        assert_eq!(hook.session_id, SESSION_ID, "{name}");
    }
}

#[test]
#[ignore = "T038"]
fn unknown_extra_fields_are_ignored() {
    let mut raw = fixture("native/userPromptSubmitted");
    raw["brandNewField"] = json!({"nested": [1, 2, 3]});
    assert_eq!(one_of(&raw).event, Event::UserPromptSubmit);
}

#[test]
#[ignore = "T038"]
fn missing_session_id_or_event_is_a_silent_no_op() {
    for (name, id_field) in [
        ("native/agentStop", "sessionId"),
        ("claude-style/Stop", "session_id"),
    ] {
        let mut raw = fixture(name);
        raw.as_object_mut().unwrap().remove(id_field);
        assert!(CopilotKind.parse_hook(&raw).unwrap().is_empty(), "{name}");
    }

    let mut no_name = fixture("claude-style/Stop");
    no_name.as_object_mut().unwrap().remove("hook_event_name");
    assert!(CopilotKind.parse_hook(&no_name).unwrap().is_empty());

    let nothing_to_infer = json!({"sessionId": SESSION_ID, "timestamp": 1, "cwd": CWD});
    assert!(CopilotKind
        .parse_hook(&nothing_to_infer)
        .unwrap()
        .is_empty());

    assert!(CopilotKind.parse_hook(&json!({})).unwrap().is_empty());
    assert!(CopilotKind
        .parse_hook(&json!("not an object"))
        .unwrap()
        .is_empty());
}

#[test]
#[ignore = "T038"]
fn unknown_event_name_is_ignored() {
    let mut raw = fixture("claude-style/Stop");
    raw["hook_event_name"] = json!("SomethingNew");
    assert!(parse(&raw).is_empty());
}

#[test]
#[ignore = "T038"]
fn wrong_types_name_the_field() {
    let cases: [(&str, &str, &str); 5] = [
        ("native/agentStop", "sessionId", "sessionId"),
        ("claude-style/Stop", "session_id", "session_id"),
        ("native/userPromptSubmitted", "prompt", "prompt"),
        ("native/preToolUse", "toolName", "toolName"),
        ("native/sessionEnd", "reason", "reason"),
    ];
    for (name, key, field) in cases {
        let mut raw = fixture(name);
        raw[key] = json!(["not", "text"]);
        let err = CopilotKind.parse_hook(&raw).unwrap_err();
        assert!(
            matches!(&err, ParseError::WrongType { field: f, .. } if *f == field),
            "{name}: {err:?}"
        );
        assert!(err.to_string().contains(field), "{name}");
    }
}
