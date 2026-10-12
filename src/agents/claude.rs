use serde_json::Value;

use super::{
    claude_discover, standard_launch, AgentKind, DiscoverError, Launch, LaunchError, LaunchMode,
    LauncherConfig, ParseError, ParsedHook, Precondition, SessionStub,
};
use crate::env::Env;
use crate::model::{Event, NotificationKind, Session, StartSource};

const PROGRAM: &str = "claude";

const TOOL_SEARCH: &str = "ToolSearch";
const UNKNOWN_END_REASON: &str = "unknown";

/// Claude Code: hook payloads on stdin, `claude --resume <id>`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeKind;

impl AgentKind for ClaudeKind {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn discover(&self, env: &Env) -> Result<Vec<SessionStub>, DiscoverError> {
        claude_discover::scan(env).map(|report| report.stubs)
    }

    fn launch(
        &self,
        session: &Session,
        mode: LaunchMode,
        cfg: &LauncherConfig,
    ) -> Result<Launch, LaunchError> {
        standard_launch(self.id(), PROGRAM, session, mode, cfg)
    }

    /// The transcript sits in the project directory of the cwd the session
    /// started in, named by the encoding Claude Code uses for it.
    fn resume_preconditions(&self, session: &Session, env: &Env) -> Vec<Precondition> {
        let Ok(projects) = claude_discover::projects_dir(env) else {
            return Vec::new();
        };
        let transcript = projects
            .join(encode_cwd(&session.cwd))
            .join(format!("{}.jsonl", session.agent_session_id));
        vec![Precondition {
            description: "the conversation transcript exists".to_owned(),
            required_path: transcript,
        }]
    }

    /// Payload shapes are those confirmed on Claude Code 2.1.293
    /// (`tests/fixtures/claude/`). `ToolSearch` tool events and notification
    /// types other than `permission_prompt` and `idle_prompt` yield no event.
    /// A `SessionStart` source of `compact` maps to `Resume` (same session id);
    /// any other unknown source maps to `Startup`. A `SessionEnd` without a
    /// reason is stored as `unknown`.
    fn parse_hook(&self, raw: &Value) -> Result<Vec<ParsedHook>, ParseError> {
        let Some(session_id) = opt_str(raw, "session_id")? else {
            return Ok(Vec::new());
        };
        let Some(name) = opt_str(raw, "hook_event_name")? else {
            return Ok(Vec::new());
        };
        let hook = |event| {
            let mut hook = ParsedHook::new(event, session_id);
            hook.cwd = opt_str(raw, "cwd")?.map(str::to_owned);
            Ok::<_, ParseError>(hook)
        };

        let parsed = match name {
            "SessionStart" => {
                let source = match opt_str(raw, "source")? {
                    Some("resume" | "compact") => StartSource::Resume,
                    Some("clear") => StartSource::Clear,
                    _ => StartSource::Startup,
                };
                hook(Event::SessionStart { source })?
            }
            "UserPromptSubmit" => {
                let mut h = hook(Event::UserPromptSubmit)?;
                h.prompt = opt_str(raw, "prompt")?.map(str::to_owned);
                h
            }
            "PreToolUse" | "PostToolUse" => {
                let tool = opt_str(raw, "tool_name")?;
                if tool == Some(TOOL_SEARCH) {
                    return Ok(Vec::new());
                }
                let event = if name == "PreToolUse" {
                    Event::PreToolUse
                } else {
                    Event::PostToolUse
                };
                let mut h = hook(event)?;
                h.tool_name = tool.map(str::to_owned);
                h
            }
            "PermissionRequest" => {
                let mut h = hook(Event::PermissionRequest)?;
                h.tool_name = opt_str(raw, "tool_name")?.map(str::to_owned);
                h
            }
            "Notification" => {
                let kind = match opt_str(raw, "notification_type")? {
                    Some("permission_prompt") => NotificationKind::PermissionPrompt,
                    Some("idle_prompt") => NotificationKind::IdlePrompt,
                    _ => return Ok(Vec::new()),
                };
                let mut h = hook(Event::Notification(kind))?;
                h.message = opt_str(raw, "message")?.map(str::to_owned);
                h
            }
            "Stop" => {
                let mut h = hook(Event::Stop)?;
                h.last_assistant_message =
                    opt_str(raw, "last_assistant_message")?.map(str::to_owned);
                h
            }
            "SessionEnd" => {
                let reason = opt_str(raw, "reason")?.unwrap_or(UNKNOWN_END_REASON);
                hook(Event::SessionEnd {
                    reason: reason.to_owned(),
                })?
            }
            _ => return Ok(Vec::new()),
        };
        Ok(vec![parsed])
    }
}

/// Claude Code's project directory name for a cwd: every character that is
/// not an ASCII letter or digit becomes `-`.
fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
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
