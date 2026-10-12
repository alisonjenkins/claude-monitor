//! One refresh pass: everything the monitor can learn without a hook.
//!
//! Passive discovery of Claude and Copilot sessions, then the Claude live
//! session registry (pid, tmux pane, busy/waiting/idle), then reconcile. Only
//! reads the agents' own directories; every change is an `events` row.

use rusqlite::{Transaction, TransactionBehavior};
use serde_json::json;

use crate::agents::claude::ClaudeKind;
use crate::agents::claude_registry::{self, RegistryEntry};
use crate::agents::discover_store::{DiscoverStoreError, DiscoverSummary};
use crate::agents::{claude_discover, copilot_discover, AgentKind};
use crate::env::Env;
use crate::model::{Author, Session, State};
use crate::reconcile::{reconcile, ReconcileReport, Scope};
use crate::store::events::{EventsError, NewEvent};
use crate::store::sessions::SessionsError;
use crate::store::Store;

const REGISTRY_COMPONENT: &str = "registry";
const ENTITY_SESSION: &str = "session";
const REASON_REGISTRY: &str = "registry";

/// What one [`refresh`] did. Sources that failed are listed in `warnings`;
/// they never fail the pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub discovered: DiscoverSummary,
    /// Sessions created from a live registry entry no transcript had yet.
    pub registry_created: usize,
    /// Known sessions whose pid, pane or state the registry changed.
    pub registry_updated: usize,
    pub reconcile: Option<ReconcileReport>,
    pub warnings: Vec<String>,
}

impl RefreshReport {
    /// Whether the store may differ from before the pass.
    pub fn changed(&self) -> bool {
        self.discovered.created > 0
            || self.discovered.updated > 0
            || self.registry_created > 0
            || self.registry_updated > 0
            || self
                .reconcile
                .as_ref()
                .is_some_and(|r| r.interrupted > 0 || r.unchanged < r.checked)
    }
}

#[derive(Debug, thiserror::Error)]
enum SyncError {
    #[error("{0}")]
    Sessions(#[from] SessionsError),
    #[error("{0}")]
    Events(#[from] EventsError),
}

/// Discover, read the registry, reconcile. Idempotent: a second pass over the
/// same facts writes nothing.
pub fn refresh(store: &Store, env: &Env, now: &str) -> RefreshReport {
    let mut report = RefreshReport::default();
    discover(store, env, &mut report);
    sync_registry(store, env, now, &mut report);
    match reconcile(store, env, Scope::All, now) {
        Ok(done) => report.reconcile = Some(done),
        Err(e) => report.warnings.push(format!("reconcile: {e}")),
    }
    report
}

type DiscoverFn = fn(&Env, &Store) -> Result<DiscoverSummary, DiscoverStoreError>;

fn discover(store: &Store, env: &Env, report: &mut RefreshReport) {
    let sources: [(&str, DiscoverFn); 2] = [
        ("claude", claude_discover::discover_and_store),
        ("copilot", copilot_discover::discover_and_store),
    ];
    for (label, run) in sources {
        match run(env, store) {
            Ok(summary) => {
                let total = &mut report.discovered;
                total.created = total.created.saturating_add(summary.created);
                total.updated = total.updated.saturating_add(summary.updated);
                total.degraded = total.degraded.saturating_add(summary.degraded);
            }
            Err(e) => report.warnings.push(format!("{label} discovery: {e}")),
        }
    }
}

fn sync_registry(store: &Store, env: &Env, now: &str, report: &mut RefreshReport) {
    let registry = match claude_registry::scan(env) {
        Ok(registry) => registry,
        Err(e) => {
            report.warnings.push(format!("claude registry: {e}"));
            return;
        }
    };
    let live = claude_registry::live_by_session_id(&registry);
    if live.is_empty() {
        return;
    }
    let mut entries: Vec<&RegistryEntry> = live.into_values().collect();
    entries.sort_by_key(|entry| entry.pid);
    let boot_id = env.boot_id.current().ok();
    let tx = match Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => {
            report
                .warnings
                .push(format!("claude registry: cannot begin transaction: {e}"));
            return;
        }
    };
    for entry in entries {
        match sync_entry(store, env, entry, boot_id.as_deref(), now) {
            Ok(Change::None) => {}
            Ok(Change::Created) => {
                report.registry_created = report.registry_created.saturating_add(1);
            }
            Ok(Change::Updated) => {
                report.registry_updated = report.registry_updated.saturating_add(1);
            }
            Err(e) => report.warnings.push(format!(
                "claude registry: session {}: {e}",
                entry.session_id
            )),
        }
    }
    if let Err(e) = tx.commit() {
        report
            .warnings
            .push(format!("claude registry: cannot commit: {e}"));
    }
}

enum Change {
    None,
    Created,
    Updated,
}

/// What `apply_entry` changed on a session.
#[derive(Default)]
struct Applied {
    from_state: Option<State>,
    process_changed: bool,
}

fn registry_author() -> Author {
    Author::System {
        component: REGISTRY_COMPONENT.to_owned(),
    }
}

/// Finished sessions are over and interrupted ones only a hook revives.
fn syncable(state: State) -> bool {
    state == State::Unknown || state.is_active()
}

fn sync_entry(
    store: &Store,
    env: &Env,
    entry: &RegistryEntry,
    boot_id: Option<&str>,
    now: &str,
) -> Result<Change, SyncError> {
    let Some(pid_start) = env.processes.start_time(entry.pid).ok().flatten() else {
        return Ok(Change::None);
    };
    let author = registry_author();
    let kind = ClaudeKind.id();
    let Some(existing) = store.get_session_by_agent_id(kind, &entry.session_id)? else {
        let cwd = entry
            .cwd
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let started_at = entry
            .started_at_ms
            .and_then(iso_from_millis)
            .unwrap_or_else(|| now.to_owned());
        let mut session = Session::new(
            &uuid::Uuid::new_v4().to_string(),
            kind,
            &entry.session_id,
            &cwd,
            &started_at,
        );
        apply_entry(&mut session, entry, pid_start, boot_id, now);
        store.upsert_session(&session)?;
        let payload = json!({
            "agent_kind": session.agent_kind,
            "agent_session_id": session.agent_session_id,
            "state": session.state.as_str(),
            "cwd": session.cwd,
            "pid": session.pid,
            "tmux_pane": session.tmux_pane,
        });
        store.append_event(&NewEvent {
            at: now,
            entity_kind: ENTITY_SESSION,
            entity_id: &session.id,
            kind: "session.discovered",
            payload: &payload,
            author: &author,
        })?;
        return Ok(Change::Created);
    };
    if !syncable(existing.state) {
        return Ok(Change::None);
    }
    let mut session = existing.clone();
    let applied = apply_entry(&mut session, entry, pid_start, boot_id, now);
    if session == existing {
        return Ok(Change::None);
    }
    store.upsert_session(&session)?;
    if let Some(from) = applied.from_state {
        let payload = json!({
            "state": session.state.as_str(),
            "from": from.as_str(),
            "reason": REASON_REGISTRY,
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
    if applied.process_changed {
        let payload = json!({
            "pid": session.pid,
            "pid_start": session.pid_start,
            "tmux_pane": session.tmux_pane,
            "boot_id": session.boot_id,
        });
        store.append_event(&NewEvent {
            at: now,
            entity_kind: ENTITY_SESSION,
            entity_id: &session.id,
            kind: "session.process_changed",
            payload: &payload,
            author: &author,
        })?;
    }
    Ok(Change::Updated)
}

/// Copies the registry's view of a live session onto `session`.
fn apply_entry(
    session: &mut Session,
    entry: &RegistryEntry,
    pid_start: u64,
    boot_id: Option<&str>,
    now: &str,
) -> Applied {
    let mut applied = Applied::default();
    if session.pid != Some(entry.pid) || session.pid_start != Some(pid_start) {
        session.pid = Some(entry.pid);
        session.pid_start = Some(pid_start);
        applied.process_changed = true;
    }
    if let Some(boot_id) = boot_id {
        if session.boot_id.as_deref() != Some(boot_id) {
            session.boot_id = Some(boot_id.to_owned());
            applied.process_changed = true;
        }
    }
    if let Some(tmux) = &entry.tmux {
        if session.tmux_pane.as_deref() != Some(tmux.pane_id.as_str()) {
            session.tmux_pane = Some(tmux.pane_id.clone());
            applied.process_changed = true;
        }
    }
    if let Some(status) = &entry.status {
        let next = claude_registry::to_state(status);
        if session.state != next {
            applied.from_state = Some(session.state);
            session.state = next;
            now.clone_into(&mut session.state_since);
        }
    }
    applied
}

fn iso_from_millis(ms: u64) -> Option<String> {
    let ms = i64::try_from(ms).ok()?;
    let at = chrono::DateTime::from_timestamp_millis(ms)?;
    Some(at.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}
