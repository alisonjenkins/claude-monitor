use serde::Deserialize;
use std::io::Read;
use std::path::Path;

use crate::env::Clock;
use crate::session::{self, Action};

/// Partial structure of the hook stdin JSON. Fields Claude Code may omit on
/// older payloads (e.g. `notification_type`) are optional, so a missing
/// field leaves state unchanged rather than failing to parse.
#[derive(Deserialize)]
struct HookInput {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    notification_type: Option<String>,
}

/// Git facts about a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    pub root: String,
    pub branch: Option<String>,
}

pub trait GitProbe: Send + Sync {
    /// `None` when `cwd` is not inside a git work tree.
    fn probe(&self, cwd: &str) -> Option<GitInfo>;
}

#[derive(Debug, Clone, Copy)]
pub struct NoGit;

impl GitProbe for NoGit {
    fn probe(&self, _cwd: &str) -> Option<GitInfo> {
        None
    }
}

/// Reads `.git/HEAD` directly, without spawning git.
#[derive(Debug, Clone, Copy)]
pub struct FsGit;

impl GitProbe for FsGit {
    fn probe(&self, _cwd: &str) -> Option<GitInfo> {
        None
    }
}

pub struct HookContext<'a> {
    pub agent: &'a str,
    pub state_dir: &'a Path,
    /// Directory of the legacy per-session JSON files; `None` skips that write.
    pub runtime_dir: Option<&'a Path>,
    pub tmux_pane: Option<&'a str>,
    pub clock: &'a dyn Clock,
    pub git: &'a dyn GitProbe,
}

/// Ingest one hook payload. Never fails and never prints: errors go to
/// `<state-dir>/hook.log`.
pub fn ingest(_ctx: &HookContext<'_>, _input: &str) {}

/// Entry point for `claude-monitor hook`, called by Claude Code's hook
/// mechanism. Always exits 0 and never writes to stdout — Claude Code may
/// interpret stdout for some events. Failures go to stderr only.
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-monitor hook: {e:#}");
    }
}

fn try_run() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let Ok(hook_input) = serde_json::from_str::<HookInput>(&input) else {
        return Ok(());
    };

    let Some(session_id) = hook_input.session_id else {
        return Ok(());
    };
    let Some(hook_event_name) = hook_input.hook_event_name else {
        return Ok(());
    };

    let action = session::map_event(&hook_event_name, hook_input.notification_type.as_deref());
    let dir = session::status_dir();

    match action {
        Action::Ignore => Ok(()),
        Action::Remove => session::remove_session(&dir, &session_id),
        Action::Set(state) => {
            let cwd = hook_input
                .cwd
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.display().to_string())
                })
                .unwrap_or_default();
            let tmux_pane = std::env::var("TMUX_PANE").ok();
            session::upsert_session(&dir, &session_id, state, cwd, tmux_pane)
        }
    }
}
