use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Working,
    NeedsPermission,
    Idle,
    Finished,
    Interrupted,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartSource {
    Startup,
    Resume,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NotificationKind {
    PermissionPrompt,
    ElicitationDialog,
    IdlePrompt,
    AgentIdle,
    AgentCompleted,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    SessionStart { source: StartSource },
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PermissionRequest,
    Notification(NotificationKind),
    Stop,
    SessionEnd { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    ProcessGone,
    BootChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventOutcome {
    pub lineage_continuation: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DoneError {
    #[error("only a finished session can be marked done (state is {state:?})")]
    NotFinished { state: State },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Session {
    pub id: String,
    pub agent_kind: String,
    pub agent_session_id: String,
    pub resumed_from: Option<String>,
    pub forked_from: Option<String>,
    pub repo_root: Option<String>,
    pub cwd: String,
    pub branch: Option<String>,
    pub worktree_exists: bool,
    pub state: State,
    pub end_reason: Option<String>,
    pub task_description: Option<String>,
    pub left_off: Option<String>,
    pub pid: Option<u32>,
    pub pid_start: Option<u64>,
    pub boot_id: Option<String>,
    pub tmux_pane: Option<String>,
    pub started_at: String,
    pub last_activity_at: String,
    pub state_since: String,
    pub project_id: Option<String>,
    pub parked: bool,
    pub dismissed: bool,
    pub done_at: Option<String>,
}

impl Session {
    /// A session seen only passively: `Unknown` until its first hook.
    pub fn new(id: &str, agent_kind: &str, agent_session_id: &str, cwd: &str, at: &str) -> Self {
        Self {
            id: id.to_owned(),
            agent_kind: agent_kind.to_owned(),
            agent_session_id: agent_session_id.to_owned(),
            resumed_from: None,
            forked_from: None,
            repo_root: None,
            cwd: cwd.to_owned(),
            branch: None,
            worktree_exists: true,
            state: State::Unknown,
            end_reason: None,
            task_description: None,
            left_off: None,
            pid: None,
            pid_start: None,
            boot_id: None,
            tmux_pane: None,
            started_at: at.to_owned(),
            last_activity_at: at.to_owned(),
            state_since: at.to_owned(),
            project_id: None,
            parked: false,
            dismissed: false,
            done_at: None,
        }
    }

    pub fn apply_event(&mut self, _event: &Event, _at: &str) -> EventOutcome {
        EventOutcome {
            lineage_continuation: false,
        }
    }

    pub fn reconcile(&mut self, _liveness: Liveness, _at: &str) {}

    pub fn running(&self, _liveness: Liveness) -> bool {
        false
    }

    pub fn mark_done(&mut self, _at: &str) -> Result<(), DoneError> {
        Ok(())
    }

    pub fn clear_done_on_respawn(&mut self) {}
}
