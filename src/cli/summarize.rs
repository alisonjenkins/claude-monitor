//! `claude-monitor summarize <session>`: one on-demand model-written summary,
//! from a model on this machine only (FR-017a, FR-020).

use std::path::Path;

use serde_json::json;

use super::migrate::{open_viewer_store, store_error_message};
use super::show::resolve;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::store::summaries::GENERATED_LABEL;
use crate::summary::{ConfigError, ModelClient, MODEL_VAR, URL_VAR};

const OWNER_KIND_SESSION: &str = "session";

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
    let id = resolve(&store, target)?;
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
