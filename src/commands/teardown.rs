use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn settings_path() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".claude")
        .join("settings.json")
}

/// Remove claude-monitor notification hooks from ~/.claude/settings.json
pub fn run() -> Result<()> {
    let path = settings_path();

    if !path.exists() {
        println!("No settings.json found at {}", path.display());
        return Ok(());
    }

    let content = fs::read_to_string(&path).context("Failed to read settings.json")?;
    let mut settings: Value =
        serde_json::from_str(&content).context("Failed to parse settings.json")?;

    let removed = if let Some(hooks) = settings.get_mut("hooks") {
        if let Some(notifications) = hooks.get_mut("Notification") {
            if let Some(arr) = notifications.as_array_mut() {
                let before = arr.len();
                arr.retain(|entry| {
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
                before - arr.len()
            } else {
                0
            }
        } else {
            0
        }
    } else {
        0
    };

    if removed > 0 {
        let content = serde_json::to_string_pretty(&settings)?;
        fs::write(&path, content)?;
        println!(
            "Removed {} claude-monitor hook(s) from {}",
            removed,
            path.display()
        );
    } else {
        println!("No claude-monitor hooks found in {}", path.display());
    }

    Ok(())
}
