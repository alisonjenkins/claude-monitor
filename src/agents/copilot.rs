use serde_json::Value;

use super::{
    copilot_discover, standard_launch, AgentKind, DiscoverError, Launch, LaunchError, LaunchMode,
    LauncherConfig, ParseError, ParsedHook, Precondition, SessionStub,
};
use crate::env::Env;
use crate::model::{Event, NotificationKind, Session, StartSource};

const PROGRAM: &str = "copilot";

const UNKNOWN_END_REASON: &str = "unknown";

const SESSION_ID_FIELDS: [&str; 2] = ["sessionId", "session_id"];
const TOOL_NAME_FIELDS: [&str; 2] = ["toolName", "tool_name"];
const EVENT_NAME_FIELDS: [&str; 3] = ["hook_event_name", "hookEventName", "hookName"];

/// The hook events Copilot reports, whichever spelling delivered them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Name {
    SessionStart,
    UserPrompt,
    PreTool,
    PostTool,
    Permission,
    Notification,
    Stop,
    SessionEnd,
}

/// GitHub Copilot CLI: hook payloads on stdin.
#[derive(Debug, Clone, Copy, Default)]
pub struct CopilotKind;

impl AgentKind for CopilotKind {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn discover(&self, env: &Env) -> Result<Vec<SessionStub>, DiscoverError> {
        copilot_discover::scan(env).map(|report| report.stubs)
    }

    /// Copilot resumes from any directory; the recorded one is still used.
    fn launch(
        &self,
        session: &Session,
        mode: LaunchMode,
        cfg: &LauncherConfig,
    ) -> Result<Launch, LaunchError> {
        standard_launch(self.id(), PROGRAM, session, mode, cfg, None, None)
    }

    fn resume_preconditions(&self, session: &Session, env: &Env) -> Vec<Precondition> {
        let Ok(states) = copilot_discover::session_state_dir(env) else {
            return Vec::new();
        };
        vec![Precondition {
            description: "the session state directory exists".to_owned(),
            required_path: states.join(&session.agent_session_id),
            also_accepted: Vec::new(),
        }]
    }

    /// Payload shapes are those captured on Copilot CLI 1.0.93
    /// (`tests/fixtures/copilot/`). Two spellings are accepted: Copilot's own
    /// camelCase (`sessionId`, `toolName`, ...) and the snake_case that
    /// Copilot sends to hooks read from `.claude/settings.json`.
    ///
    /// Native payloads carry no event name except `permissionRequest`
    /// (`hookName`) and `notification` (`hook_event_name`), so when no name
    /// is present the event is inferred from which fields exist. An event
    /// name that is present but unknown yields nothing. Notification types
    /// other than `permission_prompt`, `elicitation_dialog`, `agent_idle` and
    /// `agent_completed` yield no event. A `sessionStart` source of `resume`
    /// maps to `Resume`; anything else is `Startup`. A `sessionEnd` without a
    /// reason is stored as `unknown`; the model decides which reasons are
    /// abnormal. `agentStop` carries no assistant text, so `left_off` is
    /// never set from it.
    fn parse_hook(&self, raw: &Value) -> Result<Vec<ParsedHook>, ParseError> {
        let Some(session_id) = first_str(raw, &SESSION_ID_FIELDS)? else {
            return Ok(Vec::new());
        };
        let Some(name) = event_name(raw)? else {
            return Ok(Vec::new());
        };
        let hook = |event| {
            let mut hook = ParsedHook::new(event, session_id);
            hook.cwd = opt_str(raw, "cwd")?.map(str::to_owned);
            Ok::<_, ParseError>(hook)
        };

        let parsed = match name {
            Name::SessionStart => {
                let source = match opt_str(raw, "source")? {
                    Some("resume") => StartSource::Resume,
                    _ => StartSource::Startup,
                };
                hook(Event::SessionStart { source })?
            }
            Name::UserPrompt => {
                let mut h = hook(Event::UserPromptSubmit)?;
                h.prompt = opt_str(raw, "prompt")?.map(str::to_owned);
                h
            }
            Name::PreTool | Name::PostTool => {
                let event = if name == Name::PreTool {
                    Event::PreToolUse
                } else {
                    Event::PostToolUse
                };
                let mut h = hook(event)?;
                h.tool_name = first_str(raw, &TOOL_NAME_FIELDS)?.map(str::to_owned);
                h
            }
            Name::Permission => {
                let mut h = hook(Event::PermissionRequest)?;
                h.tool_name = first_str(raw, &TOOL_NAME_FIELDS)?.map(str::to_owned);
                h
            }
            Name::Notification => {
                let kind = match opt_str(raw, "notification_type")? {
                    Some("permission_prompt") => NotificationKind::PermissionPrompt,
                    Some("elicitation_dialog") => NotificationKind::ElicitationDialog,
                    Some("agent_idle") => NotificationKind::AgentIdle,
                    Some("agent_completed") => NotificationKind::AgentCompleted,
                    _ => return Ok(Vec::new()),
                };
                let mut h = hook(Event::Notification(kind))?;
                h.message = opt_str(raw, "message")?.map(str::to_owned);
                h
            }
            Name::Stop => hook(Event::Stop)?,
            Name::SessionEnd => {
                let reason = opt_str(raw, "reason")?.unwrap_or(UNKNOWN_END_REASON);
                hook(Event::SessionEnd {
                    reason: reason.to_owned(),
                })?
            }
        };
        Ok(vec![parsed])
    }
}

/// The event an explicit name field says, else the one the camelCase fields
/// imply. `None` when the name is unknown or nothing identifies the event.
fn event_name(raw: &Value) -> Result<Option<Name>, ParseError> {
    if let Some(name) = first_str(raw, &EVENT_NAME_FIELDS)? {
        return Ok(named(name));
    }
    let has = |field: &str| raw.get(field).is_some_and(|v| !v.is_null());
    let inferred = if has("source") || has("initialPrompt") {
        Name::SessionStart
    } else if has("reason") {
        Name::SessionEnd
    } else if has("stopReason") || has("transcriptPath") {
        Name::Stop
    } else if has("toolResult") {
        Name::PostTool
    } else if has("toolName") || has("toolArgs") {
        Name::PreTool
    } else if has("prompt") {
        Name::UserPrompt
    } else {
        return Ok(None);
    };
    Ok(Some(inferred))
}

fn named(name: &str) -> Option<Name> {
    Some(match name.to_ascii_lowercase().as_str() {
        "sessionstart" => Name::SessionStart,
        "userpromptsubmitted" | "userpromptsubmit" => Name::UserPrompt,
        "pretooluse" => Name::PreTool,
        "posttooluse" | "posttoolusefailure" => Name::PostTool,
        "permissionrequest" => Name::Permission,
        "notification" => Name::Notification,
        "agentstop" | "stop" => Name::Stop,
        "sessionend" => Name::SessionEnd,
        _ => return None,
    })
}

/// The first of `fields` that is present and not null, as a string; a field of
/// any other type is an error.
fn first_str<'a>(raw: &'a Value, fields: &[&'static str]) -> Result<Option<&'a str>, ParseError> {
    for &field in fields {
        if let Some(value) = opt_str(raw, field)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// A string field that may be absent or null; any other type is an error.
fn opt_str<'a>(raw: &'a Value, field: &'static str) -> Result<Option<&'a str>, ParseError> {
    match raw.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(ParseError::WrongType {
            field,
            expected: "a string",
        }),
    }
}
