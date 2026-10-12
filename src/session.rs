//! The legacy per-session runtime files, kept only so the one-time import
//! into the store can read them (`store::legacy`).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A Claude Code session's state as the old runtime files recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    NeedsPermission,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub session_id: String,
    pub state: SessionState,
    pub cwd: String,
    pub tmux_pane: Option<String>,
    pub since: DateTime<Utc>,
}

/// Directory where the legacy status files were stored.
pub fn status_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("claude-monitor");
    }
    // getuid() has no failure mode.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/claude-monitor-{uid}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_session_ids() {
        assert!(!is_valid_session_id("../x"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id(""));
        assert!(is_valid_session_id("abc-123_XYZ"));
    }
}
