//! Links between control-centre records and nodes in the external graph
//! (FR-026, FR-028): how a link is shown as JSON.

use serde_json::{json, Value};

use crate::store::links::Link;

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
