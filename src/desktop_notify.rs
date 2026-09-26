use anyhow::Result;
use std::collections::HashMap;

use crate::session::{SessionState, SessionStatus};

/// Send a desktop notification that `project` has transitioned into `state`.
/// Failures are the caller's problem to surface (e.g. in the dashboard
/// footer); this never panics and never writes to stdout/stderr itself.
pub fn notify_transition(project: &str, state: SessionState) -> Result<()> {
    let body = format!("{project}: {}", state.label());

    #[cfg(target_os = "linux")]
    {
        notify_rust::Notification::new()
            .summary("Claude Code")
            .body(&body)
            .urgency(notify_rust::Urgency::Normal)
            .show()?;
    }

    #[cfg(target_os = "macos")]
    {
        mac_notification_sys::send_notification("Claude Code", None, &body, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    Ok(())
}

/// A state worth notifying about becoming true (i.e. not `Working`).
fn is_notifiable(state: SessionState) -> bool {
    matches!(state, SessionState::NeedsPermission | SessionState::Idle)
}

/// Pure diff: given the previous state per session id and the current
/// session list, return the sessions that just transitioned into a
/// notifiable state (`NeedsPermission` or `Idle`), including sessions that
/// are new to `prev` (first seen with that state) but excluding a session
/// with no entry in `prev` when the caller passes an empty map for "startup,
/// don't notify yet" — callers decide that by only calling this after the
/// first load has seeded `prev`.
pub fn transitions_into_attention<'a>(
    prev: &HashMap<String, SessionState>,
    current: &'a [SessionStatus],
) -> Vec<&'a SessionStatus> {
    current
        .iter()
        .filter(|s| is_notifiable(s.state))
        .filter(|s| prev.get(&s.session_id) != Some(&s.state))
        .collect()
}

/// Build the `session_id -> state` map used as `prev` on the next call.
pub fn state_snapshot(sessions: &[SessionStatus]) -> HashMap<String, SessionState> {
    sessions
        .iter()
        .map(|s| (s.session_id.clone(), s.state))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::Utc;

    fn session(id: &str, state: SessionState) -> SessionStatus {
        SessionStatus {
            session_id: id.to_string(),
            state,
            cwd: format!("/tmp/{id}"),
            tmux_pane: None,
            since: Utc::now(),
        }
    }

    #[test]
    fn notifies_on_transition_into_permission() {
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), SessionState::Working);
        let current = vec![session("a", SessionState::NeedsPermission)];
        let notified = transitions_into_attention(&prev, &current);
        assert_eq!(notified.len(), 1);
        assert_eq!(notified[0].session_id, "a");
    }

    #[test]
    fn notifies_on_newly_appearing_session_already_needing_attention() {
        let prev = HashMap::new();
        let current = vec![session("a", SessionState::Idle)];
        let notified = transitions_into_attention(&prev, &current);
        assert_eq!(notified.len(), 1);
    }

    #[test]
    fn no_notification_when_state_unchanged() {
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), SessionState::Idle);
        let current = vec![session("a", SessionState::Idle)];
        assert!(transitions_into_attention(&prev, &current).is_empty());
    }

    #[test]
    fn no_notification_for_transition_into_working() {
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), SessionState::Idle);
        let current = vec![session("a", SessionState::Working)];
        assert!(transitions_into_attention(&prev, &current).is_empty());
    }

    #[test]
    fn state_snapshot_maps_ids_to_states() {
        let sessions = vec![
            session("a", SessionState::Idle),
            session("b", SessionState::Working),
        ];
        let snap = state_snapshot(&sessions);
        assert_eq!(snap.get("a"), Some(&SessionState::Idle));
        assert_eq!(snap.get("b"), Some(&SessionState::Working));
    }
}
