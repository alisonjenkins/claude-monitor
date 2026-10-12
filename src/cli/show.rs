//! `claude-monitor show <session>`: one session in full.

use std::path::Path;

use serde_json::{json, Value};

use super::list::age;
use super::migrate::{open_viewer_store, store_error_message};
use super::output::strip_control;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::store::queries::OverviewEntry;
use crate::store::Store;

const LABEL_WIDTH: usize = 14;

fn refused(code: &'static str, message: String) -> Error {
    Error::Refused { code, message }
}

/// `id` exactly, else the one session whose id starts with it.
pub(super) fn resolve(store: &Store, id: &str) -> Result<String, Error> {
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
        if let Some(error) = &entry.error {
            map.insert("error".to_owned(), json!(error));
        }
    }

    let (resumed_from, forked_from) = session
        .as_ref()
        .map(|s| (s.resumed_from.clone(), s.forked_from.clone()))
        .unwrap_or_default();
    let text = detail_text(
        &entry,
        resumed_from.as_deref(),
        forked_from.as_deref(),
        &resumed_by,
        &env.clock.now(),
    );
    Ok(Output::Dual { text, json })
}

fn detail_text(
    entry: &OverviewEntry,
    resumed_from: Option<&str>,
    forked_from: Option<&str>,
    resumed_by: &[String],
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
    put("project", entry.project.as_deref());
    put("last active", Some(&active));
    put("task", entry.task_description.as_deref());
    put("left off", entry.left_off.as_deref());
    put("resumed from", resumed_from);
    put("forked from", forked_from);
    put("resumed by", Some(&resumed_by.join(", ")));
    put("error", entry.error.as_deref());
    lines.join("\n")
}
