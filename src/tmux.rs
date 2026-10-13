use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Information about a tmux pane's location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLocation {
    pub session_name: String,
    pub window_name: String,
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

/// The `-F` format `list_panes_at` asks tmux for. Fields are separated by a
/// space, not a tab: outside a UTF-8 locale tmux prints a tab as `_`. The pane
/// id and both indexes never contain a space. The window and session names
/// both may, so they share the rest of the line split at the last `:`: tmux
/// rewrites `:` in a session name to `_`, a window name may keep it.
pub const LIST_PANES_FORMAT: &str =
    "#{pane_id} #{window_index} #{pane_index} #{window_name}:#{session_name}";

/// Parse the output of `tmux list-panes -a -F` [`LIST_PANES_FORMAT`] into a
/// pane id -> location map. Malformed lines are skipped.
pub fn parse_list_panes(output: &str) -> HashMap<String, PaneLocation> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        let [pane_id, window_index, pane_index, names] = parts[..] else {
            continue;
        };
        let Some((window_name, session_name)) = names.rsplit_once(':') else {
            continue;
        };
        map.insert(
            pane_id.to_string(),
            PaneLocation {
                session_name: session_name.to_string(),
                window_name: window_name.to_string(),
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
    list_panes_at(None)
}

/// `list_panes` against the server on `socket` (`tmux -S`), or the default
/// server when `None`.
pub fn list_panes_at(socket: Option<&Path>) -> Result<Option<HashMap<String, PaneLocation>>> {
    let mut command = Command::new("tmux");
    if let Some(socket) = socket {
        command.arg("-S").arg(socket);
    }
    let output = command
        .args(["list-panes", "-a", "-F", LIST_PANES_FORMAT])
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
    fn parses_space_separated_lines_with_the_session_name_last() {
        let out = "%0 0 0 zsh:main\n%1 2 1 zsh:my project\n";
        let map = parse_list_panes(out);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("%1"),
            Some(&PaneLocation {
                session_name: "my project".to_string(),
                window_name: "zsh".to_string(),
                window_index: "2".to_string(),
                pane_index: "1".to_string(),
            })
        );
    }

    #[test]
    fn window_and_session_names_may_both_contain_spaces_and_colons() {
        let out = "%4 3 0 my win: build:my project\n%5 0 1 plain:main\n";
        let map = parse_list_panes(out);
        assert_eq!(
            map.get("%4"),
            Some(&PaneLocation {
                session_name: "my project".to_string(),
                window_name: "my win: build".to_string(),
                window_index: "3".to_string(),
                pane_index: "0".to_string(),
            })
        );
        assert_eq!(
            map.get("%5")
                .map(|l| (l.window_name.as_str(), l.session_name.as_str())),
            Some(("plain", "main"))
        );
    }

    #[test]
    fn space_format_skips_malformed_lines() {
        let map = parse_list_panes("garbage\n\n%2 1 0 w:other\n%3 1\n%6 1 0 nocolon\n");
        assert_eq!(map.len(), 1);
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
            window_name: "zsh".to_string(),
            window_index: "2".to_string(),
            pane_index: "3".to_string(),
        };
        assert_eq!(loc.format(), "main:2.3");
    }
}
