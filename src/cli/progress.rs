//! `claude-monitor progress [<id>]` and `progress bind`: a session's plan
//! progress with its source, and the choice of source (FR-015a).

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::migrate::{open_viewer_store, store_error_message};
use super::{show, Output};
use crate::env::Env;
use crate::error::Error;
use crate::model::{Author, Session};
use crate::progress::binding::{
    self, display_path, session_root, BindingError, Bound, Candidate, Choice, ProgressView,
};
use crate::progress::{Item, Progress, SourceKind};

/// `--source` value that opts a session out of progress.
const SOURCE_NONE: &str = "none";
/// `--source` value for the agent's own todo list, which has no snapshot yet;
/// choosing it is the same as choosing none.
const SOURCE_AGENT_TODO: &str = "agent_todo";

fn refused(code: &'static str, message: String) -> Error {
    Error::Refused { code, message }
}

fn binding_error(store_path: &Path, error: BindingError) -> Error {
    match error {
        BindingError::NotAPlan { .. } => refused("not_a_plan", error.to_string()),
        BindingError::Unreadable { .. } => refused("unreadable_source", error.to_string()),
        other => store_error_message("progress", Some(store_path), &other),
    }
}

/// A relative path means the one under the shell's directory when it exists
/// there; otherwise the session's repository resolves it.
fn from_shell_dir(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    match std::env::current_dir() {
        Ok(cwd) if cwd.join(&path).exists() => cwd.join(path),
        _ => path,
    }
}

pub fn run(
    env: &Env,
    state_dir: Option<&Path>,
    owner: &str,
    file: Option<PathBuf>,
    source: Option<String>,
) -> Result<Output, Error> {
    let choice = match (file, source.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(Error::Usage("give a path or --source, not both".to_owned()))
        }
        (Some(path), None) => Some(Choice::Path(from_shell_dir(path))),
        (None, Some(SOURCE_NONE | SOURCE_AGENT_TODO)) => Some(Choice::None),
        (None, Some(other)) => {
            return Err(Error::Usage(format!(
                "unknown source {other:?}; expected {SOURCE_NONE} or {SOURCE_AGENT_TODO}"
            )))
        }
        (None, None) => None,
    };
    let store = open_viewer_store(env, state_dir)?;
    let id = show::resolve(&store, owner)?;
    let session = store
        .get_session(&id)
        .map_err(|e| store_error_message("read session", Some(store.path()), &e))?
        .ok_or_else(|| refused("unknown_session", format!("no session matches {id:?}")))?;
    let now = env.clock.now();
    let failed = |e| binding_error(store.path(), e);
    match choice {
        Some(choice) => {
            let author = Author::User {
                name: env.vars.var("USER").unwrap_or_else(|| "user".to_owned()),
            };
            binding::choose(&store, env, &session, &choice, &author, &now).map_err(failed)?;
        }
        None => {
            binding::sync_session(&store, env, &session, &now).map_err(failed)?;
        }
    }
    let view = binding::view(&store, env, &session).map_err(failed)?;
    Ok(Output::Dual {
        text: text(&session, &view),
        json: json_value(&session.id, &view),
    })
}

pub(crate) fn current_text(item: &Item) -> String {
    match &item.external_id {
        Some(id) if *id != item.text => format!("{id} {}", item.text),
        _ => item.text.clone(),
    }
}

fn opted_out(bound: &Bound) -> bool {
    bound.binding.source.kind == SourceKind::AgentTodo && bound.binding.source.path.is_none()
}

/// A figure as JSON: `done`, `total`, `current`, its source and whether the
/// source can still be read.
pub(crate) fn progress_json(progress: &Progress, bound: &Bound) -> Value {
    let mut map = Map::new();
    map.insert(
        "source".to_owned(),
        json!(progress
            .source
            .path
            .clone()
            .unwrap_or_else(|| "agent todo list".to_owned())),
    );
    map.insert("source_label".to_owned(), json!(progress.source_label()));
    map.insert("source_kind".to_owned(), json!(progress.source.kind));
    map.insert("done".to_owned(), json!(progress.done));
    map.insert("total".to_owned(), json!(progress.total));
    map.insert(
        "current".to_owned(),
        json!(progress.current.as_ref().map(current_text)),
    );
    map.insert("phases".to_owned(), json!(progress.phases));
    map.insert("stale".to_owned(), json!(bound.stale));
    map.insert(
        "chosen_by".to_owned(),
        json!(bound.binding.chosen_by.as_str()),
    );
    Value::Object(map)
}

/// The command's JSON: `status` is `bound` (with `progress`), `pick_one` (with
/// `candidates`, no figure) or `none` (no figure, FR-016).
pub(crate) fn json_value(session_id: &str, view: &ProgressView) -> Value {
    let mut map = Map::new();
    map.insert("session".to_owned(), json!(session_id));
    match view {
        ProgressView::None => {
            map.insert("status".to_owned(), json!("none"));
        }
        ProgressView::PickOne(candidates) => {
            map.insert("status".to_owned(), json!("pick_one"));
            map.insert("candidates".to_owned(), json!(candidates));
        }
        ProgressView::Bound(bound) => match &bound.progress {
            Some(progress) => {
                map.insert("status".to_owned(), json!("bound"));
                map.insert("progress".to_owned(), progress_json(progress, bound));
            }
            None => {
                map.insert("status".to_owned(), json!("none"));
                map.insert("opted_out".to_owned(), json!(opted_out(bound)));
            }
        },
    }
    Value::Object(map)
}

pub(crate) fn candidate_line(root: &Path, candidate: &Candidate) -> String {
    let kind = match candidate.kind {
        SourceKind::SpeckitTasks => "spec-kit tasks",
        SourceKind::MarkdownChecklist => "Markdown checklist",
        SourceKind::AgentTodo => "agent todo list",
    };
    format!(
        "  {}  ({kind}, {} of {})",
        display_path(root, &candidate.path),
        candidate.done,
        candidate.total
    )
}

fn text(session: &Session, view: &ProgressView) -> String {
    let id = &session.id;
    match view {
        ProgressView::None => format!("{id}: no progress: no plan found for this session"),
        ProgressView::PickOne(candidates) => {
            let root = session_root(session);
            let mut lines = vec![
                format!("{id}: no progress figure: several plans could be this session's."),
                format!("Choose one with `claude-monitor progress bind {id} <path>`:"),
            ];
            lines.extend(candidates.iter().map(|c| candidate_line(&root, c)));
            lines.join("\n")
        }
        ProgressView::Bound(bound) => match &bound.progress {
            Some(progress) => {
                let mut line = format!("{id}: {}  {}", progress.label(), progress.source_label());
                if bound.stale {
                    line.push_str("  (source unreadable: figure may be stale)");
                }
                match &progress.current {
                    Some(current) => format!("{line}\ncurrent: {}", current_text(current)),
                    None => line,
                }
            }
            None if opted_out(bound) => {
                format!("{id}: no progress: this session is set to show none")
            }
            None => format!(
                "{id}: no progress: {} holds no tasks",
                bound.binding.source.path.as_deref().unwrap_or("the source")
            ),
        },
    }
}
