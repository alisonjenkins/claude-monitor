//! Reader for Claude Code's live-session registry, `<config dir>/sessions`.
//!
//! Each running session keeps `<pid>.json` there. The sibling
//! `<pid>.<hash>.key` files hold secrets and are never opened, read or
//! statted: only names ending in `.json` are looked at.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use super::claude_discover::config_dir;
use crate::env::{Env, EnvError};
use crate::model::State;

/// Most bytes read from one registry file; the real files are well under 1 KiB.
pub const MAX_ENTRY_BYTES: usize = 16 * 1024;

const SESSIONS_DIR_NAME: &str = "sessions";
const ENTRY_EXTENSION: &str = ".json";
const CLI_ENTRYPOINT: &str = "cli";
const USER_NAME_SOURCE: &str = "user";
/// Claude Code rewrites these files in place, so a read can catch half a write;
/// one more read almost always sees the finished file.
const READ_ATTEMPTS: usize = 2;

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
    /// The display name the user set; automatic names are not kept.
    pub user_name: Option<String>,
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

/// The registry directory: `<config dir>/sessions`.
pub fn sessions_dir(env: &Env) -> Result<PathBuf, RegistryError> {
    config_dir(env)
        .map(|dir| dir.join(SESSIONS_DIR_NAME))
        .map_err(|_| RegistryError::NoConfigDir)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEntry {
    session_id: Option<String>,
    cwd: Option<String>,
    started_at: Option<u64>,
    proc_start: Option<String>,
    version: Option<String>,
    kind: Option<String>,
    entrypoint: Option<String>,
    tmux: Option<String>,
    status: Option<String>,
    waiting_for: Option<String>,
    name: Option<String>,
    name_source: Option<String>,
}

enum Load {
    Entry(Box<RawEntry>),
    /// Removed between listing and reading: the session ended, not a fault.
    Vanished,
    Degraded,
}

/// Reads every registry file. A file that cannot be used is counted in
/// `degraded` and never fails the scan; an absent directory is an empty result.
pub fn scan(env: &Env) -> Result<RegistryReport, RegistryError> {
    let dir = sessions_dir(env)?;
    let mut report = RegistryReport::default();
    if !env.fs.exists(&dir) {
        return Ok(report);
    }
    let files = env.fs.list_dir(&dir).map_err(|e| RegistryError::List {
        path: dir.display().to_string(),
        detail: e.to_string(),
    })?;
    for file in files {
        let Some(pid) = entry_pid(&file) else {
            continue;
        };
        match load(env, &file) {
            Load::Entry(raw) => match build(env, pid, *raw) {
                Some(entry) if entry.entrypoint == CLI_ENTRYPOINT => report.entries.push(entry),
                Some(_) => {}
                None => report.degraded = report.degraded.saturating_add(1),
            },
            Load::Vanished => {}
            Load::Degraded => report.degraded = report.degraded.saturating_add(1),
        }
    }
    report.entries.sort_by_key(|entry| entry.pid);
    Ok(report)
}

/// The pid of the live registry entry for `session_id`, if there is one.
pub fn live_pid_for_session(env: &Env, session_id: &str) -> Result<Option<u32>, RegistryError> {
    let report = scan(env)?;
    Ok(live_by_session_id(&report)
        .get(session_id)
        .map(|entry| entry.pid))
}

/// The pid of a `<pid>.json` file name; `None` for anything else, which
/// includes every `.key` file.
fn entry_pid(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_suffix(ENTRY_EXTENSION)?
        .parse()
        .ok()
}

fn load(env: &Env, path: &Path) -> Load {
    for _ in 0..READ_ATTEMPTS {
        let bytes = match env.fs.read_range(path, 0, MAX_ENTRY_BYTES) {
            Ok(bytes) => bytes,
            Err(EnvError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Load::Vanished;
            }
            Err(_) => return Load::Degraded,
        };
        if bytes.len() >= MAX_ENTRY_BYTES {
            return Load::Degraded;
        }
        if let Ok(raw) = serde_json::from_slice(&bytes) {
            return Load::Entry(Box::new(raw));
        }
    }
    Load::Degraded
}

fn build(env: &Env, pid: u32, raw: RawEntry) -> Option<RegistryEntry> {
    let live = raw
        .proc_start
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .is_some_and(|recorded| {
            matches!(env.processes.start_time(pid), Ok(Some(actual)) if actual == recorded)
        });
    let user_name = user_name(raw.name.as_deref(), raw.name_source.as_deref());
    Some(RegistryEntry {
        pid,
        session_id: raw.session_id?,
        cwd: raw.cwd.map(PathBuf::from),
        started_at_ms: raw.started_at,
        status: raw.status.as_deref().and_then(parse_status),
        waiting_for: raw.waiting_for,
        kind: match raw.kind.as_deref() {
            None | Some("interactive") => RegistryKind::Interactive,
            Some("bg") => RegistryKind::Background,
            Some(_) => RegistryKind::Other,
        },
        entrypoint: raw.entrypoint.unwrap_or_else(|| CLI_ENTRYPOINT.to_owned()),
        tmux: raw.tmux.as_deref().and_then(parse_tmux),
        live,
        version: raw.version,
        user_name,
    })
}

fn user_name(name: Option<&str>, source: Option<&str>) -> Option<String> {
    let name = name?.trim();
    (source == Some(USER_NAME_SOURCE) && !name.is_empty()).then(|| name.to_owned())
}

fn parse_status(status: &str) -> Option<RegistryStatus> {
    match status {
        "busy" => Some(RegistryStatus::Busy),
        "waiting" => Some(RegistryStatus::Waiting),
        "idle" => Some(RegistryStatus::Idle),
        "shell" => Some(RegistryStatus::Shell),
        _ => None,
    }
}

/// Session names may contain `:` and `.`, so the split is from the right.
fn parse_tmux(tmux: &str) -> Option<TmuxRef> {
    let (session, rest) = tmux.rsplit_once(":@")?;
    let (window, pane) = rest.split_once(".%")?;
    if session.is_empty() || window.is_empty() || pane.is_empty() {
        return None;
    }
    Some(TmuxRef {
        session: session.to_owned(),
        window_id: format!("@{window}"),
        pane_id: format!("%{pane}"),
    })
}

pub fn to_state(status: &RegistryStatus) -> State {
    match status {
        RegistryStatus::Busy | RegistryStatus::Shell => State::Working,
        RegistryStatus::Waiting => State::NeedsPermission,
        RegistryStatus::Idle => State::Idle,
    }
}

/// Live entries keyed by session id; if two live entries claim one id, the
/// higher pid wins.
pub fn live_by_session_id(report: &RegistryReport) -> HashMap<String, &RegistryEntry> {
    report
        .entries
        .iter()
        .filter(|entry| entry.live)
        .map(|entry| (entry.session_id.clone(), entry))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_numeric_json_names_are_entries() {
        assert_eq!(entry_pid(Path::new("/s/42.json")), Some(42));
        assert_eq!(entry_pid(Path::new("/s/42.abcdef.key")), None);
        assert_eq!(entry_pid(Path::new("/s/notes.json")), None);
        assert_eq!(entry_pid(Path::new("/s/42.abcdef.json")), None);
    }

    #[test]
    fn malformed_tmux_strings_are_none() {
        assert_eq!(parse_tmux("main"), None);
        assert_eq!(parse_tmux(":@1.%2"), None);
        assert_eq!(parse_tmux("main:@.%2"), None);
        assert_eq!(parse_tmux("main:@1.%"), None);
    }
}
