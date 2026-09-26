use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Command;

/// Information about a tmux pane's location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLocation {
    pub session_name: String,
    pub window_index: String,
    pub pane_index: String,
}

impl PaneLocation {
    pub fn format(&self) -> String {
        format!(
            "{}:{}.{}",
            self.session_name, self.window_index, self.pane_index
        )
    }
}

/// Parse the output of `tmux list-panes -a -F "#{pane_id} #{session_name}
/// #{window_index} #{pane_index}"` into a pane id -> location map. Lines that
/// don't have exactly four space-separated fields are skipped.
pub fn parse_list_panes(output: &str) -> HashMap<String, PaneLocation> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        let [pane_id, session_name, window_index, pane_index] = parts[..] else {
            continue;
        };
        map.insert(
            pane_id.to_string(),
            PaneLocation {
                session_name: session_name.to_string(),
                window_index: window_index.to_string(),
                pane_index: pane_index.to_string(),
            },
        );
    }
    map
}

/// Run `tmux list-panes -a` once and return every pane's location, keyed by
/// pane id. `Ok(None)` means tmux itself is unavailable or the command
/// failed (no server running); callers should treat that as "unknown", not
/// as "no panes".
pub fn list_panes() -> Result<Option<HashMap<String, PaneLocation>>> {
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} #{session_name} #{window_index} #{pane_index}",
        ])
        .output()
        .context("failed to run tmux list-panes")?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(Some(parse_list_panes(&stdout)))
}

/// Switch to the tmux pane identified by `pane_id`, using an already
/// resolved location so this never spawns an extra `list-panes` call.
pub fn switch_to_pane(loc: &PaneLocation) -> Result<()> {
    Command::new("tmux")
        .args(["switch-client", "-t", &loc.session_name])
        .output()
        .context("failed to switch tmux client")?;

    let window_target = format!("{}:{}", loc.session_name, loc.window_index);
    Command::new("tmux")
        .args(["select-window", "-t", &window_target])
        .output()
        .context("failed to select tmux window")?;

    let pane_target = format!("{}.{}", window_target, loc.pane_index);
    Command::new("tmux")
        .args(["select-pane", "-t", &pane_target])
        .output()
        .context("failed to select tmux pane")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn parses_well_formed_lines() {
        let out = "%0 main 0 0\n%1 main 0 1\n%2 other 1 0\n";
        let map = parse_list_panes(out);
        assert_eq!(map.len(), 3);
        assert_eq!(
            map.get("%1"),
            Some(&PaneLocation {
                session_name: "main".to_string(),
                window_index: "0".to_string(),
                pane_index: "1".to_string(),
            })
        );
    }

    #[test]
    fn skips_malformed_lines() {
        let out = "%0 main 0 0\ngarbage\n\n%2 other 1 0\n";
        let map = parse_list_panes(out);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("%0"));
        assert!(map.contains_key("%2"));
    }

    #[test]
    fn empty_input_yields_empty_map() {
        assert!(parse_list_panes("").is_empty());
    }

    #[test]
    fn format_renders_session_window_pane() {
        let loc = PaneLocation {
            session_name: "main".to_string(),
            window_index: "2".to_string(),
            pane_index: "3".to_string(),
        };
        assert_eq!(loc.format(), "main:2.3");
    }
}
