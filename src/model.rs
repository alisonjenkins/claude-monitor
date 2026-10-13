use std::fmt;

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
    /// Where the pane was last seen: kept after the pane is gone so a
    /// respawn can put the session back in the same tmux session and window.
    pub tmux_session: Option<String>,
    pub tmux_window_name: Option<String>,
    pub tmux_window_index: Option<String>,
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
            tmux_session: None,
            tmux_window_name: None,
            tmux_window_index: None,
            started_at: at.to_owned(),
            last_activity_at: at.to_owned(),
            state_since: at.to_owned(),
            project_id: None,
            parked: false,
            dismissed: false,
            done_at: None,
        }
    }

    /// Records where tmux says the pane is; `true` when that changed anything.
    pub fn record_tmux_location(&mut self, location: &crate::tmux::PaneLocation) -> bool {
        let session = Some(location.session_name.as_str());
        let window_name = Some(location.window_name.as_str());
        let window_index = Some(location.window_index.as_str());
        if self.tmux_session.as_deref() == session
            && self.tmux_window_name.as_deref() == window_name
            && self.tmux_window_index.as_deref() == window_index
        {
            return false;
        }
        self.tmux_session = Some(location.session_name.clone());
        self.tmux_window_name = Some(location.window_name.clone());
        self.tmux_window_index = Some(location.window_index.clone());
        true
    }

    pub fn apply_event(&mut self, event: &Event, at: &str) -> EventOutcome {
        let is_start = matches!(event, Event::SessionStart { .. });
        if self.state == State::Finished && !is_start {
            return EventOutcome {
                lineage_continuation: false,
            };
        }
        let (next, lineage_continuation) = next_state(self.state, event);
        self.set_state(next, at);
        at.clone_into(&mut self.last_activity_at);
        match event {
            Event::SessionStart { .. } => self.end_reason = None,
            Event::SessionEnd { reason } => self.end_reason = Some(reason.clone()),
            _ => {}
        }
        EventOutcome {
            lineage_continuation,
        }
    }

    pub fn reconcile(&mut self, liveness: Liveness, at: &str) {
        if self.state != State::Finished && liveness != Liveness::Alive {
            self.set_state(State::Interrupted, at);
        }
    }

    pub fn running(&self, liveness: Liveness) -> bool {
        self.state.is_active() && liveness == Liveness::Alive
    }

    pub fn mark_done(&mut self, at: &str) -> Result<(), DoneError> {
        if self.state != State::Finished {
            return Err(DoneError::NotFinished { state: self.state });
        }
        self.done_at = Some(at.to_owned());
        Ok(())
    }

    pub fn clear_done_on_respawn(&mut self) {
        self.done_at = None;
    }

    fn set_state(&mut self, next: State, at: &str) {
        if self.state != next {
            self.state = next;
            at.clone_into(&mut self.state_since);
        }
    }
}

const CLEAR_REASON: &str = "clear";
const ABNORMAL_END_REASONS: [&str; 3] = ["error", "abort", "timeout"];

/// Returns the next state and whether a `/clear` continuation Session must be created.
/// A finished session only leaves `Finished` through `SessionStart`.
pub fn next_state(current: State, event: &Event) -> (State, bool) {
    match event {
        Event::SessionStart { .. } => (State::Idle, false),
        _ if current == State::Finished => (State::Finished, false),
        Event::UserPromptSubmit | Event::PreToolUse | Event::PostToolUse => (State::Working, false),
        Event::PermissionRequest
        | Event::Notification(
            NotificationKind::PermissionPrompt | NotificationKind::ElicitationDialog,
        ) => (State::NeedsPermission, false),
        Event::Stop
        | Event::Notification(
            NotificationKind::IdlePrompt
            | NotificationKind::AgentIdle
            | NotificationKind::AgentCompleted,
        ) => (State::Idle, false),
        Event::Notification(_) => (current, false),
        Event::SessionEnd { reason } if reason == CLEAR_REASON => (State::Finished, true),
        Event::SessionEnd { reason } if ABNORMAL_END_REASONS.contains(&reason.as_str()) => {
            (State::Interrupted, false)
        }
        Event::SessionEnd { .. } => (State::Finished, false),
    }
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Working => "working",
            State::NeedsPermission => "needs_permission",
            State::Idle => "idle",
            State::Finished => "finished",
            State::Interrupted => "interrupted",
            State::Unknown => "unknown",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, State::Working | State::NeedsPermission | State::Idle)
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    Parked,
    Done,
    Archived,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Parked => "parked",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "active" => Self::Active,
            "parked" => Self::Parked,
            "done" => Self::Done,
            "archived" => Self::Archived,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Project {
    pub id: String,
    pub name: String,
    pub status: ProjectStatus,
    pub note: Option<String>,
    pub progress_binding_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Todo,
    Doing,
    Done,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Step {
    pub binding_id: String,
    pub ordinal: u32,
    pub text: String,
    pub status: StepStatus,
    pub group: Option<String>,
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Session,
    Project,
}

impl OwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "session" => Some(Self::Session),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "author_kind", rename_all = "snake_case")]
pub enum Author {
    User { name: String },
    Agent { kind: String, model: Option<String> },
    System { component: String },
}

/// Who made a change, and when; the shape of an `events` row's author columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub at: String,
    pub author: Author,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Note {
    pub id: String,
    pub owner_kind: OwnerKind,
    pub owner_id: String,
    pub body: String,
    pub created_at: String,
    pub author: Author,
}
