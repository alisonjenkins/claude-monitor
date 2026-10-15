//! Passive discovery of Copilot CLI sessions from their session state.
//!
//! Each session lives in `$COPILOT_HOME/session-state/<id>/`, with a flat
//! `workspace.yaml` (identity, place, times, name) and an `events.jsonl` of the
//! whole conversation. Only a bounded head of each file is read, and only
//! identity, place, times, name and the first `user.message` are kept; an
//! empty directory left by a failed run has no `workspace.yaml` and is counted
//! as degraded.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};

use super::claude_discover::{objects, str_field, MAX_BRANCH_BYTES, MAX_CWD_BYTES};
pub use super::claude_discover::{MAX_PROMPT_CHARS, MAX_SESSION_ID_BYTES, READ_WINDOW_BYTES};
use super::discover_store::store_report;
pub use super::discover_store::{DiscoverStoreError, DiscoverSummary};
use super::{DiscoverError, ScanReport, SessionStub};
use crate::env::Env;
use crate::hook::NoGit;
use crate::store::Store;

const KIND: &str = "copilot";
const COPILOT_HOME_VAR: &str = "COPILOT_HOME";
const HOME_VAR: &str = "HOME";
const HOME_DIR_NAME: &str = ".copilot";
const SESSION_STATE_DIR_NAME: &str = "session-state";
const WORKSPACE_FILE: &str = "workspace.yaml";
const EVENTS_FILE: &str = "events.jsonl";
const USER_MESSAGE_EVENT: &str = "user.message";

/// `$COPILOT_HOME` if set, else `$HOME/.copilot`, then `session-state`.
pub fn session_state_dir(env: &Env) -> Result<PathBuf, DiscoverError> {
    let home = match env.vars.var(COPILOT_HOME_VAR) {
        Some(dir) => PathBuf::from(dir),
        None => env
            .vars
            .var(HOME_VAR)
            .map(|home| Path::new(&home).join(HOME_DIR_NAME))
            .ok_or(DiscoverError::NoConfigDir { kind: KIND })?,
    };
    Ok(home.join(SESSION_STATE_DIR_NAME))
}

/// Reads every session directory under the session-state directory. An entry
/// that cannot be used is counted in `degraded` and never fails the scan; an
/// absent session-state directory is an empty result.
pub fn scan(env: &Env) -> Result<ScanReport, DiscoverError> {
    let root = session_state_dir(env)?;
    let mut report = ScanReport::default();
    if !env.fs.exists(&root) {
        return Ok(report);
    }
    let entries = env.fs.list_dir(&root).map_err(|e| DiscoverError::List {
        kind: KIND,
        path: root.display().to_string(),
        detail: e.to_string(),
    })?;
    for entry in entries {
        let is_dir = env.fs.metadata(&entry).is_ok_and(|meta| meta.is_dir);
        match is_dir.then(|| read_session(env, &entry)).flatten() {
            Some(stub) => report.stubs.push(stub),
            None => report.degraded = report.degraded.saturating_add(1),
        }
    }
    Ok(report)
}

/// The values of the `workspace.yaml` keys discovery uses.
#[derive(Debug, Default)]
struct Workspace {
    id: Option<String>,
    cwd: Option<String>,
    git_root: Option<String>,
    branch: Option<String>,
    name: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// Flat `key: value` lines only. Tolerates CRLF, quoted and empty values and
/// ignores indented, comment and unknown lines.
fn parse_workspace(text: &str) -> Workspace {
    let mut workspace = Workspace::default();
    for line in text.lines() {
        if line.starts_with(|c: char| c.is_whitespace() || c == '#' || c == '-') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        let slot = match key.trim() {
            "id" => &mut workspace.id,
            "cwd" => &mut workspace.cwd,
            "git_root" => &mut workspace.git_root,
            "branch" => &mut workspace.branch,
            "name" => &mut workspace.name,
            "created_at" => &mut workspace.created_at,
            "updated_at" => &mut workspace.updated_at,
            _ => continue,
        };
        slot.get_or_insert_with(|| value.to_owned());
    }
    workspace
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// An id becomes a command argument (`copilot --resume <id>`), so only
/// characters that cannot change its meaning are accepted.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_BYTES
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn normalize(time: &str) -> Option<String> {
    let parsed = DateTime::parse_from_rfc3339(time).ok()?;
    Some(
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

/// The first non-empty `user.message` in the head window.
fn first_prompt(env: &Env, events: &Path) -> Option<String> {
    let head = env.fs.read_range(events, 0, READ_WINDOW_BYTES).ok()?;
    let prompt = objects(&head)
        .filter(|line| str_field(line, "type") == Some(USER_MESSAGE_EVENT))
        .find_map(|line| {
            let text = line.get("data")?.get("content")?.as_str()?.trim();
            (!text.is_empty()).then(|| text.chars().take(MAX_PROMPT_CHARS).collect())
        });
    prompt
}

fn read_session(env: &Env, dir: &Path) -> Option<SessionStub> {
    let workspace_path = dir.join(WORKSPACE_FILE);
    let bytes = env
        .fs
        .read_range(&workspace_path, 0, READ_WINDOW_BYTES)
        .ok()?;
    let workspace = parse_workspace(std::str::from_utf8(&bytes).ok()?);

    let dir_name = dir.file_name().and_then(|name| name.to_str());
    let id = workspace
        .id
        .as_deref()
        .or(dir_name)
        .filter(|id| valid_id(id))?;
    let cwd = workspace.cwd.filter(|cwd| cwd.len() <= MAX_CWD_BYTES)?;

    let fallback = env
        .fs
        .metadata(&workspace_path)
        .ok()
        .and_then(|meta| meta.modified)
        .map(DateTime::<Utc>::from)
        .map_or_else(
            || env.clock.now(),
            |at| at.to_rfc3339_opts(SecondsFormat::Secs, true),
        );
    let started_at = workspace
        .created_at
        .as_deref()
        .and_then(normalize)
        .unwrap_or(fallback);
    let last_activity_at = workspace
        .updated_at
        .as_deref()
        .and_then(normalize)
        .filter(|at| *at >= started_at)
        .unwrap_or_else(|| started_at.clone());

    Some(SessionStub {
        agent_session_id: id.to_owned(),
        cwd,
        repo_root: workspace
            .git_root
            .filter(|root| root.len() <= MAX_CWD_BYTES),
        branch: workspace
            .branch
            .filter(|branch| branch.len() <= MAX_BRANCH_BYTES),
        name: workspace
            .name
            .map(|name| name.chars().take(MAX_PROMPT_CHARS).collect()),
        started_at,
        last_activity_at,
        first_prompt: first_prompt(env, &dir.join(EVENTS_FILE)),
    })
}

/// Scans, then records each stub: a new session as `unknown` plus a
/// `session.discovered` event; a known session only gets missing fields filled.
pub fn discover_and_store(env: &Env, store: &Store) -> Result<DiscoverSummary, DiscoverStoreError> {
    let report = scan(env)?;
    store_report(env, store, KIND, &report, &NoGit)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::testing::FakeEnv;

    #[test]
    fn scans_through_the_fs_seam() {
        let fake = FakeEnv::new();
        fake.vars.set("COPILOT_HOME", "/c");
        fake.fs.add_file(
            "/c/session-state/s1/workspace.yaml",
            "id: s1\ncwd: /w/a\ncreated_at: 2026-10-12T09:00:00Z\n",
        );

        let report = scan(&fake.env).unwrap();

        assert_eq!(report.degraded, 0);
        assert_eq!(report.stubs.len(), 1);
        assert_eq!(report.stubs.first().unwrap().cwd, "/w/a");
    }

    #[test]
    fn colons_inside_values_are_kept() {
        let workspace = parse_workspace("cwd: C:/x\r\ncreated_at: 2026-10-12T09:00:00Z\r\n");
        assert_eq!(workspace.cwd.as_deref(), Some("C:/x"));
        assert_eq!(
            workspace.created_at.as_deref(),
            Some("2026-10-12T09:00:00Z")
        );
    }
}
