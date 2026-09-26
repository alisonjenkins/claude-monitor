use serde::Deserialize;
use std::io::Read;

use crate::session::{self, Action};

/// Partial structure of the hook stdin JSON. Fields Claude Code may omit on
/// older payloads (e.g. `notification_type`) are optional, so a missing
/// field leaves state unchanged rather than failing to parse.
#[derive(Deserialize)]
struct HookInput {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    notification_type: Option<String>,
}

/// Entry point for `claude-monitor hook`, called by Claude Code's hook
/// mechanism. Always exits 0 and never writes to stdout — Claude Code may
/// interpret stdout for some events. Failures go to stderr only.
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-monitor hook: {e:#}");
    }
}

fn try_run() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let Ok(hook_input) = serde_json::from_str::<HookInput>(&input) else {
        return Ok(());
    };

    let Some(session_id) = hook_input.session_id else {
        return Ok(());
    };
    let Some(hook_event_name) = hook_input.hook_event_name else {
        return Ok(());
    };

    let action = session::map_event(&hook_event_name, hook_input.notification_type.as_deref());
    let dir = session::status_dir();

    match action {
        Action::Ignore => Ok(()),
        Action::Remove => session::remove_session(&dir, &session_id),
        Action::Set(state) => {
            let cwd = hook_input
                .cwd
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.display().to_string())
                })
                .unwrap_or_default();
            let tmux_pane = std::env::var("TMUX_PANE").ok();
            session::upsert_session(&dir, &session_id, state, cwd, tmux_pane)
        }
    }
}
