use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn settings_path() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".claude")
        .join("settings.json")
}

/// Install claude-monitor notification hooks into ~/.claude/settings.json
pub fn run() -> Result<()> {
    let path = settings_path();

    // Read existing settings or create empty object
    let mut settings: Value = if path.exists() {
        let content = fs::read_to_string(&path).context("Failed to read settings.json")?;
        serde_json::from_str(&content).context("Failed to parse settings.json")?
    } else {
        json!({})
    };

    let hooks = settings
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let notifications = hooks
        .as_object_mut()
        .context("hooks is not an object")?
        .entry("Notification")
        .or_insert_with(|| json!([]));

    let notification_array = notifications
        .as_array_mut()
        .context("Notification is not an array")?;

    // Remove any existing claude-monitor hooks to avoid duplicates
    notification_array.retain(|entry| {
        !entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.starts_with("claude-monitor notify"))
                })
            })
    });

    // Add our hooks
    notification_array.push(json!({
        "matcher": "idle_prompt",
        "hooks": [
            {
                "type": "command",
                "command": "claude-monitor notify idle_prompt"
            }
        ]
    }));

    notification_array.push(json!({
        "matcher": "permission_prompt",
        "hooks": [
            {
                "type": "command",
                "command": "claude-monitor notify permission_prompt"
            }
        ]
    }));

    // Write back
    let content = serde_json::to_string_pretty(&settings)?;
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, content)?;

    println!("Installed claude-monitor hooks into {}", path.display());
    println!("Note: Existing Claude Code sessions need to be restarted to pick up the hooks.");
    Ok(())
}
