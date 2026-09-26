use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::Read;

use crate::session::{self, SessionState};

/// Partial structure of the hook stdin JSON — we only need session_id and cwd
#[derive(Deserialize)]
struct HookInput {
    session_id: Option<String>,
    cwd: Option<String>,
}

/// Called by Claude Code notification hooks.
/// Reads hook data from stdin and updates the session's status file.
pub fn run(status: &str) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;

    let hook_input: HookInput = serde_json::from_str(&input).unwrap_or(HookInput {
        session_id: None,
        cwd: None,
    });

    let Some(session_id) = hook_input.session_id else {
        return Ok(());
    };

    let cwd = hook_input
        .cwd
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    let tmux_pane = std::env::var("TMUX_PANE").ok();

    let state = match status {
        "permission_prompt" => SessionState::NeedsPermission,
        _ => SessionState::Idle,
    };

    let dir = session::status_dir();
    session::upsert_session(&dir, &session_id, state, cwd, tmux_pane)
}
