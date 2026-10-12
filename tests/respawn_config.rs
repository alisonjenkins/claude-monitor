//! Effective respawn settings: environment variable, else `config.json`,
//! else the default.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use claude_monitor::respawn::{launcher_for, tmux_session_for};
use claude_monitor::testing::FakeEnv;

const CONFIG: &str = "/home/u/.config/claude-monitor/config.json";

const FULL: &str =
    r#"{"tmuxSession":"work","launchers":{"claude":"claude --resume {session_id}"}}"#;

fn env_with_config(contents: &str) -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_file(CONFIG, contents);
    fake
}

#[test]
fn launcher_falls_back_to_config_json() {
    let fake = env_with_config(FULL);
    let launcher = launcher_for(&fake.env, "claude");
    assert_eq!(
        launcher.template.as_deref(),
        Some("claude --resume {session_id}")
    );
    assert_eq!(launcher_for(&fake.env, "copilot").template, None);
}

#[test]
fn launcher_env_var_beats_config_json() {
    let fake = env_with_config(FULL);
    fake.vars.set("CLAUDE_MONITOR_LAUNCHER_CLAUDE", "sleep 30");
    assert_eq!(
        launcher_for(&fake.env, "claude").template.as_deref(),
        Some("sleep 30")
    );
}

#[test]
fn launcher_is_none_without_env_or_config() {
    let fake = FakeEnv::new();
    assert_eq!(launcher_for(&fake.env, "claude").template, None);
}

#[test]
fn tmux_session_falls_back_to_config_json() {
    let fake = env_with_config(FULL);
    assert_eq!(tmux_session_for(&fake.env), "work");
}

#[test]
fn tmux_session_defaults_without_env_or_config() {
    assert_eq!(tmux_session_for(&FakeEnv::new().env), "agents");
}

#[test]
fn tmux_session_env_var_beats_config_json() {
    let fake = env_with_config(FULL);
    fake.vars.set("CLAUDE_MONITOR_TMUX_SESSION", "override");
    assert_eq!(tmux_session_for(&fake.env), "override");
}
