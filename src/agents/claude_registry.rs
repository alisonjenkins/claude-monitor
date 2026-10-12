//! Reader for Claude Code's live-session registry, `<config dir>/sessions`.

use std::collections::HashMap;
use std::path::PathBuf;

use thiserror::Error;

use crate::env::Env;
use crate::model::State;

/// Most bytes read from one registry file; the real files are well under 1 KiB.
pub const MAX_ENTRY_BYTES: usize = 16 * 1024;

/// What Claude Code reports a session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryStatus {
    Busy,
    Waiting,
    Idle,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryKind {
    Interactive,
    Background,
    Other,
}

/// `<session>:@<window>.%<pane>`, with the tmux id sigils kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxRef {
    pub session: String,
    pub window_id: String,
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub pid: u32,
    pub session_id: String,
    pub cwd: Option<PathBuf>,
    pub started_at_ms: Option<u64>,
    /// `None` before the first status write.
    pub status: Option<RegistryStatus>,
    pub waiting_for: Option<String>,
    pub kind: RegistryKind,
    pub entrypoint: String,
    pub tmux: Option<TmuxRef>,
    /// The pid is running and its start time equals the recorded `procStart`.
    pub live: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryReport {
    /// Sorted by pid.
    pub entries: Vec<RegistryEntry>,
    /// Files skipped because they could not be read or parsed.
    pub degraded: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error(
        "cannot locate the Claude config directory: neither CLAUDE_CONFIG_DIR nor HOME is set"
    )]
    NoConfigDir,
    #[error("cannot list Claude session registry {path}: {detail}")]
    List { path: String, detail: String },
}

pub fn scan(_env: &Env) -> Result<RegistryReport, RegistryError> {
    Ok(RegistryReport::default())
}

pub fn to_state(_status: &RegistryStatus) -> State {
    State::Unknown
}

/// Live entries keyed by session id.
pub fn live_by_session_id(_report: &RegistryReport) -> HashMap<String, &RegistryEntry> {
    HashMap::new()
}
