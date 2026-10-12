//! Agent kinds: one module per supported coding agent.
//!
//! A kind turns that agent's hook payloads into [`ParsedHook`]s and says how
//! to discover, launch and resume its sessions. Nothing outside this module
//! tree knows an agent's payload shapes; the store consumes only
//! [`crate::model::Event`] plus the extra data in [`ParsedHook`].

use std::path::PathBuf;

use thiserror::Error;

use crate::env::Env;
use crate::model::{Event, Session};

pub mod claude;
pub mod claude_discover;

/// A hook payload that is present but malformed.
///
/// A payload that is merely incomplete (no session id, unknown event name) is
/// not an error: [`AgentKind::parse_hook`] returns an empty list for it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("hook payload field `{field}` must be {expected}")]
    WrongType {
        field: &'static str,
        expected: &'static str,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiscoverError {
    #[error("discovery is not supported by agent kind `{kind}`")]
    Unsupported { kind: &'static str },
    #[error("cannot locate the `{kind}` config directory: neither its override variable nor HOME is set")]
    NoConfigDir { kind: &'static str },
    #[error("cannot list `{kind}` sessions in {path}: {detail}")]
    List {
        kind: &'static str,
        path: String,
        detail: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LaunchError {
    #[error("launching is not supported by agent kind `{kind}`")]
    Unsupported { kind: &'static str },
}

/// One hook payload parsed into the closed [`Event`] set plus the extra data
/// the store needs. Carrying a `model::Event` keeps one definition of "what
/// happened"; the other fields are the payload data that does not change state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHook {
    pub event: Event,
    pub session_id: String,
    pub cwd: Option<String>,
    /// `UserPromptSubmit`: the full prompt text.
    pub prompt: Option<String>,
    /// `Stop`: the full last assistant message.
    pub last_assistant_message: Option<String>,
    /// Tool events and `PermissionRequest`.
    pub tool_name: Option<String>,
    /// `Notification`: the human-readable message.
    pub message: Option<String>,
}

impl ParsedHook {
    pub fn new(event: Event, session_id: &str) -> Self {
        Self {
            event,
            session_id: session_id.to_owned(),
            cwd: None,
            prompt: None,
            last_assistant_message: None,
            tool_name: None,
            message: None,
        }
    }
}

/// A session found on disk without hooks. Holds identity, place and times
/// only; the first prompt is the one piece of conversation it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStub {
    pub agent_session_id: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub started_at: String,
    pub last_activity_at: String,
    /// Truncated to [`claude_discover::MAX_PROMPT_CHARS`] characters.
    pub first_prompt: Option<String>,
}

/// What one scan found, and how many files it had to skip as unreadable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub stubs: Vec<SessionStub>,
    pub degraded: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Resume,
    Fresh,
}

/// A process to start: program, arguments, working directory and extra environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// A condition a respawn must check first (for example "transcript exists").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Precondition {
    pub description: String,
}

/// Launch command templates per kind; placeholders `{session_id}`, `{cwd}`, `{name}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherConfig {
    pub template: Option<String>,
}

/// Everything the rest of the program needs to know about one agent.
///
/// `parse_hook` is pure: no I/O, no clock. The other methods have defaults
/// that report "not supported" so a kind implements only what it can do.
pub trait AgentKind: Send + Sync {
    /// Stable id stored in `session.agent_kind` (`"claude"`, `"copilot"`).
    fn id(&self) -> &'static str;

    /// Turn one hook payload into zero or more events. An incomplete payload
    /// (no session id or event name, unknown event) yields an empty list; a
    /// field of the wrong type yields a [`ParseError`] naming it.
    fn parse_hook(&self, raw: &serde_json::Value) -> Result<Vec<ParsedHook>, ParseError>;

    /// Sessions visible on disk without hooks. Read-only; may return empty.
    fn discover(&self, _env: &Env) -> Result<Vec<SessionStub>, DiscoverError> {
        Ok(Vec::new())
    }

    /// How to relaunch a session.
    fn launch(
        &self,
        _session: &Session,
        _mode: LaunchMode,
        _cfg: &LauncherConfig,
    ) -> Result<Launch, LaunchError> {
        Err(LaunchError::Unsupported { kind: self.id() })
    }

    /// Preconditions a respawn must check. Empty if none.
    fn resume_preconditions(&self, _session: &Session) -> Vec<Precondition> {
        Vec::new()
    }
}

/// The agent kinds that exist, in registration order.
pub fn kinds() -> Vec<Box<dyn AgentKind>> {
    vec![Box::new(claude::ClaudeKind)]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn registered_ids_are_unique_and_non_empty() {
        let ids: Vec<&str> = kinds().iter().map(|k| k.id()).collect();
        let unique: HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len());
        assert!(ids.iter().all(|id| !id.is_empty()));
        assert!(ids.contains(&"claude"));
    }

    struct Bare;

    impl AgentKind for Bare {
        fn id(&self) -> &'static str {
            "bare"
        }

        fn parse_hook(&self, _raw: &serde_json::Value) -> Result<Vec<ParsedHook>, ParseError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn defaults_report_unsupported_or_empty() {
        let session = Session::new("s", "bare", "a", "/tmp", "2026-10-12T00:00:00Z");
        assert_eq!(
            Bare.discover(&crate::testing::FakeEnv::new().env),
            Ok(Vec::new())
        );
        assert_eq!(
            Bare.launch(&session, LaunchMode::Fresh, &LauncherConfig::default()),
            Err(LaunchError::Unsupported { kind: "bare" })
        );
        assert!(Bare.resume_preconditions(&session).is_empty());
    }
}
