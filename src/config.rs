//! User configuration written by the Home Manager module:
//! `$XDG_CONFIG_HOME/claude-monitor/config.json` (else `$HOME/.config/...`).
//!
//! ```json
//! {"tmuxSession": "agents", "launchers": {"claude": "claude --resume {session_id}"}}
//! ```
//!
//! A missing file means defaults. A malformed file also means defaults, with a
//! warning on stderr naming the path, so a typo never blocks a respawn.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::agents::LauncherConfig;
use crate::env::Env;

pub const DEFAULT_TMUX_SESSION: &str = "agents";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    #[serde(default)]
    tmux_session: Option<String>,
    #[serde(default)]
    launchers: BTreeMap<String, String>,
}

/// Path of the config file, `None` when neither `XDG_CONFIG_HOME` (absolute)
/// nor `HOME` is set.
pub fn config_path(env: &Env) -> Option<PathBuf> {
    let base = match env.vars.var("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(xdg) if xdg.is_absolute() => xdg,
        _ => PathBuf::from(env.vars.var("HOME")?).join(".config"),
    };
    Some(base.join("claude-monitor").join("config.json"))
}

fn load(env: &Env) -> ConfigFile {
    let Some(path) = config_path(env) else {
        return ConfigFile::default();
    };
    if !env.fs.exists(&path) {
        return ConfigFile::default();
    }
    let parsed = env
        .fs
        .read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()));
    parsed.unwrap_or_else(|detail| {
        eprintln!(
            "claude-monitor: ignoring {}: {}",
            crate::text::single_line(&path.display().to_string()),
            crate::text::single_line(&detail)
        );
        ConfigFile::default()
    })
}

/// Launch command template per agent kind id; empty when none is configured.
pub fn load_launchers(env: &Env) -> BTreeMap<String, LauncherConfig> {
    load(env)
        .launchers
        .into_iter()
        .map(|(kind, template)| {
            (
                kind,
                LauncherConfig {
                    template: Some(template),
                },
            )
        })
        .collect()
}

/// The tmux session the config file forces every respawn into; `None` when
/// the file does not set one.
pub fn tmux_session_override(env: &Env) -> Option<String> {
    load(env).tmux_session.filter(|name| !name.is_empty())
}

/// Name of the tmux session respawned agents run in.
pub fn tmux_session(env: &Env) -> String {
    tmux_session_override(env).unwrap_or_else(|| DEFAULT_TMUX_SESSION.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeEnv;

    const PATH: &str = "/xdg/claude-monitor/config.json";

    fn env_with(contents: Option<&str>) -> FakeEnv {
        let fake = FakeEnv::new();
        fake.vars.set("XDG_CONFIG_HOME", "/xdg");
        if let Some(contents) = contents {
            fake.fs.add_file(PATH, contents);
        }
        fake
    }

    #[test]
    fn missing_file_gives_defaults() {
        let fake = env_with(None);
        assert!(load_launchers(&fake.env).is_empty());
        assert_eq!(tmux_session(&fake.env), "agents");
    }

    #[test]
    fn reads_launchers_and_session() {
        let fake = env_with(Some(
            r#"{"tmuxSession":"work","launchers":{"claude":"claude --resume {session_id}","copilot":"copilot"}}"#,
        ));
        let launchers = load_launchers(&fake.env);
        assert_eq!(launchers.len(), 2);
        assert_eq!(
            launchers.get("claude").and_then(|l| l.template.as_deref()),
            Some("claude --resume {session_id}")
        );
        assert_eq!(
            launchers.get("copilot").and_then(|l| l.template.as_deref()),
            Some("copilot")
        );
        assert_eq!(tmux_session(&fake.env), "work");
    }

    #[test]
    fn malformed_file_gives_defaults() {
        let fake = env_with(Some("{not json"));
        assert!(load_launchers(&fake.env).is_empty());
        assert_eq!(tmux_session(&fake.env), "agents");
    }

    #[test]
    fn falls_back_to_home_dot_config() {
        let fake = FakeEnv::new();
        fake.vars.set("HOME", "/home/u");
        fake.fs.add_file(
            "/home/u/.config/claude-monitor/config.json",
            r#"{"tmuxSession":"home"}"#,
        );
        assert_eq!(tmux_session(&fake.env), "home");
    }
}
