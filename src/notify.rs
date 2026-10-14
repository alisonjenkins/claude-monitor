//! What the dashboard and `watch` notify about, and how it reads: one rule
//! set so a headless watcher and an open dashboard say the same thing.

use crate::model::State;
use crate::text::single_line;

pub const SUMMARY: &str = "Claude Code";

/// A session waiting on the person: for a permission, or idle at a prompt.
pub fn is_notifiable(state: State) -> bool {
    matches!(state, State::NeedsPermission | State::Idle)
}

pub fn state_label(state: State) -> &'static str {
    match state {
        State::Working => "WORKING",
        State::NeedsPermission => "PERMISSION",
        State::Idle => "IDLE",
        State::Finished => "FINISHED",
        State::Interrupted => "INTERRUPTED",
        State::Unknown => "UNKNOWN",
    }
}

/// The notification body: `<project>: <STATE>`.
pub fn body(project: &str, state: State) -> String {
    format!("{}: {}", single_line(project), state_label(state))
}
