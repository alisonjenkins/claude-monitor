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

/// Parse the output of `tmux list-panes -a -F "#{pane_id}\t#{session_name}\t
/// #{window_index}\t#{pane_index}"` into a pane id -> location map. Lines that
/// don't have exactly four tab-separated fields are skipped. Tabs are used
/// because tmux session names may contain spaces.
pub fn parse_list_panes(output: &str) -> HashMap<String, PaneLocation> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
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
            "#{pane_id}\t#{session_name}\t#{window_index}\t#{pane_index}",
        ])
        .output()
        .context("failed to run tmux list-panes")?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(Some(parse_list_panes(&stdout)))
}

/// Run one tmux command, failing if tmux exits non-zero so a failed switch
/// is reported instead of silently ignored.
fn run_tmux(args: &[&str]) -> Result<()> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| format!("failed to run tmux {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Switch the current tmux client to the pane with id `pane_id` (e.g. "%5").
/// Pane ids are valid targets for every command, so this is immune to
/// session names containing spaces or dots.
pub fn switch_to_pane(pane_id: &str) -> Result<()> {
    run_tmux(&["switch-client", "-t", pane_id])?;
    run_tmux(&["select-window", "-t", pane_id])?;
    run_tmux(&["select-pane", "-t", pane_id])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn parses_well_formed_lines() {
        let out = "%0\tmain\t0\t0\n%1\tmain\t0\t1\n%2\tother\t1\t0\n";
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
        let out = "%0\tmain\t0\t0\ngarbage\n\n%2\tother\t1\t0\n";
        let map = parse_list_panes(out);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("%0"));
        assert!(map.contains_key("%2"));
    }

    #[test]
    fn session_names_may_contain_spaces() {
        let map = parse_list_panes("%7\tmy project\t2\t1\n");
        assert_eq!(
            map.get("%7").map(|l| l.session_name.as_str()),
            Some("my project")
        );
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
