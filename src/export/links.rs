//! Links between control-centre records and nodes in the external graph
//! (FR-026, FR-028): how a link is shown as JSON, and `links check`, which
//! compares each link's two statuses through the graph's command-line client
//! and never writes either side.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::env::Vars;
use crate::model::OwnerKind;
use crate::store::links::{Link, LinksError};
use crate::store::projects::ProjectsError;
use crate::store::sessions::SessionsError;
use crate::store::Store;

/// The client program `links check` runs, found on `PATH`.
pub const CLIENT_PROGRAM: &str = "cortexiphan";
/// Names the client reads its server address and bearer token from.
pub const SERVER_VAR: &str = "CORTEXIPHAN_SERVER";
pub const TOKEN_VAR: &str = "CORTEXIPHAN_TOKEN";
/// How long one remote lookup may take before the link counts as unreachable.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a running lookup is polled for exit.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// A node reply is small; anything past this is not read.
const MAX_REPLY_BYTES: u64 = 1024 * 1024;

pub fn link_json(link: &Link) -> Value {
    json!({
        "id": link.id,
        "owner": { "kind": link.owner_kind.as_str(), "id": link.owner_id },
        "node_id": link.cortex_node_id,
        "doc_id": link.cortex_doc_id,
        "kind": link.cortex_kind,
        "last_agreed_at": link.last_agreed_at,
        "last_remote_status": link.last_remote_status,
    })
}

impl Outcome {
    /// Stable machine name, as in `links check --json`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Agree { .. } => "agree",
            Self::Disagree { .. } => "disagree",
            Self::NoRemoteStatus => "no_remote_status",
            Self::OwnerMissing => "owner_missing",
            Self::Unreachable { .. } => "unreachable",
        }
    }
}

/// One checked link as JSON: the link, both statuses and the outcome. The
/// remote status is given only when it differs from the local one.
pub fn check_json(check: &LinkCheck) -> Value {
    let mut value = link_json(&check.link);
    let (remote, reason) = match &check.outcome {
        Outcome::Disagree { remote, .. } => (Some(remote.as_str()), None),
        Outcome::Unreachable { reason } => (None, Some(reason.as_str())),
        Outcome::Agree { .. } | Outcome::NoRemoteStatus | Outcome::OwnerMissing => (None, None),
    };
    if let Value::Object(map) = &mut value {
        map.insert("local_status".to_owned(), json!(check.local_status));
        map.insert("remote_status".to_owned(), json!(remote));
        map.insert("outcome".to_owned(), json!(check.outcome.name()));
        map.insert("reason".to_owned(), json!(reason));
    }
    value
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("cannot read the links to check: {0}")]
    Links(#[from] LinksError),
    #[error("cannot read a session to compare: {0}")]
    Session(#[from] SessionsError),
    #[error("cannot read the projects to compare: {0}")]
    Projects(#[from] ProjectsError),
}

/// How one link's two sides compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Agree {
        status: String,
    },
    Disagree {
        local: String,
        remote: String,
    },
    /// The remote answered but its reply carries no status (a memory node).
    NoRemoteStatus,
    /// The owner is no longer in the store.
    OwnerMissing,
    Unreachable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCheck {
    pub link: Link,
    pub local_status: Option<String>,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Nothing was run; `reason` says which settings are missing.
    Skipped {
        reason: String,
    },
    Checked(Vec<LinkCheck>),
}

/// The `status` in the client's `--json` reply, wherever it sits. A custom
/// status arrives as an object with one string value.
pub fn remote_status(json: &str) -> Option<String> {
    find_status(&serde_json::from_str::<Value>(json).ok()?)
}

fn find_status(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get("status").and_then(status_text) {
                return Some(found);
            }
            map.values().find_map(find_status)
        }
        Value::Array(items) => items.iter().find_map(find_status),
        _ => None,
    }
}

fn status_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) if map.len() == 1 => map.values().find_map(|v| match v {
            Value::String(text) => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// Local session states and project statuses mapped onto the remote task
/// vocabulary (todo, doing, done, blocked); anything else compares as itself.
fn canonical(status: &str) -> String {
    let lower = status.trim().to_ascii_lowercase();
    match lower.as_str() {
        "working" | "needs_permission" | "idle" | "in_progress" | "doing" | "active" => "doing",
        "finished" | "done" | "archived" => "done",
        "interrupted" | "parked" | "blocked" => "blocked",
        other => return other.to_owned(),
    }
    .to_owned()
}

/// True when the local status and the remote status mean the same thing.
pub fn statuses_agree(local: &str, remote: &str) -> bool {
    canonical(local) == canonical(remote)
}

fn local_status(store: &Store, link: &Link) -> Result<Option<String>, CheckError> {
    Ok(match link.owner_kind {
        OwnerKind::Session => store.get_session(&link.owner_id)?.map(|session| {
            if session.done_at.is_some() {
                "done".to_owned()
            } else {
                session.state.as_str().to_owned()
            }
        }),
        OwnerKind::Project => store
            .list_projects()?
            .into_iter()
            .find(|project| project.id == link.owner_id)
            .map(|project| project.status.as_str().to_owned()),
    })
}

fn find_client(vars: &dyn Vars) -> Option<PathBuf> {
    let path = vars.var("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(CLIENT_PROGRAM))
        .find(|candidate| candidate.is_file())
}

fn unreachable(reason: impl Into<String>) -> Result<String, String> {
    Err(reason.into())
}

/// Runs `cortexiphan --json node get --doc D --id N` and returns its stdout.
/// Every failure is a reason string; the caller reports it, never raises it.
fn fetch_node(
    client: Option<&PathBuf>,
    server: &str,
    token: &str,
    link: &Link,
    timeout: Duration,
) -> Result<String, String> {
    let Some(doc) = link.cortex_doc_id.as_deref() else {
        return unreachable("no doc id recorded for this link; link it again with --doc");
    };
    let Some(client) = client else {
        return unreachable(format!("{CLIENT_PROGRAM} was not found on PATH"));
    };
    let mut child = Command::new(client)
        .args(["--json", "node", "get", "--doc", doc, "--id"])
        .arg(&link.cortex_node_id)
        .env(SERVER_VAR, server)
        .env(TOKEN_VAR, token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", client.display()))?;
    let reader = child.stdout.take().map(|stdout| {
        thread::spawn(move || {
            let mut reply = String::new();
            let _ = stdout.take(MAX_REPLY_BYTES).read_to_string(&mut reply);
            reply
        })
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("timed out after {}s", timeout.as_secs_f32()));
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(e) => break Err(format!("cannot wait for {CLIENT_PROGRAM}: {e}")),
        }
    };
    let reply = reader
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let status = status?;
    if status.success() {
        Ok(reply)
    } else {
        Err(format!("{CLIENT_PROGRAM} exited with {status}"))
    }
}

fn missing_settings(vars: &dyn Vars) -> Option<String> {
    let missing: Vec<&str> = [SERVER_VAR, TOKEN_VAR]
        .into_iter()
        .filter(|name| vars.var(name).is_none())
        .collect();
    match missing.as_slice() {
        [] => None,
        [one] => Some(format!("{one} is not set")),
        several => Some(format!("{} are not set", several.join(" and "))),
    }
}

fn compare(local: &str, reply: &str) -> Outcome {
    match remote_status(reply) {
        None => Outcome::NoRemoteStatus,
        Some(remote) if statuses_agree(local, &remote) => Outcome::Agree {
            status: local.to_owned(),
        },
        Some(remote) => Outcome::Disagree {
            local: local.to_owned(),
            remote,
        },
    }
}

/// Compares every link with its remote node. Reads the store and runs the
/// client with read-only subcommands; writes nothing. Does nothing unless the
/// server address and token are both set.
pub fn check_links(store: &Store, vars: &dyn Vars, timeout: Duration) -> Result<Check, CheckError> {
    if let Some(reason) = missing_settings(vars) {
        return Ok(Check::Skipped { reason });
    }
    let (server, token) = (
        vars.var(SERVER_VAR).unwrap_or_default(),
        vars.var(TOKEN_VAR).unwrap_or_default(),
    );
    let client = find_client(vars);
    let mut checks = Vec::new();
    for link in store.all_links()? {
        let local = local_status(store, &link)?;
        let outcome = match &local {
            None => Outcome::OwnerMissing,
            Some(local) => match fetch_node(client.as_ref(), &server, &token, &link, timeout) {
                Ok(reply) => compare(local, &reply),
                Err(reason) => Outcome::Unreachable { reason },
            },
        };
        checks.push(LinkCheck {
            link,
            local_status: local,
            outcome,
        });
    }
    Ok(Check::Checked(checks))
}
