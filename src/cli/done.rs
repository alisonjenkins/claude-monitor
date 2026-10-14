//! `claude-monitor done <session>...` and `claude-monitor undone <session>...`
//! (FR-019b, FR-019c). Only a person marks a session done, and only a finished
//! one.

use std::path::Path;

use serde_json::json;

use super::migrate::{open_viewer_store, store_error_message};
use super::show::{resolve, user_author};
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::store::markers::MarkersError;
use crate::text::single_line;

pub fn run(
    env: &Env,
    state_dir: Option<&Path>,
    sessions: &[String],
    done: bool,
) -> Result<Output, Error> {
    if sessions.is_empty() {
        return Err(Error::Usage(
            "name at least one session to change".to_owned(),
        ));
    }
    let store = open_viewer_store(env, state_dir)?;
    let mut ids = Vec::new();
    for session in sessions {
        let id = resolve(&store, session)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let now = env.clock.now();
    let author = user_author(env);
    let result = if done {
        store.mark_done(&ids, &now, &author)
    } else {
        store.unmark_done(&ids, &now, &author)
    };
    let report = result.map_err(|error| match error {
        MarkersError::NotFinished { .. } => Error::Refused {
            code: "not_finished",
            message: single_line(&error.to_string()),
        },
        MarkersError::UnknownSession { .. } => Error::Refused {
            code: "unknown_session",
            message: error.to_string(),
        },
        other => store_error_message("update done", Some(store.path()), &other),
    })?;
    let verb = if done { "done" } else { "finished again" };
    Ok(Output::Dual {
        text: format!(
            "{} marked {verb} ({} already were)",
            report.changed.len(),
            report.unchanged.len()
        ),
        json: json!({
            "ok": true,
            "marker": if done { "done" } else { "undone" },
            "changed": report.changed,
            "unchanged": report.unchanged,
        }),
    })
}
