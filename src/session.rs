use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A Claude Code session's current state, as tracked from hook events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    NeedsPermission,
    Idle,
}

impl SessionState {
    pub fn label(&self) -> &'static str {
        match self {
            SessionState::Working => "WORKING",
            SessionState::NeedsPermission => "PERMISSION",
            SessionState::Idle => "IDLE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub session_id: String,
    pub state: SessionState,
    pub cwd: String,
    pub tmux_pane: Option<String>,
    pub since: DateTime<Utc>,
}

impl SessionStatus {
    /// Get a display-friendly project name from the cwd
    pub fn project_name(&self) -> &str {
        Path::new(&self.cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.cwd)
    }

    /// Get a human-readable status label
    pub fn state_label(&self) -> &'static str {
        self.state.label()
    }
}

/// What a hook event implies should happen to a session's recorded state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Set(SessionState),
    Remove,
    Ignore,
}

/// Pure mapping from a hook's `hook_event_name` (and, for `Notification`
/// events, its `notification_type`) to the action it implies. Kept free of
/// I/O so the mapping table can be tested directly.
pub fn map_event(hook_event_name: &str, notification_type: Option<&str>) -> Action {
    match hook_event_name {
        "SessionStart" => Action::Set(SessionState::Idle),
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => Action::Set(SessionState::Working),
        "Notification" => match notification_type {
            Some("permission_prompt") => Action::Set(SessionState::NeedsPermission),
            Some("idle_prompt") => Action::Set(SessionState::Idle),
            _ => Action::Ignore,
        },
        "Stop" => Action::Set(SessionState::Idle),
        "SessionEnd" => Action::Remove,
        _ => Action::Ignore,
    }
}

/// Directory where status files are stored.
pub fn status_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("claude-monitor");
    }
    // getuid() has no failure mode.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/claude-monitor-{uid}"))
}

/// Create the status directory (mode 0700) if it does not already exist.
pub fn ensure_status_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("setting permissions on {}", dir.display()))?;
    }
    Ok(())
}

/// A session id is used verbatim as a filename, so only a conservative
/// character set is allowed; anything else (including path separators) is
/// rejected rather than sanitised, so callers can treat it as a no-op.
pub fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn status_path(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("{session_id}.json"))
}

fn read_session_file(path: &Path) -> Option<SessionStatus> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write `contents` to `path` atomically: write to a sibling temp file, then
/// rename over the destination.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no file name", path.display()))?;
    let tmp_path = dir.join(format!("{file_name}.tmp.{}", std::process::id()));

    fs::write(&tmp_path, contents).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

/// Create or update a session's status file. `since` is preserved from the
/// existing file when the state is unchanged, and refreshed otherwise. A
/// session id that fails [`is_valid_session_id`] is a no-op.
pub fn upsert_session(
    dir: &Path,
    session_id: &str,
    state: SessionState,
    cwd: String,
    tmux_pane: Option<String>,
) -> Result<()> {
    if !is_valid_session_id(session_id) {
        return Ok(());
    }
    ensure_status_dir(dir)?;

    let path = status_path(dir, session_id);
    let since = match read_session_file(&path) {
        Some(existing) if existing.state == state => existing.since,
        _ => Utc::now(),
    };

    let status = SessionStatus {
        session_id: session_id.to_string(),
        state,
        cwd,
        tmux_pane,
        since,
    };
    let json = serde_json::to_vec_pretty(&status).context("serialising session status")?;
    write_atomic(&path, &json)
}

/// Remove a session's status file. A missing or invalid session id is a
/// no-op.
pub fn remove_session(dir: &Path, session_id: &str) -> Result<()> {
    if !is_valid_session_id(session_id) {
        return Ok(());
    }
    let path = status_path(dir, session_id);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Read all current session status files. Non-`.json` entries and files that
/// fail to parse are silently skipped — the TUI owns the terminal, so this
/// never prints.
pub fn read_all_sessions(dir: &Path) -> Vec<SessionStatus> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut sessions: Vec<SessionStatus> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .filter_map(|path| read_session_file(&path))
        .collect();

    sessions.sort_by_key(|s| std::cmp::Reverse(s.since));
    sessions
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn map_event_table() {
        let cases: &[(&str, Option<&str>, Action)] = &[
            ("SessionStart", None, Action::Set(SessionState::Idle)),
            ("UserPromptSubmit", None, Action::Set(SessionState::Working)),
            ("PreToolUse", None, Action::Set(SessionState::Working)),
            ("PostToolUse", None, Action::Set(SessionState::Working)),
            (
                "Notification",
                Some("permission_prompt"),
                Action::Set(SessionState::NeedsPermission),
            ),
            (
                "Notification",
                Some("idle_prompt"),
                Action::Set(SessionState::Idle),
            ),
            ("Notification", None, Action::Ignore),
            ("Notification", Some("something_else"), Action::Ignore),
            ("Stop", None, Action::Set(SessionState::Idle)),
            ("SessionEnd", None, Action::Remove),
            ("SomeUnknownEvent", None, Action::Ignore),
        ];

        for (event, notif, expected) in cases {
            assert_eq!(
                map_event(event, *notif),
                *expected,
                "event={event} notif={notif:?}"
            );
        }
    }

    #[test]
    fn rejects_path_traversal_session_ids() {
        assert!(!is_valid_session_id("../x"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id(""));
        assert!(is_valid_session_id("abc-123_XYZ"));
    }

    #[test]
    fn since_preserved_on_same_state() {
        let dir = tempdir().unwrap();
        upsert_session(
            dir.path(),
            "sess1",
            SessionState::Working,
            "/tmp/a".to_string(),
            None,
        )
        .unwrap();
        let first = read_session_file(&status_path(dir.path(), "sess1")).unwrap();

        sleep(Duration::from_millis(10));
        upsert_session(
            dir.path(),
            "sess1",
            SessionState::Working,
            "/tmp/b".to_string(),
            Some("%1".to_string()),
        )
        .unwrap();
        let second = read_session_file(&status_path(dir.path(), "sess1")).unwrap();

        assert_eq!(first.since, second.since);
        assert_eq!(second.cwd, "/tmp/b");
        assert_eq!(second.tmux_pane, Some("%1".to_string()));
    }

    #[test]
    fn since_updates_on_state_change() {
        let dir = tempdir().unwrap();
        upsert_session(
            dir.path(),
            "sess1",
            SessionState::Working,
            "/tmp/a".to_string(),
            None,
        )
        .unwrap();
        let first = read_session_file(&status_path(dir.path(), "sess1")).unwrap();

        sleep(Duration::from_millis(10));
        upsert_session(
            dir.path(),
            "sess1",
            SessionState::Idle,
            "/tmp/a".to_string(),
            None,
        )
        .unwrap();
        let second = read_session_file(&status_path(dir.path(), "sess1")).unwrap();

        assert!(second.since > first.since);
    }

    #[test]
    fn session_end_removes_file() {
        let dir = tempdir().unwrap();
        upsert_session(
            dir.path(),
            "sess1",
            SessionState::Working,
            "/tmp/a".to_string(),
            None,
        )
        .unwrap();
        assert!(status_path(dir.path(), "sess1").exists());

        remove_session(dir.path(), "sess1").unwrap();
        assert!(!status_path(dir.path(), "sess1").exists());
    }

    #[test]
    fn atomic_write_leaves_no_tmp_file() {
        let dir = tempdir().unwrap();
        upsert_session(
            dir.path(),
            "sess1",
            SessionState::Working,
            "/tmp/a".to_string(),
            None,
        )
        .unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "leftover tmp files: {leftovers:?}");
    }

    #[test]
    fn read_all_sessions_skips_garbage() {
        let dir = tempdir().unwrap();
        upsert_session(
            dir.path(),
            "good",
            SessionState::Idle,
            "/tmp/good".to_string(),
            None,
        )
        .unwrap();
        fs::write(dir.path().join("not-json.txt"), b"ignored").unwrap();
        fs::write(dir.path().join("garbage.json"), b"{not valid json").unwrap();

        let sessions = read_all_sessions(dir.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "good");
    }

    #[test]
    fn invalid_session_id_is_noop() {
        let dir = tempdir().unwrap();
        upsert_session(
            dir.path(),
            "../evil",
            SessionState::Working,
            "/tmp/a".to_string(),
            None,
        )
        .unwrap();
        assert!(read_all_sessions(dir.path()).is_empty());
    }
}
