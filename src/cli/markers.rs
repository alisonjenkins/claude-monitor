//! `claude-monitor note <session|project> <text>` and
//! `claude-monitor park|unpark|dismiss|undismiss <session|project>`
//! (FR-018, FR-019). None of them deletes anything.

use std::path::Path;

use serde_json::json;

use super::confirm::Question;
use super::migrate::{open_viewer_store, store_error_message};
use super::show::{resolve, user_author};
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::OwnerKind;
use crate::store::markers::Marker;
use crate::store::Store;
use crate::text::single_line;

/// What a `<session|project>` argument names.
pub(super) enum Target {
    Session(String),
    Project { id: String, name: String },
}

fn unknown_target(target: &str) -> Error {
    Error::Refused {
        code: "unknown_target",
        message: format!("{target:?} is neither a session id nor a project name"),
    }
}

/// An exact session id, else an exact project name, else the one session
/// whose id starts with `target`.
pub(super) fn resolve_target(store: &Store, target: &str) -> Result<Target, Error> {
    let lookup =
        |e: &dyn std::fmt::Display| store_error_message("find target", Some(store.path()), e);
    if store
        .overview_entry(target)
        .map_err(|e| lookup(&e))?
        .is_some()
    {
        return Ok(Target::Session(target.to_owned()));
    }
    if let Some(project) = store.project_by_name(target).map_err(|e| lookup(&e))? {
        return Ok(Target::Project {
            id: project.id,
            name: project.name,
        });
    }
    match resolve(store, target) {
        Ok(id) => Ok(Target::Session(id)),
        Err(Error::Refused {
            code: "unknown_session",
            ..
        }) => Err(unknown_target(target)),
        Err(other) => Err(other),
    }
}

pub fn run_note(
    env: &Env,
    state_dir: Option<&Path>,
    target: &str,
    text: &str,
) -> Result<Output, Error> {
    let body = text.trim();
    if body.is_empty() {
        return Err(Error::Usage("a note needs some text".to_owned()));
    }
    let store = open_viewer_store(env, state_dir)?;
    let (kind, owner_id) = match resolve_target(&store, target)? {
        Target::Session(id) => (OwnerKind::Session, id),
        Target::Project { id, .. } => (OwnerKind::Project, id),
    };
    let note = store
        .add_note(kind, &owner_id, body, &env.clock.now(), &user_author(env))
        .map_err(|e| store_error_message("add note", Some(store.path()), &e))?;
    Ok(Output::Dual {
        text: format!("noted on {} {}", kind.as_str(), single_line(&owner_id)),
        json: json!({ "ok": true, "note": note }),
    })
}

/// What `dismiss` will do, for the confirmation; fails the way `dismiss`
/// would on an unknown target.
pub(super) fn dismiss_question(
    env: &Env,
    state_dir: Option<&Path>,
    target: &str,
) -> Result<Question, Error> {
    let store = open_viewer_store(env, state_dir)?;
    Ok(match resolve_target(&store, target)? {
        Target::Session(id) => Question {
            action: format!("Dismiss session {}", single_line(&id)),
            consequence: "it is hidden from the default views; nothing is deleted".to_owned(),
        },
        Target::Project { name, .. } => Question {
            action: format!("Dismiss project {}", single_line(&name)),
            consequence: "it is archived and hidden from the default views; nothing is deleted"
                .to_owned(),
        },
    })
}

pub fn run_marker(
    env: &Env,
    state_dir: Option<&Path>,
    marker: Marker,
    on: bool,
    target: &str,
) -> Result<Output, Error> {
    let store = open_viewer_store(env, state_dir)?;
    let author = user_author(env);
    let now = env.clock.now();
    let (kind, id, shown, changed) = match resolve_target(&store, target)? {
        Target::Session(id) => {
            let changed = store.set_session_marker(&id, marker, on, &now, &author);
            ("session", id.clone(), id, changed)
        }
        Target::Project { id, name } => {
            let changed = store.set_project_marker(&name, marker, on, &now, &author);
            ("project", id, name, changed)
        }
    };
    let changed = changed.map_err(|e| store_error_message("set marker", Some(store.path()), &e))?;
    let verb = match (marker, on) {
        (Marker::Parked, true) => "parked",
        (Marker::Parked, false) => "unparked",
        (Marker::Dismissed, true) => "dismissed",
        (Marker::Dismissed, false) => "undismissed",
    };
    let shown = single_line(&shown);
    let text = if changed {
        format!("{verb} {kind} {shown}")
    } else {
        format!("{kind} {shown} was already {verb}")
    };
    Ok(Output::Dual {
        text,
        json: json!({
            "ok": true,
            "target": { "kind": kind, "id": id },
            "marker": verb,
            "changed": changed,
        }),
    })
}
