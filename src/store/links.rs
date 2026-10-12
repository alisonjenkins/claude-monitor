//! The `links` table: pairs of a session or project with a node in the
//! external graph (FR-026). Each change appends an event in the same
//! transaction; nothing here deletes anything but the link row itself.

use crate::model::{Author, OwnerKind};

use super::Store;

#[derive(Debug, thiserror::Error)]
pub enum LinksError {
    #[error("cannot {operation}: {source}")]
    Db {
        operation: &'static str,
        source: rusqlite::Error,
    },
    #[error("cannot record the {operation} event: {source}")]
    Event {
        operation: &'static str,
        source: super::events::EventsError,
    },
    #[error("link {id} has an unknown owner kind {owner_kind:?}")]
    OwnerKind { id: Box<str>, owner_kind: Box<str> },
    #[error("links are not implemented yet")]
    NotImplemented,
}

/// One `links` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: String,
    pub owner_kind: OwnerKind,
    pub owner_id: String,
    pub cortex_node_id: String,
    pub cortex_doc_id: Option<String>,
    pub cortex_kind: String,
    pub last_agreed_at: Option<String>,
    pub last_remote_status: Option<String>,
}

/// The fields a caller chooses when linking.
#[derive(Debug, Clone, Copy)]
pub struct NewLink<'a> {
    pub owner_kind: OwnerKind,
    pub owner_id: &'a str,
    pub node_id: &'a str,
    pub doc_id: Option<&'a str>,
    pub cortex_kind: &'a str,
}

/// What `add_link` did: the row, and whether it was new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedLink {
    pub link: Link,
    pub created: bool,
}

impl Store {
    /// Links `owner` to the remote node; linking the same pair again changes
    /// nothing and reports `created: false`.
    pub fn add_link(
        &self,
        _new: &NewLink<'_>,
        _at: &str,
        _author: &Author,
    ) -> Result<AddedLink, LinksError> {
        Err(LinksError::NotImplemented)
    }

    /// Removes the link; true when one existed.
    pub fn remove_link(
        &self,
        _owner_kind: OwnerKind,
        _owner_id: &str,
        _node_id: &str,
        _at: &str,
        _author: &Author,
    ) -> Result<bool, LinksError> {
        Err(LinksError::NotImplemented)
    }

    /// The owner's links, oldest first.
    pub fn links_for(
        &self,
        _owner_kind: OwnerKind,
        _owner_id: &str,
    ) -> Result<Vec<Link>, LinksError> {
        Err(LinksError::NotImplemented)
    }

    /// Every link, oldest first.
    pub fn all_links(&self) -> Result<Vec<Link>, LinksError> {
        Err(LinksError::NotImplemented)
    }
}
