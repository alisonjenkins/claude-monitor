//! Liveness reconciliation: marks sessions whose agent process, tmux pane or
//! machine boot is gone as interrupted (FR-005..FR-008).

use crate::env::Env;
use crate::store::sessions::SessionsError;
use crate::store::Store;

/// Which sessions a reconcile pass looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Pane(String),
    Session(String),
}

/// A row that could not be read or updated; reconcile skips it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DegradedSession {
    pub id: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub checked: usize,
    pub interrupted: usize,
    pub revived: usize,
    pub unchanged: usize,
    pub degraded: Vec<DegradedSession>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("cannot reconcile: {0}")]
    Sessions(#[from] SessionsError),
}

pub fn reconcile(
    _store: &Store,
    _env: &Env,
    _scope: Scope,
    _now: &str,
) -> Result<ReconcileReport, ReconcileError> {
    Ok(ReconcileReport::default())
}
