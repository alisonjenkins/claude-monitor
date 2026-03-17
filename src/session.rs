use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub session_id: String,
    pub status: String,
    pub cwd: String,
    pub tmux_pane: String,
    pub timestamp: i64,
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
    pub fn status_label(&self) -> &str {
        match self.status.as_str() {
            "idle_prompt" => "IDLE",
            "permission_prompt" => "PERMISSION",
            _ => &self.status,
        }
    }
}

/// Get the directory where status files are stored
pub fn status_dir() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("claude-monitor")
}

/// Read all current session status files
pub fn read_all_sessions() -> Result<Vec<SessionStatus>> {
    let dir = status_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<SessionStatus>(&content) {
                    Ok(session) => sessions.push(session),
                    Err(e) => eprintln!("Failed to parse {}: {}", path.display(), e),
                },
                Err(e) => eprintln!("Failed to read {}: {}", path.display(), e),
            }
        }
    }

    // Sort by timestamp, newest first
    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}

/// Remove a session's status file
pub fn remove_session(session_id: &str) -> Result<()> {
    let path = status_dir().join(format!("{}.json", session_id));
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}
