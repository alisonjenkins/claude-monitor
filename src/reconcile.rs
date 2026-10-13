//! Liveness reconciliation: marks sessions whose agent process, tmux pane or
//! machine boot is gone as interrupted (FR-005..FR-008).
//!
//! [`decide`] is the pure rule; [`reconcile`] gathers the observations from the
//! providers and writes the result. Reconcile is idempotent: it appends one
//! event per real change and nothing when a second pass finds the same facts.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Transaction, TransactionBehavior};
use serde_json::json;

use crate::agents::claude::ClaudeKind;
use crate::agents::AgentKind;
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
    /// Claude's live-session registry has no entry for it and no pid is recorded.
    RegistryMissing,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessGone => "process_gone",
            Self::BootChanged => "boot_changed",
            Self::PaneGone => "pane_gone",
            Self::RegistryMissing => "registry_missing",
        }
    }

    fn liveness(self) -> Liveness {
        match self {
            Self::BootChanged => Liveness::BootChanged,
            Self::ProcessGone | Self::PaneGone | Self::RegistryMissing => Liveness::ProcessGone,
        }
    }
}

/// A session this young may not have a registry file yet: its hook can beat it.
const REGISTRY_GRACE_SECS: i64 = 120;

/// The session ids Claude's registry lists as running. Pass it only from a scan
/// that succeeded and found at least one: an empty or failed scan cannot vouch
/// for absence.
pub type LiveRegistryIds = HashSet<String>;

/// What the providers say about one session; `None` means "could not tell".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Observation<'a> {
    /// The current boot id.
    pub boot_id: Option<&'a str>,
    /// Whether the recorded pid is running with the recorded start time.
    pub process_alive: Option<bool>,
    /// Whether the recorded tmux pane still exists.
    pub pane_exists: Option<bool>,
    /// A Claude session old enough to be in the registry, which has no live
    /// entry for it. False when the registry could not be trusted.
    pub registry_missing: bool,
}

/// The rule: `Some(reason)` when `session` must become interrupted.
///
/// Only working, waiting and idle sessions can be interrupted: finished ones
/// are over, interrupted ones already are, and unknown ones were never seen
/// by a hook, so nothing says they should be running. A different boot id
/// beats everything. Without a pid the session is not judged by processes; a
/// vanished tmux pane, then a missing registry entry, are secondary signals
/// used only then.
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
    observation
        .registry_missing
        .then_some(Reason::RegistryMissing)
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
    /// Always 0: reconcile never revives; a hook or a live registry entry
    /// does (counted in `RefreshReport::registry_updated`).
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
    reconcile_with_registry(store, env, scope, now, None)
}

/// [`reconcile`] that also interrupts Claude sessions `live_ids` does not list.
pub fn reconcile_with_registry(
    store: &Store,
    env: &Env,
    scope: Scope,
    now: &str,
    live_ids: Option<&LiveRegistryIds>,
) -> Result<ReconcileReport, ReconcileError> {
    // Probe before taking the write lock: tmux and a stat on a hung mount can
    // block, and a snapshot written back after the probes would revert a hook
    // or viewer change that landed meanwhile.
    let (snapshot, _) = in_scope_sessions(store, &scope)?;
    let boot_id = env.boot_id.current().ok();
    let panes = live_panes(env, &snapshot);
    let probes: HashMap<String, Probe> = snapshot
        .iter()
        .map(|s| (s.id.clone(), Probe::gather(env, s, boot_id.as_deref())))
        .collect();

    let tx = Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(ReconcileError::Begin)?;
    let (sessions, degraded) = in_scope_sessions(store, &scope)?;
    let mut report = ReconcileReport {
        degraded,
        ..ReconcileReport::default()
    };
    for session in sessions {
        // Created since the probes ran: the next pass sees it.
        let Some(probe) = probes.get(&session.id) else {
            continue;
        };
        report.checked = report.checked.saturating_add(1);
        let mut observation = observe(&session, boot_id.as_deref(), panes.as_ref(), probe);
        observation.registry_missing =
            live_ids.is_some_and(|ids| missing_from_registry(&session, ids, now));
        let id = session.id.clone();
        match apply(store, session, &observation, probe, panes.as_ref(), now) {
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

/// The readable sessions in `scope`, and the unreadable rows it reaches.
fn in_scope_sessions(
    store: &Store,
    scope: &Scope,
) -> Result<(Vec<Session>, Vec<DegradedSession>), ReconcileError> {
    let mut sessions = Vec::new();
    let mut degraded = Vec::new();
    for row in store.list_sessions_lenient()? {
        match row {
            SessionRow::Readable(session) if in_scope(scope, &session) => sessions.push(*session),
            // A corrupt row has no readable pane, so only `All` and its own id reach it.
            SessionRow::Corrupt { id, error }
                if match scope {
                    Scope::All => true,
                    Scope::Session(wanted) => *wanted == id,
                    Scope::Pane(_) => false,
                } =>
            {
                degraded.push(DegradedSession { id, error });
            }
            SessionRow::Readable(_) | SessionRow::Corrupt { .. } => {}
        }
    }
    Ok((sessions, degraded))
}

/// What the slow providers said about one session before the write lock was
/// taken; valid only for the pid, cwd and pane it was gathered for.
struct Probe {
    pid: Option<(u32, u64)>,
    process_alive: Option<bool>,
    cwd: String,
    worktree_exists: bool,
    tmux_pane: Option<String>,
}

impl Probe {
    fn gather(env: &Env, session: &Session, boot_id: Option<&str>) -> Self {
        let boot_changed = matches!(
            (session.boot_id.as_deref(), boot_id),
            (Some(recorded), Some(current)) if recorded != current
        );
        let pid = session.pid.zip(session.pid_start);
        let process_alive = match pid {
            Some((pid, start)) if !boot_changed => Some(env.processes.is_alive(pid, start)),
            _ => None,
        };
        Self {
            pid,
            process_alive,
            cwd: session.cwd.clone(),
            worktree_exists: session.cwd.is_empty() || env.fs.exists(Path::new(&session.cwd)),
            tmux_pane: session.tmux_pane.clone(),
        }
    }

    /// The pane listing was taken for the pane the snapshot recorded; a hook
    /// may have recorded another since, which the listing cannot vouch for.
    fn panes_for<'p>(
        &self,
        session: &Session,
        panes: Option<&'p HashMap<String, PaneLocation>>,
    ) -> Option<&'p HashMap<String, PaneLocation>> {
        if self.tmux_pane == session.tmux_pane {
            panes
        } else {
            None
        }
    }

    /// A hook may have recorded another pid since; then nothing is known.
    fn process_alive_for(&self, session: &Session) -> Option<bool> {
        (self.pid == session.pid.zip(session.pid_start))
            .then_some(self.process_alive)
            .flatten()
    }

    /// A changed cwd keeps the stored value until the next pass.
    fn worktree_exists_for(&self, session: &Session) -> bool {
        if self.cwd == session.cwd {
            self.worktree_exists
        } else {
            session.worktree_exists
        }
    }
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
    session: &Session,
    boot_id: Option<&'a str>,
    panes: Option<&HashMap<String, PaneLocation>>,
    probe: &Probe,
) -> Observation<'a> {
    let process_alive = probe.process_alive_for(session);
    let pane_exists = match (&session.tmux_pane, probe.panes_for(session, panes)) {
        (Some(pane), Some(panes)) => Some(panes.contains_key(pane)),
        _ => None,
    };
    Observation {
        boot_id,
        process_alive,
        pane_exists,
        registry_missing: false,
    }
}

/// Only Claude writes the registry, and a session whose last state change is
/// within the grace period may simply not have its file yet.
fn missing_from_registry(session: &Session, live_ids: &LiveRegistryIds, now: &str) -> bool {
    if session.agent_kind != ClaudeKind.id() || live_ids.contains(&session.agent_session_id) {
        return false;
    }
    let (Ok(since), Ok(now)) = (
        chrono::DateTime::parse_from_rfc3339(&session.state_since),
        chrono::DateTime::parse_from_rfc3339(now),
    ) else {
        return false;
    };
    now.signed_duration_since(since).num_seconds() >= REGISTRY_GRACE_SECS
}

enum Outcome {
    Unchanged,
    Interrupted,
    /// Only `worktree_exists` changed.
    Refreshed,
}

fn apply(
    store: &Store,
    mut session: Session,
    observation: &Observation<'_>,
    probe: &Probe,
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
    let worktree_exists = probe.worktree_exists_for(&session);
    let worktree_changed = worktree_exists != session.worktree_exists;
    session.worktree_exists = worktree_exists;
    // A pane id only names this session's pane while the session is live: after
    // a reboot tmux reuses the ids. A vanished pane leaves the old placement.
    let panes = probe.panes_for(&session, panes);
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
    use std::sync::Arc;

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
            registry_missing: false,
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
            registry_missing: true,
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
            registry_missing: true,
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
            registry_missing: false,
        };
        assert_eq!(decide(&s, &observation), None);
    }

    #[test]
    fn a_missing_pane_counts_only_for_a_session_with_no_pid() {
        let gone = Observation {
            boot_id: Some("boot-1"),
            process_alive: Some(true),
            pane_exists: Some(false),
            registry_missing: false,
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
    fn a_missing_registry_entry_interrupts_only_a_session_with_no_pid() {
        let missing = Observation {
            boot_id: Some("boot-1"),
            process_alive: None,
            pane_exists: Some(true),
            registry_missing: true,
        };
        let mut no_pid = session(State::Idle);
        no_pid.pid = None;
        no_pid.pid_start = None;
        assert_eq!(decide(&no_pid, &missing), Some(Reason::RegistryMissing));
        assert_eq!(
            decide(
                &no_pid,
                &Observation {
                    registry_missing: false,
                    ..missing
                }
            ),
            None
        );
        assert_eq!(decide(&session(State::Idle), &missing), None);
        let mut done = no_pid.clone();
        done.state = State::Finished;
        assert_eq!(decide(&done, &missing), None);
    }

    #[test]
    fn a_gone_pane_is_reported_before_a_missing_registry_entry() {
        let gone = Observation {
            boot_id: Some("boot-1"),
            process_alive: None,
            pane_exists: Some(false),
            registry_missing: true,
        };
        let mut s = session(State::Idle);
        s.pid = None;
        s.pid_start = None;
        s.tmux_pane = Some("%1".to_owned());
        assert_eq!(decide(&s, &gone), Some(Reason::PaneGone));
    }

    /// Runs `during` the first time the pass probes a process, i.e. before it
    /// holds the write lock.
    struct HookedProcesses {
        inner: Arc<crate::testing::FakeProcesses>,
        during: Box<dyn Fn() + Send + Sync>,
    }

    impl crate::env::Processes for HookedProcesses {
        fn is_alive(&self, pid: u32, start_time: u64) -> bool {
            (self.during)();
            self.inner.is_alive(pid, start_time)
        }

        fn start_time(&self, pid: u32) -> Result<Option<u64>, crate::env::EnvError> {
            self.inner.start_time(pid)
        }
    }

    #[test]
    fn a_change_landing_during_the_probes_is_not_reverted() {
        use crate::store::StoreOptions;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
        let other_dir = dir.path().to_owned();
        let mut idle = session(State::Idle);
        idle.cwd = "/gone".to_owned();
        store.upsert_session(&idle).unwrap();

        let fake = crate::testing::FakeEnv::new();
        fake.processes.spawn(10, 5);
        let mut env = fake.env.clone();
        env.processes = Arc::new(HookedProcesses {
            inner: fake.processes.clone(),
            during: Box::new(move || {
                let other = Store::open_in_dir(&other_dir, &StoreOptions::default()).unwrap();
                let mut working = other.get_session("s").unwrap().unwrap();
                if working.state != State::Working {
                    working.state = State::Working;
                    other.upsert_session(&working).unwrap();
                }
            }),
        });

        let report =
            reconcile_with_registry(&store, &env, Scope::All, "2026-10-12T01:00:00Z", None)
                .unwrap();

        let after = store.get_session("s").unwrap().unwrap();
        assert_eq!(after.state, State::Working, "{report:?}");
        assert!(!after.worktree_exists);
    }

    /// Runs `during` every time the pass stats a path, which happens after the
    /// pane listing and before the write lock.
    struct HookedFs {
        inner: Arc<dyn crate::env::Fs>,
        during: Box<dyn Fn() + Send + Sync>,
    }

    impl crate::env::Fs for HookedFs {
        fn read_to_string(&self, path: &Path) -> Result<String, crate::env::EnvError> {
            self.inner.read_to_string(path)
        }

        fn exists(&self, path: &Path) -> bool {
            (self.during)();
            self.inner.exists(path)
        }

        fn list_dir(&self, path: &Path) -> Result<Vec<std::path::PathBuf>, crate::env::EnvError> {
            self.inner.list_dir(path)
        }

        fn read_link(&self, path: &Path) -> Result<std::path::PathBuf, crate::env::EnvError> {
            self.inner.read_link(path)
        }

        fn metadata(&self, path: &Path) -> Result<crate::env::FileMeta, crate::env::EnvError> {
            self.inner.metadata(path)
        }

        fn read_range(
            &self,
            path: &Path,
            offset: u64,
            max_len: usize,
        ) -> Result<Vec<u8>, crate::env::EnvError> {
            self.inner.read_range(path, offset, max_len)
        }
    }

    #[test]
    fn a_pane_recorded_after_the_listing_is_not_judged_gone() {
        use crate::store::StoreOptions;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
        let other_dir = dir.path().to_owned();
        let mut bare = session(State::Idle);
        bare.pid = None;
        bare.pid_start = None;
        store.upsert_session(&bare).unwrap();
        let mut paned = session(State::Idle);
        paned.id = "t".to_owned();
        paned.agent_session_id = "t".to_owned();
        paned.pid = None;
        paned.pid_start = None;
        paned.tmux_pane = Some("%1".to_owned());
        store.upsert_session(&paned).unwrap();

        let fake = crate::testing::FakeEnv::new();
        fake.tmux.add_pane(
            "%1",
            PaneLocation {
                session_name: "main".to_owned(),
                window_name: "w".to_owned(),
                window_index: "0".to_owned(),
                pane_index: "0".to_owned(),
                window_panes: "1".to_owned(),
                window_layout: String::new(),
            },
        );
        let mut env = fake.env.clone();
        env.fs = Arc::new(HookedFs {
            inner: fake.env.fs.clone(),
            during: Box::new(move || {
                let other = Store::open_in_dir(&other_dir, &StoreOptions::default()).unwrap();
                let mut s = other.get_session("s").unwrap().unwrap();
                if s.tmux_pane.is_none() {
                    s.tmux_pane = Some("%9".to_owned());
                    other.upsert_session(&s).unwrap();
                }
            }),
        });

        reconcile_with_registry(&store, &env, Scope::All, "2026-10-12T01:00:00Z", None).unwrap();

        let after = store.get_session("s").unwrap().unwrap();
        assert_eq!(after.state, State::Idle);
        assert_eq!(after.tmux_pane.as_deref(), Some("%9"));
    }

    #[test]
    fn reasons_have_stable_names() {
        assert_eq!(Reason::RegistryMissing.as_str(), "registry_missing");
        assert_eq!(Reason::ProcessGone.as_str(), "process_gone");
        assert_eq!(Reason::BootChanged.as_str(), "boot_changed");
        assert_eq!(Reason::PaneGone.as_str(), "pane_gone");
    }
}
