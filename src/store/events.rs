use serde_json::Value;

use super::Store;
use crate::model::Author;

const AGENT_MODEL_SEPARATOR: char = '/';

#[derive(Debug, thiserror::Error)]
pub enum EventsError {
    #[error("cannot encode payload of {event}: {source}")]
    EncodePayload {
        event: Box<str>,
        source: serde_json::Error,
    },
    #[error("cannot append {event}: {source}")]
    Append {
        event: Box<str>,
        source: rusqlite::Error,
    },
    #[error("cannot read events after seq {cursor}: {source}")]
    Read {
        cursor: i64,
        source: rusqlite::Error,
    },
    #[error("cannot read the last event seq: {source}")]
    LastSeq { source: rusqlite::Error },
    #[error("event {seq} has unknown author kind {author_kind:?}")]
    UnknownAuthorKind { seq: i64, author_kind: String },
    #[error("event {seq} has an unreadable payload: {source}")]
    DecodePayload { seq: i64, source: serde_json::Error },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent<'a> {
    pub at: &'a str,
    pub entity_kind: &'a str,
    pub entity_id: &'a str,
    pub kind: &'a str,
    pub payload: &'a Value,
    pub author: &'a Author,
}

impl NewEvent<'_> {
    fn describe(&self) -> Box<str> {
        format!(
            "{} event for {} {}",
            self.kind, self.entity_kind, self.entity_id
        )
        .into()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub seq: i64,
    pub at: String,
    pub entity_kind: String,
    pub entity_id: String,
    pub kind: String,
    pub payload: Value,
    pub author: Author,
}

struct RawRow {
    seq: i64,
    at: String,
    entity_kind: String,
    entity_id: String,
    kind: String,
    payload_json: String,
    author_kind: String,
    author_name: String,
}

/// An agent's name column is its kind, plus `/model` when the model is known.
fn author_columns(author: &Author) -> (&'static str, String) {
    match author {
        Author::User { name } => ("user", name.clone()),
        Author::Agent { kind, model: None } => ("agent", kind.clone()),
        Author::Agent {
            kind,
            model: Some(model),
        } => ("agent", format!("{kind}{AGENT_MODEL_SEPARATOR}{model}")),
        Author::System { component } => ("system", component.clone()),
    }
}

fn author_from_columns(seq: i64, author_kind: &str, name: String) -> Result<Author, EventsError> {
    match author_kind {
        "user" => Ok(Author::User { name }),
        "system" => Ok(Author::System { component: name }),
        "agent" => Ok(match name.split_once(AGENT_MODEL_SEPARATOR) {
            Some((kind, model)) => Author::Agent {
                kind: kind.to_owned(),
                model: Some(model.to_owned()),
            },
            None => Author::Agent {
                kind: name,
                model: None,
            },
        }),
        other => Err(EventsError::UnknownAuthorKind {
            seq,
            author_kind: other.to_owned(),
        }),
    }
}

impl RawRow {
    fn decode(self) -> Result<EventRow, EventsError> {
        let payload = serde_json::from_str(&self.payload_json).map_err(|source| {
            EventsError::DecodePayload {
                seq: self.seq,
                source,
            }
        })?;
        Ok(EventRow {
            seq: self.seq,
            at: self.at,
            entity_kind: self.entity_kind,
            entity_id: self.entity_id,
            kind: self.kind,
            payload,
            author: author_from_columns(self.seq, &self.author_kind, self.author_name)?,
        })
    }
}

impl Store {
    /// Appends one row and returns its `seq`, the change-feed cursor. Rows are
    /// never updated or deleted (database triggers abort both).
    pub fn append_event(&self, event: &NewEvent<'_>) -> Result<i64, EventsError> {
        let payload_json =
            serde_json::to_string(event.payload).map_err(|source| EventsError::EncodePayload {
                event: event.describe(),
                source,
            })?;
        let (author_kind, author_name) = author_columns(event.author);
        self.connection()
            .query_row(
                "INSERT INTO events (at, entity_kind, entity_id, kind, payload_json, author_kind, author_name) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING seq",
                rusqlite::params![
                    event.at,
                    event.entity_kind,
                    event.entity_id,
                    event.kind,
                    payload_json,
                    author_kind,
                    author_name
                ],
                |row| row.get(0),
            )
            .map_err(|source| EventsError::Append {
                event: event.describe(),
                source,
            })
    }

    /// Appends only when `event.payload` differs from `old_payload`; returns
    /// `None` when nothing changed.
    pub fn append_event_if_changed(
        &self,
        old_payload: &Value,
        event: &NewEvent<'_>,
    ) -> Result<Option<i64>, EventsError> {
        if old_payload == event.payload {
            return Ok(None);
        }
        self.append_event(event).map(Some)
    }

    /// Rows with `seq > cursor`, oldest first, at most `limit`.
    pub fn events_since(&self, cursor: i64, limit: u32) -> Result<Vec<EventRow>, EventsError> {
        let read = |source| EventsError::Read { cursor, source };
        let mut stmt = self
            .connection()
            .prepare_cached(
                "SELECT seq, at, entity_kind, entity_id, kind, payload_json, author_kind, author_name \
                 FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
            )
            .map_err(read)?;
        let raw = stmt
            .query_map(rusqlite::params![cursor, limit], |row| {
                Ok(RawRow {
                    seq: row.get(0)?,
                    at: row.get(1)?,
                    entity_kind: row.get(2)?,
                    entity_id: row.get(3)?,
                    kind: row.get(4)?,
                    payload_json: row.get(5)?,
                    author_kind: row.get(6)?,
                    author_name: row.get(7)?,
                })
            })
            .map_err(read)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(read)?;
        raw.into_iter().map(RawRow::decode).collect()
    }

    /// The highest `seq` appended so far, or 0 for an empty table.
    pub fn last_seq(&self) -> Result<i64, EventsError> {
        self.connection()
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(|source| EventsError::LastSeq { source })
    }
}
