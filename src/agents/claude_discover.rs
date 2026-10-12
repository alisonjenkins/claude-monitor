//! Passive discovery of Claude Code sessions from their transcripts.
//!
//! Transcripts live at `<config dir>/projects/<encoded cwd>/<session id>.jsonl`.
//! The directory name is a lossy encoding of the cwd (every non-alphanumeric
//! character becomes `-`), so the cwd is read from the transcript's own lines.
//! `sessions-index.json` is not maintained by current Claude Code and is not
//! used. Transcripts hold the whole conversation, so only a bounded head and
//! tail of each file is read, and only identity, place, times and the first
//! prompt are kept.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use super::claude::ClaudeKind;
use super::discover_store::store_report;
pub use super::discover_store::{DiscoverStoreError, DiscoverSummary};
use super::{AgentKind, DiscoverError, ScanReport, SessionStub};
use crate::env::{Env, FileMeta};
use crate::store::Store;

/// Longest first prompt kept, in characters.
pub const MAX_PROMPT_CHARS: usize = 500;
/// Longest working directory kept, in bytes; PATH_MAX on Linux.
pub const MAX_CWD_BYTES: usize = 4096;
/// Longest git branch name kept, in bytes; a longer one is dropped.
pub const MAX_BRANCH_BYTES: usize = 256;
/// Longest session id (file stem) accepted, in bytes; real ids are 36.
pub const MAX_SESSION_ID_BYTES: usize = 128;
/// Bytes read from the start of a transcript (cwd, first prompt) and again from
/// its end (latest timestamp), so a multi-megabyte transcript is never slurped.
pub const READ_WINDOW_BYTES: usize = 64 * 1024;

const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";
const HOME_VAR: &str = "HOME";
const CONFIG_DIR_NAME: &str = ".claude";
const PROJECTS_DIR_NAME: &str = "projects";
const TRANSCRIPT_EXTENSION: &str = "jsonl";

/// `$CLAUDE_CONFIG_DIR` if set, else `$HOME/.claude`.
pub fn config_dir(env: &Env) -> Result<PathBuf, DiscoverError> {
    if let Some(dir) = env.vars.var(CONFIG_DIR_VAR) {
        return Ok(PathBuf::from(dir));
    }
    env.vars
        .var(HOME_VAR)
        .map(|home| Path::new(&home).join(CONFIG_DIR_NAME))
        .ok_or(DiscoverError::NoConfigDir { kind: "claude" })
}

/// The directory whose subdirectories hold the transcripts.
pub fn projects_dir(env: &Env) -> Result<PathBuf, DiscoverError> {
    config_dir(env).map(|dir| dir.join(PROJECTS_DIR_NAME))
}

enum Outcome {
    Stub(SessionStub),
    Sidechain,
    Unusable,
}

/// Reads every transcript under the projects directory. A file that cannot be
/// used is counted in `degraded` and never fails the scan; an absent projects
/// directory is an empty result.
pub fn scan(env: &Env) -> Result<ScanReport, DiscoverError> {
    let projects = projects_dir(env)?;
    let mut report = ScanReport::default();
    if !env.fs.exists(&projects) {
        return Ok(report);
    }
    let list_error = |path: &Path, detail: String| DiscoverError::List {
        kind: "claude",
        path: path.display().to_string(),
        detail,
    };
    let project_dirs = env
        .fs
        .list_dir(&projects)
        .map_err(|e| list_error(&projects, e.to_string()))?;
    for project in project_dirs {
        if !env.fs.metadata(&project).is_ok_and(|meta| meta.is_dir) {
            report.degraded = report.degraded.saturating_add(1);
            continue;
        }
        let Ok(files) = env.fs.list_dir(&project) else {
            report.degraded = report.degraded.saturating_add(1);
            continue;
        };
        for file in files {
            let Ok(meta) = env.fs.metadata(&file) else {
                report.degraded = report.degraded.saturating_add(1);
                continue;
            };
            if meta.is_dir {
                continue;
            }
            let outcome = if file.extension().is_some_and(|e| e == TRANSCRIPT_EXTENSION) {
                read_transcript(env, &file, meta)
            } else {
                Outcome::Unusable
            };
            match outcome {
                Outcome::Stub(stub) => push_stub(&mut report.stubs, stub),
                Outcome::Sidechain => {}
                Outcome::Unusable => report.degraded = report.degraded.saturating_add(1),
            }
        }
    }
    Ok(report)
}

/// A session id found in two project directories keeps its latest activity.
fn push_stub(stubs: &mut Vec<SessionStub>, stub: SessionStub) {
    match stubs
        .iter_mut()
        .find(|known| known.agent_session_id == stub.agent_session_id)
    {
        Some(known) if known.last_activity_at < stub.last_activity_at => *known = stub,
        Some(_) => {}
        None => stubs.push(stub),
    }
}

pub(super) fn objects(bytes: &[u8]) -> impl Iterator<Item = Value> + '_ {
    bytes
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(Value::is_object)
}

pub(super) fn str_field<'a>(line: &'a Value, field: &str) -> Option<&'a str> {
    line.get(field).and_then(Value::as_str)
}

fn timestamp(line: &Value) -> Option<String> {
    let parsed = DateTime::parse_from_rfc3339(str_field(line, "timestamp")?).ok()?;
    Some(
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

fn branch(line: &Value) -> Option<String> {
    str_field(line, "gitBranch")
        .filter(|b| !b.is_empty() && b.len() <= MAX_BRANCH_BYTES)
        .map(str::to_owned)
}

/// The text of a user message: a plain string, or the first text block of an
/// array. Tool-result blocks are tool output and are never read.
fn prompt(line: &Value) -> Option<String> {
    if str_field(line, "type") != Some("user") || line.get("isMeta") == Some(&Value::Bool(true)) {
        return None;
    }
    let content = line.get("message")?.get("content")?;
    let text = match content {
        Value::String(text) => text.as_str(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| str_field(block, "type") == Some("text"))
            .find_map(|block| str_field(block, "text"))?,
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(MAX_PROMPT_CHARS).collect())
}

fn read_transcript(env: &Env, path: &Path, meta: FileMeta) -> Outcome {
    let Some(session_id) = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|id| !id.is_empty() && id.len() <= MAX_SESSION_ID_BYTES)
    else {
        return Outcome::Unusable;
    };
    let Ok(head) = env.fs.read_range(path, 0, READ_WINDOW_BYTES) else {
        return Outcome::Unusable;
    };
    let window = u64::try_from(READ_WINDOW_BYTES).unwrap_or(u64::MAX);
    let tail = if meta.len > window {
        let Ok(bytes) = env
            .fs
            .read_range(path, meta.len.saturating_sub(window), READ_WINDOW_BYTES)
        else {
            return Outcome::Unusable;
        };
        // The window starts mid-line, so its first partial line is dropped.
        bytes
            .splitn(2, |b| *b == b'\n')
            .nth(1)
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    } else {
        head.clone()
    };

    let mut cwd = None;
    let mut started_at = None;
    let mut head_branch = None;
    let mut first_prompt = None;
    for line in objects(&head) {
        if line.get("isSidechain") == Some(&Value::Bool(true)) {
            return Outcome::Sidechain;
        }
        if cwd.is_none() {
            cwd = str_field(&line, "cwd").map(str::to_owned);
        }
        if started_at.is_none() {
            started_at = timestamp(&line);
        }
        if head_branch.is_none() {
            head_branch = branch(&line);
        }
        if first_prompt.is_none() {
            first_prompt = prompt(&line);
        }
    }
    let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty() && cwd.len() <= MAX_CWD_BYTES) else {
        return Outcome::Unusable;
    };

    let mut latest_at = None;
    let mut latest_branch = None;
    for line in objects(&tail).collect::<Vec<_>>().iter().rev() {
        if latest_at.is_none() {
            latest_at = timestamp(line);
        }
        if latest_branch.is_none() && line.get("gitBranch").is_some() {
            latest_branch = Some(branch(line));
        }
        if latest_at.is_some() && latest_branch.is_some() {
            break;
        }
    }

    let fallback = meta.modified.map(DateTime::<Utc>::from).map_or_else(
        || env.clock.now(),
        |at| at.to_rfc3339_opts(SecondsFormat::Secs, true),
    );
    let started_at = started_at.unwrap_or_else(|| fallback.clone());
    let last_activity_at = latest_at
        .filter(|at| *at >= started_at)
        .unwrap_or_else(|| started_at.clone());
    Outcome::Stub(SessionStub {
        agent_session_id: session_id.to_owned(),
        cwd,
        repo_root: None,
        branch: latest_branch.unwrap_or(head_branch),
        name: None,
        started_at,
        last_activity_at,
        first_prompt,
    })
}

/// Scans, then records each stub: a new session as `unknown` plus a
/// `session.discovered` event; a known session only gets missing fields filled.
/// A session a hook reported keeps its hook-derived state and times.
pub fn discover_and_store(env: &Env, store: &Store) -> Result<DiscoverSummary, DiscoverStoreError> {
    let report = scan(env)?;
    store_report(env, store, ClaudeKind.id(), &report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::testing::FakeEnv;

    #[test]
    fn scans_through_the_fs_seam() {
        let fake = FakeEnv::new();
        fake.vars.set("HOME", "/home/u");
        let line = r#"{"type":"user","sessionId":"s1","timestamp":"2026-10-12T09:00:00Z","cwd":"/w/a.b","message":{"content":"hi"}}"#;
        fake.fs.add_file(
            "/home/u/.claude/projects/-w-a-b/s1.jsonl",
            &format!("{line}\n"),
        );

        let report = scan(&fake.env).unwrap();

        assert_eq!(report.degraded, 0);
        assert_eq!(report.stubs.len(), 1);
        let stub = report.stubs.first().unwrap();
        assert_eq!(stub.cwd, "/w/a.b");
        assert_eq!(stub.first_prompt.as_deref(), Some("hi"));
    }

    #[test]
    fn config_dir_override_wins_over_home() {
        let fake = FakeEnv::new();
        fake.vars.set("HOME", "/home/u");
        fake.vars.set("CLAUDE_CONFIG_DIR", "/cfg");
        assert_eq!(projects_dir(&fake.env).unwrap(), Path::new("/cfg/projects"));
    }
}
