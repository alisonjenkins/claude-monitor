//! `claude-monitor summarize <session|project>`: one on-demand model-written summary,
//! from a model on this machine only (FR-017a, FR-020).

use std::path::Path;

use serde_json::json;

use super::markers::{resolve_target, Target};
use super::migrate::{open_viewer_store, store_error_message};
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::OwnerKind;
use crate::store::summaries::GENERATED_LABEL;
use crate::store::Store;
use crate::summary::{project_session_line, ConfigError, ModelClient, MODEL_VAR, URL_VAR};

const OWNER_KIND_SESSION: &str = "session";
const OWNER_KIND_PROJECT: &str = "project";

fn refused(code: &'static str, message: String) -> Error {
    Error::Refused { code, message }
}

fn client(env: &Env) -> Result<ModelClient, Error> {
    ModelClient::from_vars(env.vars.as_ref()).map_err(|error| match error {
        ConfigError::NotConfigured { .. } => refused(
            "summary_not_configured",
            format!(
                "no summary model is configured; set {URL_VAR} to a loopback \
                 OpenAI-compatible server and {MODEL_VAR} to its model name"
            ),
        ),
        ConfigError::Endpoint(error) => refused("endpoint_not_loopback", error.to_string()),
    })
}

pub fn run(env: &Env, state_dir: Option<&Path>, target: &str) -> Result<Output, Error> {
    let client = client(env)?;
    let store = open_viewer_store(env, state_dir)?;
    match resolve_target(&store, target) {
        Ok(Target::Session(id)) => summarize_session(env, &client, &store, &id),
        Ok(Target::Project { id, name }) => summarize_project(env, &client, &store, &id, &name),
        Err(Error::Refused {
            code: "unknown_target",
            ..
        }) => Err(refused(
            "unknown_session",
            format!("no session or project matches {target:?}"),
        )),
        Err(other) => Err(other),
    }
}

fn summarize_project(
    env: &Env,
    client: &ModelClient,
    store: &Store,
    id: &str,
    name: &str,
) -> Result<Output, Error> {
    let read = |operation, e: &dyn std::fmt::Display| {
        store_error_message(operation, Some(store.path()), e)
    };
    let notes = store
        .notes_for(OwnerKind::Project, id)
        .map_err(|e| read("read notes", &e))?
        .into_iter()
        .map(|note| note.body)
        .collect::<Vec<_>>()
        .join("\n");
    let lines: Vec<String> = store
        .list_sessions()
        .map_err(|e| read("list sessions", &e))?
        .into_iter()
        .filter(|session| session.project_id.as_deref() == Some(id))
        .filter_map(|session| {
            project_session_line(
                session.task_description.as_deref(),
                session.left_off.as_deref(),
            )
        })
        .collect();
    if notes.is_empty() && lines.is_empty() {
        return Err(refused(
            "nothing_to_summarize",
            format!("project {name} has no note and no session with recorded text"),
        ));
    }
    let body = client
        .summarize_project((!notes.is_empty()).then_some(notes.as_str()), &lines)
        .map_err(|e| refused("summary_failed", e.to_string()))?;
    let made = store
        .insert_model_summary(
            OWNER_KIND_PROJECT,
            id,
            &body,
            client.model(),
            &env.clock.now(),
        )
        .map_err(|e| read("store summary", &e))?;
    let text = format!(
        "Summary of project {name} ({GENERATED_LABEL} by {}, {}):\n{}",
        client.model(),
        made.created_at,
        made.body
    );
    let json = json!({
        "project": name,
        "project_id": id,
        "summary": made.body,
        "model": client.model(),
        "created_at": made.created_at,
        "label": GENERATED_LABEL,
        "model_written": true,
    });
    Ok(Output::Dual { text, json })
}

fn summarize_session(
    env: &Env,
    client: &ModelClient,
    store: &Store,
    id: &str,
) -> Result<Output, Error> {
    let id = id.to_owned();
    let session = store
        .get_session(&id)
        .map_err(|e| store_error_message("read session", Some(store.path()), &e))?
        .ok_or_else(|| refused("unknown_session", format!("no session matches {id:?}")))?;
    if session.task_description.is_none() && session.left_off.is_none() {
        return Err(refused(
            "nothing_to_summarize",
            format!("session {id} has no recorded task or left-off text"),
        ));
    }
    let body = client
        .summarize(
            session.task_description.as_deref(),
            session.left_off.as_deref(),
        )
        .map_err(|e| refused("summary_failed", e.to_string()))?;
    let made = store
        .insert_model_summary(
            OWNER_KIND_SESSION,
            &id,
            &body,
            client.model(),
            &env.clock.now(),
        )
        .map_err(|e| store_error_message("store summary", Some(store.path()), &e))?;
    let text = format!(
        "Summary of {id} ({GENERATED_LABEL} by {}, {}):\n{}",
        client.model(),
        made.created_at,
        made.body
    );
    let json = json!({
        "session": id,
        "summary": made.body,
        "model": client.model(),
        "created_at": made.created_at,
        "label": GENERATED_LABEL,
        "model_written": true,
    });
    Ok(Output::Dual { text, json })
}
