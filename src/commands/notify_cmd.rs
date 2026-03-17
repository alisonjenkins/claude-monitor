use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Read;

use crate::session;

/// Partial structure of the hook stdin JSON — we only need session_id and cwd
#[derive(Deserialize)]
struct HookInput {
    session_id: Option<String>,
    cwd: Option<String>,
}

/// Called by Claude Code notification hooks.
/// Reads hook data from stdin, writes a status file, and sends BEL to tmux.
pub fn run(status: &str) -> Result<()> {
    // Read stdin for hook JSON data
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;

    let hook_input: HookInput = serde_json::from_str(&input).unwrap_or(HookInput {
        session_id: None,
        cwd: None,
    });

    let session_id = hook_input
        .session_id
        .unwrap_or_else(|| format!("{}", std::process::id()));

    let cwd = hook_input
        .cwd
        .or_else(|| std::env::current_dir().ok().map(|p| p.display().to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    let tmux_pane = std::env::var("TMUX_PANE").unwrap_or_else(|_| "unknown".to_string());

    let timestamp = chrono::Local::now().timestamp();

    let session_status = session::SessionStatus {
        session_id: session_id.clone(),
        status: status.to_string(),
        cwd,
        tmux_pane,
        timestamp,
    };

    // Write status file
    let dir = session::status_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", session_id));
    let json = serde_json::to_string_pretty(&session_status)?;
    fs::write(&path, json)?;

    // Send BEL to terminal for tmux bell detection
    eprint!("\x07");

    Ok(())
}
