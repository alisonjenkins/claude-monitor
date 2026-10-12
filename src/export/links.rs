//! Links between control-centre records and nodes in the external graph
//! (FR-026, FR-028): how a link is shown as JSON, and `links check`, which
//! compares each link's two statuses through the graph's command-line client
//! and never writes either side.

use std::time::Duration;

use serde_json::{json, Value};

use crate::env::Vars;
use crate::store::links::{Link, LinksError};
use crate::store::Store;

/// The client program `links check` runs, found on `PATH`.
pub const CLIENT_PROGRAM: &str = "cortexiphan";
/// Names the client reads its server address and bearer token from.
pub const SERVER_VAR: &str = "CORTEXIPHAN_SERVER";
pub const TOKEN_VAR: &str = "CORTEXIPHAN_TOKEN";
/// How long one remote lookup may take before the link counts as unreachable.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

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

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("cannot read the links to check: {0}")]
    Links(#[from] LinksError),
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

/// The `status` in the client's `--json` reply, wherever it sits.
pub fn remote_status(_json: &str) -> Option<String> {
    None
}

/// True when the local status and the remote status mean the same thing.
pub fn statuses_agree(_local: &str, _remote: &str) -> bool {
    false
}

/// Compares every link with its remote node. Reads the store and runs the
/// client with read-only subcommands; writes nothing.
pub fn check_links(
    _store: &Store,
    _vars: &dyn Vars,
    _timeout: Duration,
) -> Result<Check, CheckError> {
    Ok(Check::Skipped {
        reason: "not implemented".to_owned(),
    })
}
