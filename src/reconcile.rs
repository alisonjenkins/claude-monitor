//! Liveness reconciliation: marks sessions whose agent process, tmux pane or
//! machine boot is gone as interrupted (FR-005..FR-008).
//!
//! [`decide`] is the pure rule; [`reconcile`] gathers the observations from the
//! providers and writes the result. Reconcile is idempotent: it appends one
//! event per real change and nothing when a second pass finds the same facts.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Transaction, TransactionBehavior};
use serde_json::json;

use crate::env::Env;
use crate::model::{Author, Liveness, Session, State};
use crate::store::events::{EventsError, NewEvent};
use crate::store::sessions::{SessionRow, SessionsError};
use crate::store::Store;
use crate::tmux::PaneLocation;

const RECONCILE_COMPONENT: &str = "reconcile";
const ENTITY_SESSION: &str = "session";

/// Which sessions a reconcile pass looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Pane(String),
    Session(String),
}

/// Why a session was marked interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    ProcessGone,
    BootChanged,
    PaneGone,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessGone => "process_gone",
            Self::BootChanged => "boot_changed",
            Self::PaneGone => "pane_gone",
        }
    }

    fn liveness(self) -> Liveness {
        match self {
            Self::BootChanged => Liveness::BootChanged,
            Self::ProcessGone | Self::PaneGone => Liveness::ProcessGone,
        }
    }
}

/// What the providers say about one session; `None` means "could not tell".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Observation<'a> {
    /// The current boot id.
    pub boot_id: Option<&'a str>,
    /// Whether the recorded pid is running with the recorded start time.
    pub process_alive: Option<bool>,
    /// Whether the recorded tmux pane still exists.
    pub pane_exists: Option<bool>,
}

/// The rule: `Some(reason)` when `session` must become interrupted.
///
/// Only working, waiting and idle sessions can be interrupted: finished ones
/// are over, interrupted ones already are, and unknown ones were never seen
/// by a hook, so nothing says they should be running. A different boot id
/// beats everything. Without a pid the session is not judged by processes; a
/// vanished tmux pane is a secondary signal used only then.
pub fn decide(session: &Session, observation: &Observation<'_>) -> Option<Reason> {
    if !session.state.is_active() {
        return None;
    }
    if let (Some(recorded), Some(current)) = (session.boot_id.as_deref(), observation.boot_id) {
        if recorded != current {
            return Some(Reason::BootChanged);
        }
    }
    if session.pid.is_some() {
        return (observation.process_alive == Some(false)).then_some(Reason::ProcessGone);
    }
    if session.tmux_pane.is_some() && observation.pane_exists == Some(false) {
        return Some(Reason::PaneGone);
    }
    None
}

/// A row that could not be read or updated; reconcile skips it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DegradedSession {
    pub id: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Readable sessions in scope.
    pub checked: usize,
    pub interrupted: usize,
    /// Always 0: only a hook revives a session.
    pub revived: usize,
    /// Checked sessions with no change at all.
    pub unchanged: usize,
    pub degraded: Vec<DegradedSession>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("cannot list sessions: {0}")]
    List(#[from] SessionsError),
    #[error("cannot begin reconcile transaction: {0}")]
    Begin(rusqlite::Error),
    #[error("cannot commit reconcile transaction: {0}")]
    Commit(rusqlite::Error),
}

#[derive(Debug, thiserror::Error)]
enum RowError {
    #[error("{0}")]
    Write(#[from] SessionsError),
    #[error("{0}")]
    Event(#[from] EventsError),
}

/// Apply the liveness rules to the sessions in `scope`, refresh their
/// `worktree_exists`, and append one event per real change.
pub fn reconcile(
    store: &Store,
    env: &Env,
    scope: Scope,
    now: &str,
) -> Result<ReconcileReport, ReconcileError> {
    let rows = store.list_sessions_lenient()?;
    let mut report = ReconcileReport::default();
    let mut sessions = Vec::new();
    for row in rows {
        match row {
            SessionRow::Readable(session) if in_scope(&scope, &session) => sessions.push(*session),
            // A corrupt row has no readable pane, so only `All` and its own id reach it.
            SessionRow::Corrupt { id, error }
                if match &scope {
                    Scope::All => true,
                    Scope::Session(wanted) => *wanted == id,
                    Scope::Pane(_) => false,
                } =>
            {
                report.degraded.push(DegradedSession { id, error });
            }
            SessionRow::Readable(_) | SessionRow::Corrupt { .. } => {}
        }
    }

    let boot_id = env.boot_id.current().ok();
    let panes = live_panes(env, &sessions);
    let tx = Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(ReconcileError::Begin)?;
    for session in sessions {
        report.checked = report.checked.saturating_add(1);
        let observation = observe(env, &session, boot_id.as_deref(), panes.as_ref());
        let id = session.id.clone();
        match apply(store, env, session, &observation, panes.as_ref(), now) {
            Ok(Outcome::Unchanged) => report.unchanged = report.unchanged.saturating_add(1),
            Ok(Outcome::Interrupted) => {
                report.interrupted = report.interrupted.saturating_add(1);
            }
            Ok(Outcome::Refreshed) => {}
            Err(error) => report.degraded.push(DegradedSession {
                id,
                error: error.to_string(),
            }),
        }
    }
    tx.commit().map_err(ReconcileError::Commit)?;
    Ok(report)
}

fn in_scope(scope: &Scope, session: &Session) -> bool {
    match scope {
        Scope::All => true,
        Scope::Pane(pane) => session.tmux_pane.as_deref() == Some(pane.as_str()),
        Scope::Session(id) => session.id == *id,
    }
}

/// The panes tmux knows, loaded only when an active session has a pane to
/// look up; `None` when none does or tmux is unavailable.
fn live_panes(env: &Env, sessions: &[Session]) -> Option<HashMap<String, PaneLocation>> {
    let needed = sessions
        .iter()
        .any(|s| s.state.is_active() && s.tmux_pane.is_some());
    if !needed {
        return None;
    }
    env.tmux.list_panes().ok().flatten()
}

/// Gathers what [`decide`] needs; a boot change settles the question, so no
/// process is probed then.
fn observe<'a>(
    env: &Env,
    session: &Session,
    boot_id: Option<&'a str>,
    panes: Option<&HashMap<String, PaneLocation>>,
) -> Observation<'a> {
    let boot_changed = matches!(
        (session.boot_id.as_deref(), boot_id),
        (Some(recorded), Some(current)) if recorded != current
    );
    let process_alive = match (session.pid, session.pid_start) {
        (Some(pid), Some(start)) if !boot_changed => Some(env.processes.is_alive(pid, start)),
        _ => None,
    };
    let pane_exists = match (&session.tmux_pane, panes) {
        (Some(pane), Some(panes)) => Some(panes.contains_key(pane)),
        _ => None,
    };
    Observation {
        boot_id,
        process_alive,
        pane_exists,
    }
}

enum Outcome {
    Unchanged,
    Interrupted,
    /// Only `worktree_exists` changed.
    Refreshed,
}

fn apply(
    store: &Store,
    env: &Env,
    mut session: Session,
    observation: &Observation<'_>,
    panes: Option<&HashMap<String, PaneLocation>>,
    now: &str,
) -> Result<Outcome, RowError> {
    let author = Author::System {
        component: RECONCILE_COMPONENT.to_owned(),
    };
    let previous = session.state;
    let reason = decide(&session, observation);
    if let Some(reason) = reason {
        session.reconcile(reason.liveness(), now);
    }
    let worktree_exists = session.cwd.is_empty() || env.fs.exists(Path::new(&session.cwd));
    let worktree_changed = worktree_exists != session.worktree_exists;
    session.worktree_exists = worktree_exists;
    // A pane id only names this session's pane while the session is live: after
    // a reboot tmux reuses the ids. A vanished pane leaves the old placement.
    let placement_changed = reason.is_none()
        && session.state.is_active()
        && session
            .tmux_pane
            .as_ref()
            .and_then(|pane| panes?.get(pane))
            .is_some_and(|location| session.record_tmux_location(location));
    if reason.is_none() && !worktree_changed && !placement_changed {
        return Ok(Outcome::Unchanged);
    }

    store.upsert_session(&session)?;
    if let Some(reason) = reason {
        let payload = json!({
            "state": State::Interrupted.as_str(),
            "end_reason": session.end_reason,
            "from": previous.as_str(),
            "reason": reason.as_str(),
        });
        store.append_event(&NewEvent {
            at: now,
            entity_kind: ENTITY_SESSION,
            entity_id: &session.id,
            kind: "session.state_changed",
            payload: &payload,
            author: &author,
        })?;
    }
    if worktree_changed {
        let payload = json!({ "worktree_exists": worktree_exists });
        store.append_event(&NewEvent {
            at: now,
            entity_kind: ENTITY_SESSION,
            entity_id: &session.id,
            kind: "session.worktree_changed",
            payload: &payload,
            author: &author,
        })?;
    }
    Ok(if reason.is_some() {
        Outcome::Interrupted
    } else {
        Outcome::Refreshed
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn session(state: State) -> Session {
        let mut s = Session::new("s", "claude", "a", "/w", "2026-10-12T00:00:00Z");
        s.state = state;
        s.pid = Some(10);
        s.pid_start = Some(5);
        s.boot_id = Some("boot-1".to_owned());
        s
    }

    fn observed(alive: Option<bool>) -> Observation<'static> {
        Observation {
            boot_id: Some("boot-1"),
            process_alive: alive,
            pane_exists: None,
        }
    }

    #[test]
    fn a_dead_process_interrupts_an_active_session() {
        for state in [State::Working, State::NeedsPermission, State::Idle] {
            assert_eq!(
                decide(&session(state), &observed(Some(false))),
                Some(Reason::ProcessGone),
                "{state}"
            );
        }
    }

    #[test]
    fn a_live_or_unprobed_process_changes_nothing() {
        let s = session(State::Working);
        assert_eq!(decide(&s, &observed(Some(true))), None);
        assert_eq!(decide(&s, &observed(None)), None);
    }

    #[test]
    fn finished_interrupted_and_unknown_sessions_are_never_touched() {
        let reboot = Observation {
            boot_id: Some("boot-2"),
            process_alive: Some(false),
            pane_exists: Some(false),
        };
        for state in [State::Finished, State::Interrupted, State::Unknown] {
            assert_eq!(decide(&session(state), &reboot), None, "{state}");
        }
    }

    #[test]
    fn a_different_boot_id_wins_over_a_live_process() {
        let observation = Observation {
            boot_id: Some("boot-2"),
            process_alive: Some(true),
            pane_exists: None,
        };
        assert_eq!(
            decide(&session(State::Idle), &observation),
            Some(Reason::BootChanged)
        );
    }

    #[test]
    fn no_recorded_boot_id_cannot_be_a_reboot() {
        let mut s = session(State::Idle);
        s.boot_id = None;
        let observation = Observation {
            boot_id: Some("boot-2"),
            process_alive: None,
            pane_exists: None,
        };
        assert_eq!(decide(&s, &observation), None);
    }

    #[test]
    fn a_missing_pane_counts_only_for_a_session_with_no_pid() {
        let gone = Observation {
            boot_id: Some("boot-1"),
            process_alive: Some(true),
            pane_exists: Some(false),
        };
        let mut with_pid = session(State::Idle);
        with_pid.tmux_pane = Some("%1".to_owned());
        assert_eq!(decide(&with_pid, &gone), None);

        let mut no_pid = with_pid.clone();
        no_pid.pid = None;
        no_pid.pid_start = None;
        assert_eq!(decide(&no_pid, &gone), Some(Reason::PaneGone));

        no_pid.tmux_pane = None;
        assert_eq!(decide(&no_pid, &gone), None);
        let unsure = Observation {
            pane_exists: None,
            ..gone
        };
        no_pid.tmux_pane = Some("%1".to_owned());
        assert_eq!(decide(&no_pid, &unsure), None);
    }

    #[test]
    fn a_pid_without_a_start_time_is_not_judged() {
        let mut s = session(State::Working);
        s.pid_start = None;
        assert_eq!(decide(&s, &observed(None)), None);
    }

    #[test]
    fn reasons_have_stable_names() {
        assert_eq!(Reason::ProcessGone.as_str(), "process_gone");
        assert_eq!(Reason::BootChanged.as_str(), "boot_changed");
        assert_eq!(Reason::PaneGone.as_str(), "pane_gone");
    }
}
