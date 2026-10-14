//! `claude-monitor link <owner> <node-id>` and `unlink` (FR-026): record or
//! drop the pairing of a session or project with an external graph node.

use std::path::Path;

use serde_json::json;

use super::confirm::Question;
use super::markers::{resolve_target, Target};
use super::migrate::{open_viewer_store, store_error_message};
use super::show::user_author;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::export::links::{
    check_json, check_links, link_json, Check, LinkCheck, Outcome, LOOKUP_TIMEOUT,
};
use crate::model::OwnerKind;
use crate::store::links::NewLink;
use crate::text::single_line;

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
    let what = format!(
        "{} {} to node {node_id}",
        owner_kind.as_str(),
        single_line(&owner_id)
    );
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

fn describe(check: &LinkCheck) -> String {
    let link = &check.link;
    let at = format!(
        "{} {} -> node {}",
        link.owner_kind.as_str(),
        single_line(&link.owner_id),
        single_line(&link.cortex_node_id)
    );
    match &check.outcome {
        Outcome::Agree { status } => format!("agree        {at} ({})", single_line(status)),
        Outcome::Disagree { local, remote } => {
            format!(
                "DISAGREE     {at}: local {}, remote {}",
                single_line(local),
                single_line(remote)
            )
        }
        Outcome::NoRemoteStatus => format!("no status    {at}: the remote node has no status"),
        Outcome::OwnerMissing => format!("owner gone   {at}: no such {}", link.owner_kind.as_str()),
        Outcome::Unreachable { reason } => {
            format!("unreachable  {at}: {}", single_line(reason))
        }
    }
}

/// `claude-monitor links check`: always succeeds; a remote that cannot be
/// reached is reported per link and as a warning, never as an error.
pub fn run_links_check(env: &Env, state_dir: Option<&Path>) -> Result<Output, Error> {
    let store = open_viewer_store(env, state_dir)?;
    let result = check_links(&store, env.vars.as_ref(), LOOKUP_TIMEOUT)
        .map_err(|e| store_error_message("check links", Some(store.path()), &e))?;
    let checks = match result {
        Check::Skipped { reason } => {
            let message = format!("links check skipped: {}", single_line(&reason));
            return Ok(Output::Dual {
                text: message,
                json: json!({ "ok": true, "checked": false, "skipped": reason, "links": [] }),
            });
        }
        Check::Checked(checks) => checks,
    };
    let count = |name: &str| checks.iter().filter(|c| c.outcome.name() == name).count();
    let (agree, disagree, unreachable) = (count("agree"), count("disagree"), count("unreachable"));
    let mut lines: Vec<String> = checks.iter().map(describe).collect();
    if lines.is_empty() {
        lines.push("no links to check".to_owned());
    }
    if unreachable > 0 {
        lines.push(format!(
            "warning: {unreachable} link(s) could not be checked; the remote was unreachable"
        ));
    }
    Ok(Output::Dual {
        text: lines.join("\n"),
        json: json!({
            "ok": true,
            "checked": true,
            "summary": { "agree": agree, "disagree": disagree, "unreachable": unreachable },
            "links": checks.iter().map(check_json).collect::<Vec<_>>(),
        }),
    })
}

/// What `unlink` will do, for the confirmation; fails the way `unlink` would
/// on a bad node id or an unknown owner.
pub(super) fn unlink_question(
    env: &Env,
    state_dir: Option<&Path>,
    owner: &str,
    node_id: &str,
) -> Result<Question, Error> {
    require_number("a node id", node_id)?;
    let store = open_viewer_store(env, state_dir)?;
    let (kind, shown) = match resolve_target(&store, owner)? {
        Target::Session(id) => ("session", single_line(&id)),
        Target::Project { name, .. } => ("project", single_line(&name)),
    };
    Ok(Question {
        action: format!("Unlink {kind} {shown} from node {node_id}"),
        consequence: "the link is removed (the node itself is untouched)".to_owned(),
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
    let what = format!(
        "{} {} and node {node_id}",
        owner_kind.as_str(),
        single_line(&owner_id)
    );
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
