//! Recording what a passive scan found, shared by every agent kind.
//!
//! A new session is stored as `unknown` plus a `session.discovered` event; a
//! known session only gets missing fields filled, so a session a hook reported
//! keeps its hook-derived state and times.

use std::path::Path;

use rusqlite::{Transaction, TransactionBehavior};
use serde_json::json;

use super::{DiscoverError, ScanReport, SessionStub};
use crate::env::Env;
use crate::hook::GitProbe;
use crate::model::{Author, Session, State};
use crate::store::events::{EventsError, NewEvent};
use crate::store::sessions::{SessionRow, SessionsError};
use crate::store::Store;

const DISCOVER_COMPONENT: &str = "discover";
const ENTITY_SESSION: &str = "session";
const EVENT_DISCOVERED: &str = "session.discovered";

#[derive(Debug, thiserror::Error)]
pub enum DiscoverStoreError {
    #[error("{0}")]
    Scan(#[from] DiscoverError),
    #[error("{0}")]
    Sessions(#[from] SessionsError),
    #[error("{0}")]
    Events(#[from] EventsError),
    #[error("cannot begin discovery write transaction: {0}")]
    Begin(rusqlite::Error),
    #[error("cannot commit discovery write transaction: {0}")]
    Commit(rusqlite::Error),
}

/// What one discovery run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscoverSummary {
    pub created: usize,
    pub updated: usize,
    pub degraded: usize,
}

/// Records each stub of `report` as a session of agent kind `kind`.
pub fn store_report(
    env: &Env,
    store: &Store,
    kind: &str,
    report: &ScanReport,
    git: &dyn GitProbe,
) -> Result<DiscoverSummary, DiscoverStoreError> {
    let author = Author::System {
        component: DISCOVER_COMPONENT.to_owned(),
    };
    let mut summary = DiscoverSummary {
        degraded: report.degraded,
        ..DiscoverSummary::default()
    };
    let now = env.clock.now();
    // The guard only issues BEGIN IMMEDIATE / COMMIT; the Store methods below
    // run on the same connection, so they are inside the transaction.
    let tx = Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(DiscoverStoreError::Begin)?;
    for stub in &report.stubs {
        match store.get_session_by_agent_id(kind, &stub.agent_session_id)? {
            None => {
                let session = new_session(kind, stub);
                store.upsert_session(&session)?;
                let payload = json!({
                    "agent_kind": session.agent_kind,
                    "agent_session_id": session.agent_session_id,
                    "state": session.state.as_str(),
                    "cwd": session.cwd,
                    "branch": session.branch,
                });
                store.append_event(&NewEvent {
                    at: &now,
                    entity_kind: ENTITY_SESSION,
                    entity_id: &session.id,
                    kind: EVENT_DISCOVERED,
                    payload: &payload,
                    author: &author,
                })?;
                summary.created = summary.created.saturating_add(1);
            }
            Some(existing) => {
                if let Some(filled) = fill_missing(&existing, stub) {
                    store.upsert_session(&filled)?;
                    summary.updated = summary.updated.saturating_add(1);
                }
            }
        }
    }
    probe_unhooked(store, kind, git)?;
    tx.commit().map_err(DiscoverStoreError::Commit)?;
    Ok(summary)
}

/// Reads the live git HEAD for every `kind` session with no `repo_root`: only a
/// hook records one, so these rows hold at best a transcript branch, which goes
/// stale. A row is rewritten only when the probe disagrees with it.
fn probe_unhooked(store: &Store, kind: &str, git: &dyn GitProbe) -> Result<(), DiscoverStoreError> {
    for row in store.list_sessions_lenient()? {
        let SessionRow::Readable(session) = row else {
            continue;
        };
        if session.agent_kind != kind || session.repo_root.is_some() {
            continue;
        }
        // The probe walks up from `cwd`, so a deleted directory would report
        // whatever repository happens to contain its path.
        if !Path::new(&session.cwd).is_dir() {
            continue;
        }
        let Some(info) = git.probe(&session.cwd) else {
            continue;
        };
        let mut probed = (*session).clone();
        probed.repo_root = Some(info.root);
        probed.branch = info.branch;
        if probed != *session {
            store.upsert_session(&probed)?;
        }
    }
    Ok(())
}

fn new_session(kind: &str, stub: &SessionStub) -> Session {
    let mut session = Session::new(
        &uuid::Uuid::new_v4().to_string(),
        kind,
        &stub.agent_session_id,
        &stub.cwd,
        &stub.started_at,
    );
    session.repo_root.clone_from(&stub.repo_root);
    session.branch.clone_from(&stub.branch);
    session.task_description.clone_from(&stub.first_prompt);
    session.last_activity_at.clone_from(&stub.last_activity_at);
    session
}

/// The session with the stub's data added where the session has none, or `None`
/// when nothing would change. Never touches state or hook-derived times; a
/// branch is only taken from a transcript recorded in the same working directory,
/// and never for a row that has a `repo_root` (git facts come from a probe then).
fn fill_missing(existing: &Session, stub: &SessionStub) -> Option<Session> {
    let mut session = existing.clone();
    if session.repo_root.is_none() && session.cwd == stub.cwd {
        session.repo_root.clone_from(&stub.repo_root);
    }
    if existing.repo_root.is_none() && session.branch.is_none() && session.cwd == stub.cwd {
        session.branch.clone_from(&stub.branch);
    }
    if session.task_description.is_none() {
        session.task_description.clone_from(&stub.first_prompt);
    }
    if session.state == State::Unknown && session.last_activity_at < stub.last_activity_at {
        session.last_activity_at.clone_from(&stub.last_activity_at);
    }
    (session != *existing).then_some(session)
}
