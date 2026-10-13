//! `claude-monitor show <session>`: one session in full.

use std::path::Path;

use serde_json::{json, Value};

use super::detail::SessionDetail;
use super::list::age;
use super::migrate::{open_viewer_store, store_error_message};
use super::output::strip_control;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::{Author, Note, OwnerKind, Session};
use crate::store::queries::OverviewEntry;
use crate::store::Store;

const LABEL_WIDTH: usize = 14;

fn refused(code: &'static str, message: String) -> Error {
    Error::Refused { code, message }
}

/// The author of a change the person asked for: `$USER`, else `user`.
pub(crate) fn user_author(env: &Env) -> Author {
    Author::User {
        name: env.vars.var("USER").unwrap_or_else(|| "user".to_owned()),
    }
}

/// `id` exactly, else the one session whose id starts with it.
pub(crate) fn resolve(store: &Store, id: &str) -> Result<String, Error> {
    let lookup = |e| store_error_message("find session", Some(store.path()), &e);
    if store.overview_entry(id).map_err(lookup)?.is_some() {
        return Ok(id.to_owned());
    }
    let mut matches = store.session_ids_with_prefix(id, 2).map_err(lookup)?;
    match (matches.pop(), matches.pop()) {
        (Some(only), None) => Ok(only),
        (Some(_), Some(_)) => Err(refused(
            "ambiguous_session",
            format!("{id:?} matches more than one session; use more of the id"),
        )),
        (None, _) => Err(refused(
            "unknown_session",
            format!("no session matches {id:?}"),
        )),
    }
}

pub fn run(env: &Env, state_dir: Option<&Path>, id: &str) -> Result<Output, Error> {
    let store = open_viewer_store(env, state_dir)?;
    let id = resolve(&store, id)?;
    let read = |e| store_error_message("show session", Some(store.path()), &e);
    let entry = store
        .overview_entry(&id)
        .map_err(read)?
        .ok_or_else(|| refused("unknown_session", format!("no session matches {id:?}")))?;
    let resumed_by = store.resumed_by(&id).map_err(read)?;
    let notes = store
        .notes_for(OwnerKind::Session, &id)
        .map_err(|e| store_error_message("read notes", Some(store.path()), &e))?;

    // A row that cannot be decoded still shows what the overview could read.
    let session = store.get_session(&id).ok().flatten();
    let mut json = match &session {
        Some(session) => serde_json::to_value(session),
        None => serde_json::to_value(&entry),
    }
    .unwrap_or_else(|_| json!({ "id": id }));
    if let Value::Object(map) = &mut json {
        map.insert("repo".to_owned(), json!(entry.repo));
        map.insert("project".to_owned(), json!(entry.project));
        map.insert("section".to_owned(), json!(entry.section.as_str()));
        map.insert("running".to_owned(), json!(entry.running));
        map.insert(
            "flags".to_owned(),
            json!(entry.flags.iter().map(|f| f.as_str()).collect::<Vec<_>>()),
        );
        map.insert("resumed_by".to_owned(), json!(resumed_by));
        map.insert("notes".to_owned(), json!(notes));
        if let Some(error) = &entry.error {
            map.insert("error".to_owned(), json!(error));
        }
    }

    let (resumed_from, forked_from) = session
        .as_ref()
        .map(|s| (s.resumed_from.clone(), s.forked_from.clone()))
        .unwrap_or_default();
    let tmux = session.as_ref().and_then(tmux_placement);
    let mut text = detail_text(
        &entry,
        tmux.as_deref(),
        resumed_from.as_deref(),
        forked_from.as_deref(),
        &resumed_by,
        &notes,
        &env.clock.now(),
    );
    // A row that cannot be decoded has no plan or summaries to add.
    if let Some(session) = &session {
        let detail = SessionDetail::load(&store, env, session)
            .map_err(|e| store_error_message("show session progress", Some(store.path()), &e))?;
        if let Value::Object(map) = &mut json {
            map.extend(detail.json_fields());
        }
        let links = detail
            .link_lines()
            .into_iter()
            .map(|link| format!("{:<LABEL_WIDTH$}{link}", "link"));
        for extra in links
            .chain(detail.progress_lines())
            .chain(detail.generated_lines())
        {
            text.push('\n');
            text.push_str(&extra);
        }
    }
    Ok(Output::Dual { text, json })
}

/// `session:window-name (window N)` as last recorded; `None` when no tmux
/// session was ever recorded.
fn tmux_placement(session: &Session) -> Option<String> {
    let name = session.tmux_session.as_deref()?;
    let mut text = format!(
        "{name}:{}",
        session.tmux_window_name.as_deref().unwrap_or_default()
    );
    let pane =
        session
            .tmux_pane_index
            .as_deref()
            .map(|pane| match session.tmux_window_panes.as_deref() {
                Some(count) => format!("pane {pane} of {count}"),
                None => format!("pane {pane}"),
            });
    let details: Vec<String> = session
        .tmux_window_index
        .as_ref()
        .map(|index| format!("window {index}"))
        .into_iter()
        .chain(pane)
        .collect();
    if !details.is_empty() {
        text.push_str(&format!(" ({})", details.join(", ")));
    }
    Some(text)
}

fn detail_text(
    entry: &OverviewEntry,
    tmux: Option<&str>,
    resumed_from: Option<&str>,
    forked_from: Option<&str>,
    resumed_by: &[String],
    notes: &[Note],
    now: &str,
) -> String {
    let mut lines = Vec::new();
    let mut put = |label: &str, value: Option<&str>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            lines.push(format!("{label:<LABEL_WIDTH$}{}", strip_control(value)));
        }
    };
    let flags: Vec<&str> = entry.flags.iter().map(|f| f.as_str()).collect();
    let state = format!(
        "{}{}",
        entry.state,
        if entry.running { " (running)" } else { "" }
    );
    let active = format!(
        "{} ago ({})",
        age(now, &entry.last_activity_at),
        entry.last_activity_at
    );
    put("id", Some(&entry.id));
    put("agent", Some(&entry.agent_kind));
    put("agent session", Some(&entry.agent_session_id));
    put("state", Some(&state));
    put("section", Some(entry.section.as_str()));
    put("flags", Some(&flags.join(", ")));
    put("repo", entry.repo.as_deref());
    put("branch", entry.branch.as_deref());
    put("cwd", Some(&entry.cwd));
    put("tmux", tmux);
    put("project", entry.project.as_deref());
    put("last active", Some(&active));
    put("task", entry.task_description.as_deref());
    put("left off", entry.left_off.as_deref());
    put("resumed from", resumed_from);
    put("forked from", forked_from);
    put("resumed by", Some(&resumed_by.join(", ")));
    put("error", entry.error.as_deref());
    for note in notes {
        put("note", Some(&note.body));
    }
    lines.join("\n")
}
