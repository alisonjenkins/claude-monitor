//! `claude-monitor link <owner> <node-id>` and `unlink` (FR-026): record or
//! drop the pairing of a session or project with an external graph node.

use std::path::Path;

use serde_json::json;

use super::markers::{resolve_target, Target};
use super::migrate::{open_viewer_store, store_error_message};
use super::show::user_author;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::export::links::link_json;
use crate::model::OwnerKind;
use crate::store::links::NewLink;

fn require_number(what: &str, value: &str) -> Result<(), Error> {
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(Error::Usage(format!(
            "{what} must be a decimal number, got {value:?}"
        )))
    }
}

fn owner_of(target: Target) -> (OwnerKind, String) {
    match target {
        Target::Session(id) => (OwnerKind::Session, id),
        Target::Project { id, .. } => (OwnerKind::Project, id),
    }
}

pub fn run_link(
    env: &Env,
    state_dir: Option<&Path>,
    owner: &str,
    node_id: &str,
    kind: &str,
    doc: Option<&str>,
) -> Result<Output, Error> {
    require_number("a node id", node_id)?;
    if let Some(doc) = doc {
        require_number("a doc id", doc)?;
    }
    let store = open_viewer_store(env, state_dir)?;
    let (owner_kind, owner_id) = owner_of(resolve_target(&store, owner)?);
    let added = store
        .add_link(
            &NewLink {
                owner_kind,
                owner_id: &owner_id,
                node_id,
                doc_id: doc,
                cortex_kind: kind,
            },
            &env.clock.now(),
            &user_author(env),
        )
        .map_err(|e| store_error_message("add link", Some(store.path()), &e))?;
    let what = format!("{} {owner_id} to node {node_id}", owner_kind.as_str());
    let text = if added.created {
        format!("linked {what}")
    } else {
        format!("{what} was already linked")
    };
    Ok(Output::Dual {
        text,
        json: json!({
            "ok": true,
            "created": added.created,
            "link": link_json(&added.link),
        }),
    })
}

pub fn run_unlink(
    env: &Env,
    state_dir: Option<&Path>,
    owner: &str,
    node_id: &str,
) -> Result<Output, Error> {
    require_number("a node id", node_id)?;
    let store = open_viewer_store(env, state_dir)?;
    let (owner_kind, owner_id) = owner_of(resolve_target(&store, owner)?);
    let changed = store
        .remove_link(
            owner_kind,
            &owner_id,
            node_id,
            &env.clock.now(),
            &user_author(env),
        )
        .map_err(|e| store_error_message("remove link", Some(store.path()), &e))?;
    let what = format!("{} {owner_id} and node {node_id}", owner_kind.as_str());
    let text = if changed {
        format!("unlinked {what}")
    } else {
        format!("{what} were not linked")
    };
    Ok(Output::Dual {
        text,
        json: json!({
            "ok": true,
            "changed": changed,
            "owner": { "kind": owner_kind.as_str(), "id": owner_id },
            "node_id": node_id,
        }),
    })
}
