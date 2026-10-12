//! One-time import of the old dashboard's per-session runtime JSON files
//! (`session::status_dir()`) into the store (FR-022).
//!
//! The files are only read, never deleted: the hook still dual-writes them for
//! the old dashboard. A file whose session is already in the store is skipped,
//! so a re-run changes nothing. An `events` row (`store.legacy_imported`) marks
//! the import as done for [`migrate_if_needed`]; there is no schema change.
//! Liveness of an imported session is unknown, so no pid is recorded and the
//! `session.created` event carries `imported: true` for reconcile to act on.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::SecondsFormat;
use rusqlite::{Transaction, TransactionBehavior};
use serde_json::{json, Value};

use super::events::{EventsError, NewEvent};
use super::sessions::SessionsError;
use super::Store;
use crate::env::Clock;
use crate::model::{Author, Session, State};
use crate::session::{is_valid_session_id, SessionState, SessionStatus};

pub const MARKER_ENTITY_KIND: &str = "store";
pub const MARKER_ENTITY_ID: &str = "legacy-runtime";
pub const MARKER_EVENT_KIND: &str = "store.legacy_imported";

const LEGACY_AGENT_KIND: &str = "claude";
const COMPONENT: &str = "migrate";
const SESSION_ENTITY_KIND: &str = "session";
const LEGACY_FILE_SUFFIX: &str = ".json";

#[derive(Debug, thiserror::Error)]
pub enum LegacyError {
    #[error("cannot list legacy runtime directory {path}: {source}")]
    ReadDir { path: PathBuf, source: io::Error },
    #[error("cannot check for the legacy import marker: {0}")]
    MarkerCheck(rusqlite::Error),
    #[error("cannot begin legacy import transaction: {0}")]
    Begin(rusqlite::Error),
    #[error("cannot commit legacy import transaction: {0}")]
    Commit(rusqlite::Error),
    #[error("{0}")]
    Sessions(#[from] SessionsError),
    #[error("{0}")]
    Events(#[from] EventsError),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrateReport {
    pub imported: u32,
    pub skipped_existing: u32,
    pub skipped_corrupt: u32,
    /// File names only, never paths or contents.
    pub corrupt_files: Vec<String>,
}

impl MigrateReport {
    /// Files that were looked at, usable or not.
    pub fn candidates(&self) -> u32 {
        self.imported
            .saturating_add(self.skipped_existing)
            .saturating_add(self.skipped_corrupt)
    }

    /// There were files, and none of them could be read.
    pub fn all_failed(&self) -> bool {
        self.skipped_corrupt > 0 && self.skipped_corrupt == self.candidates()
    }
}

/// True once a completed import has recorded its marker event.
pub fn is_migrated(store: &Store) -> Result<bool, LegacyError> {
    store
        .connection()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE entity_kind = ?1 AND entity_id = ?2 AND kind = ?3)",
            rusqlite::params![MARKER_ENTITY_KIND, MARKER_ENTITY_ID, MARKER_EVENT_KIND],
            |row| row.get(0),
        )
        .map_err(LegacyError::MarkerCheck)
}

/// Imports every readable legacy file whose session is not yet in the store,
/// and records the marker if none exists. Safe to re-run: it then reports the
/// files as `skipped_existing` and appends nothing.
pub fn migrate(
    store: &Store,
    runtime_dir: &Path,
    clock: &dyn Clock,
) -> Result<MigrateReport, LegacyError> {
    let candidates = scan(runtime_dir)?;
    let now = clock.now();
    let mut report = MigrateReport::default();

    // The guard only issues BEGIN IMMEDIATE / COMMIT; the Store methods below
    // run on the same connection, so they are inside the transaction.
    let tx = Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(LegacyError::Begin)?;
    for candidate in candidates {
        match candidate {
            Candidate::Corrupt { file_name } => {
                report.skipped_corrupt = report.skipped_corrupt.saturating_add(1);
                report.corrupt_files.push(file_name);
            }
            Candidate::Status(status) => {
                if store
                    .get_session_by_agent_id(LEGACY_AGENT_KIND, &status.session_id)?
                    .is_some()
                {
                    report.skipped_existing = report.skipped_existing.saturating_add(1);
                } else {
                    import(store, &status, &now)?;
                    report.imported = report.imported.saturating_add(1);
                }
            }
        }
    }
    if !is_migrated(store)? {
        append_marker(store, &report, &now)?;
    }
    tx.commit().map_err(LegacyError::Commit)?;
    Ok(report)
}

/// Runs [`migrate`] unless the marker says it already ran; `None` when skipped.
pub fn migrate_if_needed(
    store: &Store,
    runtime_dir: &Path,
    clock: &dyn Clock,
) -> Result<Option<MigrateReport>, LegacyError> {
    if is_migrated(store)? {
        return Ok(None);
    }
    migrate(store, runtime_dir, clock).map(Some)
}

enum Candidate {
    Status(SessionStatus),
    Corrupt { file_name: String },
}

/// Reads every `*.json` file in `dir`, sorted by name. A missing directory is
/// empty; an unreadable or invalid file is a `Corrupt` candidate.
fn scan(dir: &Path) -> Result<Vec<Candidate>, LegacyError> {
    let read_dir_error = |source| LegacyError::ReadDir {
        path: dir.to_owned(),
        source,
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(read_dir_error(e)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_dir_error)?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map_err(read_dir_error)?.is_dir();
        if file_name.ends_with(LEGACY_FILE_SUFFIX) && !is_dir {
            files.push((file_name, entry.path()));
        }
    }
    files.sort();
    Ok(files
        .into_iter()
        .map(|(file_name, path)| match read_status(&file_name, &path) {
            Some(status) => Candidate::Status(status),
            None => Candidate::Corrupt { file_name },
        })
        .collect())
}

/// The session id is later used verbatim as a file name, so it must be safe and
/// must match the file it came from.
fn read_status(file_name: &str, path: &Path) -> Option<SessionStatus> {
    let stem = file_name.strip_suffix(LEGACY_FILE_SUFFIX)?;
    let status: SessionStatus = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    (is_valid_session_id(stem) && status.session_id == stem).then_some(status)
}

fn map_state(state: SessionState) -> State {
    match state {
        SessionState::Working => State::Working,
        SessionState::NeedsPermission => State::NeedsPermission,
        SessionState::Idle => State::Idle,
    }
}

fn import(store: &Store, status: &SessionStatus, now: &str) -> Result<(), LegacyError> {
    let since = status.since.to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut session = Session::new(
        &uuid::Uuid::new_v4().to_string(),
        LEGACY_AGENT_KIND,
        &status.session_id,
        &status.cwd,
        &since,
    );
    session.state = map_state(status.state);
    session.tmux_pane.clone_from(&status.tmux_pane);
    store.upsert_session(&session)?;

    let payload = json!({
        "agent_kind": session.agent_kind,
        "agent_session_id": session.agent_session_id,
        "state": session.state.as_str(),
        "cwd": session.cwd,
        "tmux_pane": session.tmux_pane,
        "imported": true,
    });
    append(
        store,
        now,
        SESSION_ENTITY_KIND,
        &session.id,
        "session.created",
        &payload,
    )
}

fn append_marker(store: &Store, report: &MigrateReport, now: &str) -> Result<(), LegacyError> {
    let payload = json!({
        "imported": report.imported,
        "skipped_existing": report.skipped_existing,
        "skipped_corrupt": report.skipped_corrupt,
    });
    append(
        store,
        now,
        MARKER_ENTITY_KIND,
        MARKER_ENTITY_ID,
        MARKER_EVENT_KIND,
        &payload,
    )
}

fn append(
    store: &Store,
    at: &str,
    entity_kind: &str,
    entity_id: &str,
    kind: &str,
    payload: &Value,
) -> Result<(), LegacyError> {
    let author = Author::System {
        component: COMPONENT.to_owned(),
    };
    store.append_event(&NewEvent {
        at,
        entity_kind,
        entity_id,
        kind,
        payload,
        author: &author,
    })?;
    Ok(())
}
