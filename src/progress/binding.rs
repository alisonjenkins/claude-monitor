//! Which plan file a session's progress comes from (FR-015, FR-015a, FR-016).
//!
//! A session is bound automatically only when exactly one plan is clearly the
//! one it works from (its branch's spec-kit directory, or the spec-kit feature
//! the repository points at); anything else is a pick-one result and shows no
//! figure until the user chooses. The choice is stored and kept.

use std::path::PathBuf;

use serde::Serialize;

use super::{Progress, SourceKind};
use crate::env::Env;
use crate::model::{Author, Session};
use crate::store::events::EventsError;
use crate::store::progress::{Binding, ProgressStoreError};
use crate::store::Store;

#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error("{0}")]
    Store(#[from] ProgressStoreError),
    #[error("{0}")]
    Events(#[from] EventsError),
    #[error("cannot read {}: {detail}", path.display())]
    Unreadable { path: PathBuf, detail: String },
    #[error("{} holds no tasks or checklist items", path.display())]
    NotAPlan { path: PathBuf },
}

/// Why a file is offered for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateReason {
    /// The session's branch names the spec-kit directory.
    Branch,
    /// `.specify/feature.json` names the spec-kit directory.
    FeatureJson,
    /// A Markdown checklist in the repository root.
    Checklist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub path: PathBuf,
    pub kind: SourceKind,
    pub reason: CandidateReason,
    pub done: u32,
    pub total: u32,
}

/// What the user picks: a plan file, or no plan at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Path(PathBuf),
    None,
}

/// A bound session's progress. `progress` is the last figure read; `stale`
/// means the source cannot be read now, so the figure may be out of date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub binding: Binding,
    pub progress: Option<Progress>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressView {
    /// No plan can be derived: no figure (FR-016).
    None,
    /// Several candidates and none is clearly the one: no figure, ask the user.
    PickOne(Vec<Candidate>),
    Bound(Box<Bound>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// A binding was made or moved by this call.
    pub bound: bool,
    /// The stored steps changed.
    pub changed: bool,
    pub stale: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub bound: usize,
    pub changed: usize,
    pub warnings: Vec<String>,
}

pub fn discover_candidates(session: &Session, env: &Env) -> Vec<Candidate> {
    let _ = (session, env);
    Vec::new()
}

/// The candidate the session may be bound to without asking, if any.
pub fn auto_choice(candidates: &[Candidate]) -> Option<&Candidate> {
    let _ = candidates;
    None
}

pub fn sync_session(
    store: &Store,
    env: &Env,
    session: &Session,
    now: &str,
) -> Result<SyncOutcome, BindingError> {
    let _ = (store, env, session, now);
    Ok(SyncOutcome::default())
}

pub fn view(store: &Store, env: &Env, session: &Session) -> Result<ProgressView, BindingError> {
    let _ = (store, env, session);
    Ok(ProgressView::None)
}

pub fn choose(
    store: &Store,
    env: &Env,
    session: &Session,
    choice: &Choice,
    author: &Author,
    now: &str,
) -> Result<Binding, BindingError> {
    let _ = (store, env, session, choice, author, now);
    Err(BindingError::NotAPlan {
        path: PathBuf::new(),
    })
}

pub fn sync_all(store: &Store, env: &Env, now: &str) -> SyncSummary {
    let _ = (store, env, now);
    SyncSummary::default()
}
