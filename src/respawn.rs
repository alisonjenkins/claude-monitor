//! Respawn planning: turn a stored session into "start this command in this
//! directory", or into a refusal that names the problem and the ways out.
//!
//! The planner is pure over the [`Env`] providers: it reads the filesystem,
//! process table and tmux availability and returns a [`Plan`] or [`Refusal`].
//! It never creates a directory, runs git or starts a process; the executor
//! does that, and only for what the plan lists.

use std::path::PathBuf;

use crate::agents::{AgentKind, Launch, LaunchMode, LauncherConfig};
use crate::env::Env;
use crate::model::Session;

/// Why a respawn was refused. Codes are the strings stored in
/// `RespawnRequest.outcome` as `refused:<code>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    AlreadyRunning,
    MissingDirectory,
    MissingTranscript,
    BranchGone,
    NoLauncher,
    NoTmux,
    /// The flags contradict each other (`--in` without `--fresh`).
    InvalidOptions,
}

impl RefusalCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyRunning => "already_running",
            Self::MissingDirectory => "missing_directory",
            Self::MissingTranscript => "missing_transcript",
            Self::BranchGone => "branch_gone",
            Self::NoLauncher => "no_launcher",
            Self::NoTmux => "no_tmux",
            Self::InvalidOptions => "invalid_options",
        }
    }

    /// The command line exit code: 2 for a refused precondition, 1 for a
    /// usage error.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::InvalidOptions => 1,
            _ => 2,
        }
    }
}

/// Something the user can do instead, as a description and a command to run.
/// Never taken automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alternative {
    pub description: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    pub message: String,
    pub alternatives: Vec<Alternative>,
}

/// What a respawn will do, in order: run each `setup` command (for example
/// `git worktree add`), then start `launch`. `notices` are shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub session_id: String,
    pub mode: LaunchMode,
    pub launch: Launch,
    pub setup: Vec<Launch>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Run `git worktree add` for a missing worktree whose branch still exists.
    pub recreate_worktree: bool,
    /// Start without resuming the conversation.
    pub fresh: bool,
    /// With `fresh`: start in this directory instead of the recorded one.
    pub in_dir: Option<PathBuf>,
    /// The launcher template configured for the session's agent kind.
    pub launcher: LauncherConfig,
}

/// Plan a respawn of `session` using the registered agent kind it names.
pub fn plan_respawn(session: &Session, env: &Env, options: &Options) -> Result<Plan, Refusal> {
    let kinds = crate::agents::kinds();
    let kind = kinds.iter().find(|k| k.id() == session.agent_kind);
    match kind {
        Some(kind) => plan_respawn_with(kind.as_ref(), session, env, options),
        None => Err(Refusal {
            code: RefusalCode::NoLauncher,
            message: format!("no launcher for agent kind `{}`", session.agent_kind),
            alternatives: Vec::new(),
        }),
    }
}

/// [`plan_respawn`] for an explicit agent kind.
pub fn plan_respawn_with(
    _kind: &dyn AgentKind,
    _session: &Session,
    _env: &Env,
    _options: &Options,
) -> Result<Plan, Refusal> {
    Err(Refusal {
        code: RefusalCode::InvalidOptions,
        message: "respawn planning is not implemented".to_owned(),
        alternatives: Vec::new(),
    })
}
