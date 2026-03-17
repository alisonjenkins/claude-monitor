use anyhow::{Context, Result};
use std::collections::HashSet;
use std::process::Command;

/// Information about a tmux pane's location
#[derive(Debug)]
pub struct PaneLocation {
    pub session_name: String,
    pub window_index: String,
    pub pane_index: String,
}

/// Resolve a tmux pane ID (e.g. "%5") to its session:window.pane location
pub fn resolve_pane(pane_id: &str) -> Result<Option<PaneLocation>> {
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} #{session_name} #{window_index} #{pane_index}",
        ])
        .output()
        .context("Failed to run tmux list-panes")?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        if parts.len() == 4 && parts[0] == pane_id {
            return Ok(Some(PaneLocation {
                session_name: parts[1].to_string(),
                window_index: parts[2].to_string(),
                pane_index: parts[3].to_string(),
            }));
        }
    }

    Ok(None)
}

/// Get the set of all currently existing tmux pane IDs
pub fn list_all_pane_ids() -> Result<HashSet<String>> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_id}"])
        .output()
        .context("Failed to run tmux list-panes")?;

    if !output.status.success() {
        return Ok(HashSet::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().map(|l| l.to_string()).collect())
}

/// Switch to the tmux pane identified by pane_id
pub fn switch_to_pane(pane_id: &str) -> Result<()> {
    let loc = resolve_pane(pane_id)?.context("Pane not found")?;

    // Switch client to the session
    Command::new("tmux")
        .args(["switch-client", "-t", &loc.session_name])
        .output()
        .context("Failed to switch tmux client")?;

    // Select the window
    let window_target = format!("{}:{}", loc.session_name, loc.window_index);
    Command::new("tmux")
        .args(["select-window", "-t", &window_target])
        .output()
        .context("Failed to select tmux window")?;

    // Select the pane
    let pane_target = format!("{}.{}", window_target, loc.pane_index);
    Command::new("tmux")
        .args(["select-pane", "-t", &pane_target])
        .output()
        .context("Failed to select tmux pane")?;

    Ok(())
}

/// Format a pane location for display
pub fn format_pane_location(pane_id: &str) -> String {
    match resolve_pane(pane_id) {
        Ok(Some(loc)) => format!("{}:{}.{}", loc.session_name, loc.window_index, loc.pane_index),
        _ => pane_id.to_string(),
    }
}
