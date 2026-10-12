//! Which plan file a session's progress comes from (FR-015, FR-015a, FR-016).
//!
//! A session is bound automatically only when exactly one plan is clearly the
//! one it works from (its branch's spec-kit directory, or the spec-kit feature
//! the repository points at); anything else is a pick-one result and shows no
//! figure until the user chooses. The choice is stored and kept.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use super::{checklist, speckit, Progress, Source, SourceKind};
use crate::env::Env;
use crate::model::{Author, OwnerKind, Session};
use crate::store::events::{EventsError, NewEvent};
use crate::store::progress::{Binding, ChosenBy, ProgressStoreError};
use crate::store::sessions::SessionRow;
use crate::store::Store;

/// A plan file larger than this is not a plan worth reading on every refresh.
const MAX_PLAN_BYTES: u64 = 1024 * 1024;
const SPECS_DIR: &str = "specs";
const TASKS_FILE: &str = "tasks.md";
const FEATURE_JSON: &str = ".specify/feature.json";
const FEATURE_DIRECTORY_FIELD: &str = "feature_directory";
const ENTITY_SESSION: &str = "session";
const EVENT_BOUND: &str = "session.progress_bound";
const COMPONENT: &str = "progress";

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

fn session_root(session: &Session) -> PathBuf {
    PathBuf::from(session.repo_root.as_deref().unwrap_or(&session.cwd))
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A spec-kit task list when the text has `T###` tasks, else a checklist.
fn detect(text: &str, path: Option<&str>) -> Option<Progress> {
    speckit::parse(text, path).or_else(|| checklist::parse(text, path))
}

fn parse_as(kind: SourceKind, text: &str, path: Option<&str>) -> Option<Progress> {
    match kind {
        SourceKind::SpeckitTasks => speckit::parse(text, path),
        SourceKind::MarkdownChecklist => checklist::parse(text, path),
        SourceKind::AgentTodo => None,
    }
}

fn read_plan(env: &Env, path: &Path) -> Option<Progress> {
    let meta = env.fs.metadata(path).ok()?;
    if meta.is_dir || meta.len > MAX_PLAN_BYTES {
        return None;
    }
    let text = env.fs.read_to_string(path).ok()?;
    detect(&text, None)
}

fn add_candidate(out: &mut Vec<Candidate>, env: &Env, path: PathBuf, reason: CandidateReason) {
    if out.iter().any(|c| c.path == path) {
        return;
    }
    if let Some(progress) = read_plan(env, &path) {
        out.push(Candidate {
            path,
            kind: progress.source.kind,
            reason,
            done: progress.done,
            total: progress.total,
        });
    }
}

fn feature_json_tasks(env: &Env, root: &Path) -> Option<PathBuf> {
    let text = env.fs.read_to_string(&root.join(FEATURE_JSON)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let dir = Path::new(value.get(FEATURE_DIRECTORY_FIELD)?.as_str()?);
    let dir = if dir.is_absolute() {
        dir.to_owned()
    } else {
        root.join(dir)
    };
    Some(dir.join(TASKS_FILE))
}

/// Leading digits of a branch's last segment when a `-` follows them.
fn numeric_prefix(segment: &str) -> Option<&str> {
    let digits = segment.bytes().take_while(u8::is_ascii_digit).count();
    let prefix = segment.get(..digits)?;
    (digits > 0 && segment.get(digits..)?.starts_with('-')).then_some(prefix)
}

fn branch_spec_dirs(env: &Env, root: &Path, branch: &str) -> Vec<PathBuf> {
    let segment = branch.rsplit('/').next().unwrap_or(branch);
    let prefix = numeric_prefix(segment).map(|p| format!("{p}-"));
    let Ok(entries) = env.fs.list_dir(&root.join(SPECS_DIR)) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            let matches = name == segment || prefix.as_deref().is_some_and(|p| name.starts_with(p));
            matches && env.fs.metadata(path).is_ok_and(|m| m.is_dir)
        })
        .collect()
}

fn root_markdown_files(env: &Env, root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = env.fs.list_dir(root) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        })
        .collect()
}

/// Plan files that could be this session's: the spec-kit tasks its branch or
/// `.specify/feature.json` names, then Markdown checklists in the root.
pub fn discover_candidates(session: &Session, env: &Env) -> Vec<Candidate> {
    let root = session_root(session);
    let mut out = Vec::new();
    if let Some(tasks) = feature_json_tasks(env, &root) {
        add_candidate(&mut out, env, tasks, CandidateReason::FeatureJson);
    }
    if let Some(branch) = session.branch.as_deref() {
        for dir in branch_spec_dirs(env, &root, branch) {
            add_candidate(&mut out, env, dir.join(TASKS_FILE), CandidateReason::Branch);
        }
    }
    for file in root_markdown_files(env, &root) {
        add_candidate(&mut out, env, file, CandidateReason::Checklist);
    }
    out
}

/// The candidate the session may be bound to without asking: the only spec-kit
/// match, or, when there is no such match, the only candidate at all.
pub fn auto_choice(candidates: &[Candidate]) -> Option<&Candidate> {
    let mut strong = candidates
        .iter()
        .filter(|c| c.reason != CandidateReason::Checklist);
    match (strong.next(), strong.next()) {
        (Some(only), None) => Some(only),
        (Some(_), Some(_)) => None,
        (None, _) => match candidates {
            [only] => Some(only),
            _ => None,
        },
    }
}

fn candidate_source(candidate: &Candidate) -> Source {
    Source {
        kind: candidate.kind,
        path: Some(candidate.path.display().to_string()),
    }
}

fn record_bound(
    store: &Store,
    session_id: &str,
    binding: &Binding,
    author: &Author,
    now: &str,
) -> Result<(), EventsError> {
    let payload = json!({
        "path": binding.source.path,
        "source_kind": binding.source.kind,
        "chosen_by": binding.chosen_by.as_str(),
    });
    store
        .append_event(&NewEvent {
            at: now,
            entity_kind: ENTITY_SESSION,
            entity_id: session_id,
            kind: EVENT_BOUND,
            payload: &payload,
            author,
        })
        .map(|_| ())
}

fn system_author() -> Author {
    Author::System {
        component: COMPONENT.to_owned(),
    }
}

/// Binds an unbound session to its clear candidate, and moves an automatic
/// binding to a spec-kit match that appeared later. A user's choice is never
/// touched. Returns the new binding when one was made.
fn auto_bind(
    store: &Store,
    env: &Env,
    session: &Session,
    current: Option<&Binding>,
    now: &str,
) -> Result<Option<Binding>, BindingError> {
    if current.is_some_and(|b| b.chosen_by == ChosenBy::User) {
        return Ok(None);
    }
    let candidates = discover_candidates(session, env);
    let Some(pick) = auto_choice(&candidates) else {
        return Ok(None);
    };
    let source = candidate_source(pick);
    if let Some(current) = current {
        if current.source == source || pick.reason == CandidateReason::Checklist {
            return Ok(None);
        }
    }
    let binding = store.bind_progress(
        OwnerKind::Session,
        &session.id,
        &source,
        ChosenBy::Auto,
        now,
    )?;
    record_bound(store, &session.id, &binding, &system_author(), now)?;
    Ok(Some(binding))
}

fn unchanged_since_read(env: &Env, path: &Path, read_at: &str) -> bool {
    let Some(modified) = env.fs.metadata(path).ok().and_then(|m| m.modified) else {
        return false;
    };
    let Ok(read) = DateTime::parse_from_rfc3339(read_at) else {
        return false;
    };
    let modified: DateTime<Utc> = modified.into();
    modified.timestamp() < read.timestamp()
}

/// Re-reads a binding's file into its steps. `Ok((changed, stale))`; an
/// unreadable file leaves the steps as they were and reports stale.
fn read_binding(
    store: &Store,
    env: &Env,
    binding: &Binding,
    now: &str,
    force: bool,
) -> Result<(bool, bool), BindingError> {
    let Some(path) = binding.source.path.as_deref().map(Path::new) else {
        return Ok((false, false));
    };
    if !force && unchanged_since_read(env, path, &binding.read_at) {
        return Ok((false, false));
    }
    let Ok(text) = env.fs.read_to_string(path) else {
        return Ok((false, true));
    };
    let items = parse_as(binding.source.kind, &text, None)
        .map(|p| p.items)
        .unwrap_or_default();
    let changed = store.replace_steps(&binding.id, &items)?;
    if binding.read_at.as_str() < now {
        store.touch_progress_binding(&binding.id, now)?;
    }
    Ok((changed, false))
}

fn sync_one(
    store: &Store,
    env: &Env,
    session: &Session,
    now: &str,
    discover: bool,
    force: bool,
) -> Result<SyncOutcome, BindingError> {
    let mut outcome = SyncOutcome::default();
    let mut binding = store.get_progress_binding(OwnerKind::Session, &session.id)?;
    let mut force = force;
    if discover {
        if let Some(bound) = auto_bind(store, env, session, binding.as_ref(), now)? {
            outcome.bound = true;
            binding = Some(bound);
            force = true;
        }
    }
    if let Some(binding) = binding {
        let (changed, stale) = read_binding(store, env, &binding, now, force)?;
        outcome.changed = changed;
        outcome.stale = stale;
    }
    Ok(outcome)
}

/// Binds the session when a plan is clearly its own and re-reads its bound
/// file, whatever the file's modification time.
pub fn sync_session(
    store: &Store,
    env: &Env,
    session: &Session,
    now: &str,
) -> Result<SyncOutcome, BindingError> {
    sync_one(store, env, session, now, true, true)
}

/// The refresh pass: every bound file modified since it was last read is
/// parsed again, and active sessions without a binding look for a plan.
pub fn sync_all(store: &Store, env: &Env, now: &str) -> SyncSummary {
    let mut summary = SyncSummary::default();
    let rows = match store.list_sessions_lenient() {
        Ok(rows) => rows,
        Err(e) => {
            summary
                .warnings
                .push(format!("progress: cannot list sessions: {e}"));
            return summary;
        }
    };
    // An unreadable row has no repo to look in; reconcile reports it.
    let sessions = rows.into_iter().filter_map(|row| match row {
        SessionRow::Readable(session) => Some(*session),
        SessionRow::Corrupt { .. } => None,
    });
    for session in sessions {
        let session = &session;
        match sync_one(store, env, session, now, session.state.is_active(), false) {
            Ok(outcome) => {
                summary.bound = summary.bound.saturating_add(usize::from(outcome.bound));
                summary.changed = summary.changed.saturating_add(usize::from(outcome.changed));
            }
            Err(e) => summary
                .warnings
                .push(format!("progress: session {}: {e}", session.id)),
        }
    }
    summary
}

fn source_readable(env: &Env, path: &str) -> bool {
    env.fs
        .metadata(Path::new(path))
        .is_ok_and(|meta| !meta.is_dir)
}

/// What to show for a session, from the store and the candidates on disk.
/// Writes nothing; run [`sync_session`] first to bind and refresh.
pub fn view(store: &Store, env: &Env, session: &Session) -> Result<ProgressView, BindingError> {
    let Some(binding) = store.get_progress_binding(OwnerKind::Session, &session.id)? else {
        let candidates = discover_candidates(session, env);
        return Ok(
            if auto_choice(&candidates).is_none() && !candidates.is_empty() {
                ProgressView::PickOne(candidates)
            } else {
                ProgressView::None
            },
        );
    };
    let root = session_root(session);
    let items = store.list_steps(&binding.id)?;
    let source = Source {
        kind: binding.source.kind,
        path: binding
            .source
            .path
            .as_deref()
            .map(|p| display_path(&root, Path::new(p))),
    };
    let stale = binding
        .source
        .path
        .as_deref()
        .is_some_and(|p| !source_readable(env, p));
    Ok(ProgressView::Bound(Box::new(Bound {
        progress: Progress::from_items(items, source),
        binding,
        stale,
    })))
}

/// Records the user's pick (a plan file, or no plan) and reads it. A file
/// that holds no tasks is refused and nothing changes.
pub fn choose(
    store: &Store,
    env: &Env,
    session: &Session,
    choice: &Choice,
    author: &Author,
    now: &str,
) -> Result<Binding, BindingError> {
    let (source, items) = match choice {
        Choice::None => (
            Source {
                kind: SourceKind::AgentTodo,
                path: None,
            },
            Vec::new(),
        ),
        Choice::Path(path) => {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                session_root(session).join(path)
            };
            let text = env
                .fs
                .read_to_string(&path)
                .map_err(|e| BindingError::Unreadable {
                    path: path.clone(),
                    detail: e.to_string(),
                })?;
            let progress =
                detect(&text, None).ok_or_else(|| BindingError::NotAPlan { path: path.clone() })?;
            (
                Source {
                    kind: progress.source.kind,
                    path: Some(path.display().to_string()),
                },
                progress.items,
            )
        }
    };
    let binding = store.bind_progress(
        OwnerKind::Session,
        &session.id,
        &source,
        ChosenBy::User,
        now,
    )?;
    store.replace_steps(&binding.id, &items)?;
    record_bound(store, &session.id, &binding, author, now)?;
    Ok(binding)
}
