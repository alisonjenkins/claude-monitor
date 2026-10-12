//! The `links` table: pairs of a session or project with a node in the
//! external graph (FR-026). Each change appends an event in the same
//! transaction; nothing here deletes anything but the link row itself.

use rusqlite::{params, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::json;

use super::events::{EventsError, NewEvent};
use super::Store;
use crate::model::{Author, OwnerKind};

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
        source: EventsError,
    },
    #[error("link {id} has an unknown owner kind {owner_kind:?}")]
    OwnerKind { id: Box<str>, owner_kind: Box<str> },
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

const COLUMNS: &str = "id, owner_kind, owner_id, cortex_node_id, cortex_doc_id, cortex_kind, \
                       last_agreed_at, last_remote_status";

fn db(operation: &'static str) -> impl Fn(rusqlite::Error) -> LinksError {
    move |source| LinksError::Db { operation, source }
}

type RawLink = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

fn raw(row: &Row<'_>) -> rusqlite::Result<RawLink> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn link_from(raw: RawLink) -> Result<Link, LinksError> {
    let (id, owner_kind, owner_id, cortex_node_id, cortex_doc_id, cortex_kind, agreed, status) =
        raw;
    let owner_kind = OwnerKind::parse(&owner_kind).ok_or_else(|| LinksError::OwnerKind {
        id: id.as_str().into(),
        owner_kind: owner_kind.as_str().into(),
    })?;
    Ok(Link {
        id,
        owner_kind,
        owner_id,
        cortex_node_id,
        cortex_doc_id,
        cortex_kind,
        last_agreed_at: agreed,
        last_remote_status: status,
    })
}

fn begin(store: &Store) -> Result<Transaction<'_>, LinksError> {
    Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(db("begin write transaction"))
}

impl Store {
    fn find_link(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
        node_id: &str,
    ) -> Result<Option<Link>, LinksError> {
        self.connection()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM links \
                     WHERE owner_kind = ?1 AND owner_id = ?2 AND cortex_node_id = ?3"
                ),
                params![owner_kind.as_str(), owner_id, node_id],
                raw,
            )
            .optional()
            .map_err(db("read link"))?
            .map(link_from)
            .transpose()
    }

    fn link_event(
        &self,
        operation: &'static str,
        kind: &str,
        (owner_kind, owner_id, node_id): (OwnerKind, &str, &str),
        at: &str,
        author: &Author,
    ) -> Result<(), LinksError> {
        self.append_event(&NewEvent {
            at,
            entity_kind: owner_kind.as_str(),
            entity_id: owner_id,
            kind,
            payload: &json!({ "cortex_node_id": node_id }),
            author,
        })
        .map(|_| ())
        .map_err(|source| LinksError::Event { operation, source })
    }

    /// Links `owner` to the remote node; linking the same pair again changes
    /// nothing and reports `created: false`.
    pub fn add_link(
        &self,
        new: &NewLink<'_>,
        at: &str,
        author: &Author,
    ) -> Result<AddedLink, LinksError> {
        let tx = begin(self)?;
        if let Some(link) = self.find_link(new.owner_kind, new.owner_id, new.node_id)? {
            return Ok(AddedLink {
                link,
                created: false,
            });
        }
        let link = Link {
            id: uuid::Uuid::new_v4().to_string(),
            owner_kind: new.owner_kind,
            owner_id: new.owner_id.to_owned(),
            cortex_node_id: new.node_id.to_owned(),
            cortex_doc_id: new.doc_id.map(str::to_owned),
            cortex_kind: new.cortex_kind.to_owned(),
            last_agreed_at: None,
            last_remote_status: None,
        };
        self.connection()
            .execute(
                &format!(
                    "INSERT INTO links ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)"
                ),
                params![
                    link.id,
                    new.owner_kind.as_str(),
                    new.owner_id,
                    new.node_id,
                    new.doc_id,
                    new.cortex_kind
                ],
            )
            .map_err(db("add link"))?;
        self.link_event(
            "add link",
            "link.added",
            (new.owner_kind, new.owner_id, new.node_id),
            at,
            author,
        )?;
        tx.commit().map_err(db("commit link"))?;
        Ok(AddedLink {
            link,
            created: true,
        })
    }

    /// Removes the link; true when one existed.
    pub fn remove_link(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
        node_id: &str,
        at: &str,
        author: &Author,
    ) -> Result<bool, LinksError> {
        let tx = begin(self)?;
        let removed = self
            .connection()
            .execute(
                "DELETE FROM links \
                 WHERE owner_kind = ?1 AND owner_id = ?2 AND cortex_node_id = ?3",
                params![owner_kind.as_str(), owner_id, node_id],
            )
            .map_err(db("remove link"))?;
        if removed == 0 {
            return Ok(false);
        }
        self.link_event(
            "remove link",
            "link.removed",
            (owner_kind, owner_id, node_id),
            at,
            author,
        )?;
        tx.commit().map_err(db("commit link removal"))?;
        Ok(true)
    }

    fn select_links(
        &self,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Link>, LinksError> {
        let operation = "read links";
        let mut statement = self
            .connection()
            .prepare(&format!(
                "SELECT {COLUMNS} FROM links {filter} ORDER BY rowid"
            ))
            .map_err(db(operation))?;
        let rows = statement
            .query_map(args, raw)
            .map_err(db(operation))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db(operation))?;
        rows.into_iter().map(link_from).collect()
    }

    /// The owner's links, oldest first.
    pub fn links_for(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
    ) -> Result<Vec<Link>, LinksError> {
        self.select_links(
            "WHERE owner_kind = ?1 AND owner_id = ?2",
            &[&owner_kind.as_str(), &owner_id],
        )
    }

    /// Every link, oldest first.
    pub fn all_links(&self) -> Result<Vec<Link>, LinksError> {
        self.select_links("", &[])
    }
}
