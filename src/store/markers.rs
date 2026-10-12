//! Notes and the park and dismiss markers (FR-018, FR-019). Each change
//! appends a user-authored event in the same transaction. Nothing here
//! deletes a row.

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};

use super::events::{EventsError, NewEvent};
use super::projects::ProjectsError;
use super::sessions::SessionsError;
use super::Store;
use crate::model::{Author, DoneError, Note, OwnerKind, ProjectStatus, Session, State};

/// A flag a person sets on a session, or the matching status on a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Parked,
    Dismissed,
}

impl Marker {
    fn column(self) -> &'static str {
        match self {
            Self::Parked => "parked",
            Self::Dismissed => "dismissed",
        }
    }

    fn event_kind(self, on: bool) -> &'static str {
        match (self, on) {
            (Self::Parked, true) => "session.parked",
            (Self::Parked, false) => "session.unparked",
            (Self::Dismissed, true) => "session.dismissed",
            (Self::Dismissed, false) => "session.undismissed",
        }
    }

    /// The project status that stands for the marker being set.
    fn status(self) -> ProjectStatus {
        match self {
            Self::Parked => ProjectStatus::Parked,
            Self::Dismissed => ProjectStatus::Archived,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MarkersError {
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
    #[error("no session has the id {id:?}")]
    UnknownSession { id: Box<str> },
    #[error("note {id} has an unreadable author: {source}")]
    NoteAuthor {
        id: Box<str>,
        source: serde_json::Error,
    },
    #[error("note {id} has an unknown owner kind {owner_kind:?}")]
    NoteOwner { id: Box<str>, owner_kind: Box<str> },
    #[error("cannot change the project marker: {source}")]
    Project { source: ProjectsError },
    #[error("cannot read session {id} before changing done: {source}")]
    ReadSession { id: Box<str>, source: SessionsError },
    #[error("session {id} is {state}; only a finished session can be marked done")]
    NotFinished { id: Box<str>, state: State },
}

/// What a `done` or `undone` call did, session by session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoneReport {
    pub changed: Vec<String>,
    pub unchanged: Vec<String>,
}

fn db(operation: &'static str) -> impl Fn(rusqlite::Error) -> MarkersError {
    move |source| MarkersError::Db { operation, source }
}

fn begin(store: &Store) -> Result<Transaction<'_>, MarkersError> {
    Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(db("begin write transaction"))
}

fn emit(
    store: &Store,
    operation: &'static str,
    at: &str,
    (entity_kind, entity_id): (&str, &str),
    kind: &str,
    payload: &Value,
    author: &Author,
) -> Result<(), MarkersError> {
    store
        .append_event(&NewEvent {
            at,
            entity_kind,
            entity_id,
            kind,
            payload,
            author,
        })
        .map(|_| ())
        .map_err(|source| MarkersError::Event { operation, source })
}

impl Store {
    /// Attaches a free-text note to a session or project.
    pub fn add_note(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
        body: &str,
        at: &str,
        author: &Author,
    ) -> Result<Note, MarkersError> {
        let note = Note {
            id: uuid::Uuid::new_v4().to_string(),
            owner_kind,
            owner_id: owner_id.to_owned(),
            body: body.to_owned(),
            created_at: at.to_owned(),
            author: author.clone(),
        };
        let author_json =
            serde_json::to_string(author).map_err(|source| MarkersError::NoteAuthor {
                id: note.id.as_str().into(),
                source,
            })?;
        let tx = begin(self)?;
        self.connection()
            .execute(
                "INSERT INTO notes (id, owner_kind, owner_id, body, created_at, author) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    note.id,
                    owner_kind.as_str(),
                    owner_id,
                    body,
                    at,
                    author_json
                ],
            )
            .map_err(db("add note"))?;
        emit(
            self,
            "add note",
            at,
            (owner_kind.as_str(), owner_id),
            "note.added",
            &json!({ "note": note.id, "body": body }),
            author,
        )?;
        tx.commit().map_err(db("commit note"))?;
        Ok(note)
    }

    /// The notes on one session or project, oldest first.
    pub fn notes_for(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
    ) -> Result<Vec<Note>, MarkersError> {
        let mut stmt = self
            .connection()
            .prepare_cached(
                "SELECT id, owner_kind, owner_id, body, created_at, author FROM notes \
                 WHERE owner_kind = ?1 AND owner_id = ?2 ORDER BY created_at, rowid",
            )
            .map_err(db("read notes"))?;
        let rows = stmt
            .query_map(params![owner_kind.as_str(), owner_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(db("read notes"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db("read notes"))?;
        rows.into_iter()
            .map(|(id, kind, owner_id, body, created_at, author)| {
                Ok(Note {
                    owner_kind: OwnerKind::parse(&kind).ok_or_else(|| MarkersError::NoteOwner {
                        id: id.as_str().into(),
                        owner_kind: kind.as_str().into(),
                    })?,
                    author: serde_json::from_str(&author).map_err(|source| {
                        MarkersError::NoteAuthor {
                            id: id.as_str().into(),
                            source,
                        }
                    })?,
                    id,
                    owner_id,
                    body,
                    created_at,
                })
            })
            .collect()
    }

    /// Sets or clears a marker on a session; returns whether anything
    /// changed. A marker already in the wanted state records nothing.
    pub fn set_session_marker(
        &self,
        id: &str,
        marker: Marker,
        on: bool,
        at: &str,
        author: &Author,
    ) -> Result<bool, MarkersError> {
        let tx = begin(self)?;
        let column = marker.column();
        let current: bool = self
            .connection()
            .query_row(
                &format!("SELECT {column} FROM sessions WHERE id = ?1"),
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db("read session marker"))?
            .ok_or_else(|| MarkersError::UnknownSession { id: id.into() })?;
        if current == on {
            return Ok(false);
        }
        self.connection()
            .execute(
                &format!("UPDATE sessions SET {column} = ?2 WHERE id = ?1"),
                params![id, on],
            )
            .map_err(db("set session marker"))?;
        emit(
            self,
            "set session marker",
            at,
            ("session", id),
            marker.event_kind(on),
            &json!({}),
            author,
        )?;
        tx.commit().map_err(db("commit marker"))?;
        Ok(true)
    }

    /// Marks finished sessions done (FR-019b). Every session must be
    /// finished or nothing changes; one already done keeps its time and
    /// records nothing.
    pub fn mark_done(
        &self,
        ids: &[String],
        at: &str,
        author: &Author,
    ) -> Result<DoneReport, MarkersError> {
        let tx = begin(self)?;
        let mut report = DoneReport::default();
        for id in ids {
            let mut session = self.require_session(id)?;
            if session.done_at.is_some() && session.state == State::Finished {
                report.unchanged.push(id.clone());
                continue;
            }
            session
                .mark_done(at)
                .map_err(
                    |DoneError::NotFinished { state }| MarkersError::NotFinished {
                        id: id.as_str().into(),
                        state,
                    },
                )?;
            self.connection()
                .execute(
                    "UPDATE sessions SET done_at = ?2 WHERE id = ?1 AND state = 'finished'",
                    params![id, at],
                )
                .map_err(db("mark session done"))?;
            emit(
                self,
                "mark session done",
                at,
                ("session", id),
                "session.done",
                &json!({}),
                author,
            )?;
            report.changed.push(id.clone());
        }
        tx.commit().map_err(db("commit done"))?;
        Ok(report)
    }

    /// Returns done sessions to the finished group (FR-019c). One that is not
    /// done is left alone and records nothing.
    pub fn unmark_done(
        &self,
        ids: &[String],
        at: &str,
        author: &Author,
    ) -> Result<DoneReport, MarkersError> {
        let tx = begin(self)?;
        let mut report = DoneReport::default();
        for id in ids {
            let session = self.require_session(id)?;
            if session.done_at.is_none() {
                report.unchanged.push(id.clone());
                continue;
            }
            self.connection()
                .execute("UPDATE sessions SET done_at = NULL WHERE id = ?1", [id])
                .map_err(db("unmark session done"))?;
            emit(
                self,
                "unmark session done",
                at,
                ("session", id),
                "session.undone",
                &json!({}),
                author,
            )?;
            report.changed.push(id.clone());
        }
        tx.commit().map_err(db("commit undone"))?;
        Ok(report)
    }

    fn require_session(&self, id: &str) -> Result<Session, MarkersError> {
        self.get_session(id)
            .map_err(|source| MarkersError::ReadSession {
                id: id.into(),
                source,
            })?
            .ok_or_else(|| MarkersError::UnknownSession { id: id.into() })
    }

    /// Sets or clears a marker on a project: parked is the `parked` status
    /// and dismissed is `archived`. Clearing returns the project to `active`
    /// only when it holds that marker's status.
    pub fn set_project_marker(
        &self,
        name: &str,
        marker: Marker,
        on: bool,
        at: &str,
        author: &Author,
    ) -> Result<bool, MarkersError> {
        let wrap = |source| MarkersError::Project { source };
        let project = self
            .project_by_name(name)
            .map_err(wrap)?
            .ok_or_else(|| wrap(ProjectsError::UnknownProject { name: name.into() }))?;
        let next = if on {
            marker.status()
        } else if project.status == marker.status() {
            ProjectStatus::Active
        } else {
            return Ok(false);
        };
        if project.status == next {
            return Ok(false);
        }
        self.set_project_status(name, next, at, author)
            .map_err(wrap)?;
        Ok(true)
    }
}
