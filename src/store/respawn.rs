use rusqlite::{params, OptionalExtension, Row};

use super::Store;

/// Outcome of a request whose process was started and is waiting for its `SessionStart`.
pub const OUTCOME_STARTED: &str = "started";

const COLUMNS: &str = "id, session_id, requested_at, outcome, tmux_target, new_session_id";
const OPEN: &str = "outcome = ?2 AND new_session_id IS NULL";

#[derive(Debug, thiserror::Error)]
pub enum RespawnError {
    #[error("cannot {operation}: {source}")]
    Db {
        operation: &'static str,
        source: rusqlite::Error,
    },
    #[error("respawn request {id} does not exist")]
    Missing { id: Box<str> },
}

/// One `respawn_request` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RespawnRequest {
    pub id: String,
    pub session_id: String,
    pub requested_at: String,
    pub outcome: String,
    pub tmux_target: Option<String>,
    pub new_session_id: Option<String>,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<RespawnRequest> {
    Ok(RespawnRequest {
        id: row.get(0)?,
        session_id: row.get(1)?,
        requested_at: row.get(2)?,
        outcome: row.get(3)?,
        tmux_target: row.get(4)?,
        new_session_id: row.get(5)?,
    })
}

fn db(operation: &'static str) -> impl Fn(rusqlite::Error) -> RespawnError {
    move |source| RespawnError::Db { operation, source }
}

impl Store {
    /// Records that session `session_id` was respawned into tmux pane `pane`.
    /// Re-running for the same pane and session returns the open request.
    pub fn create_respawn_request(
        &self,
        session_id: &str,
        pane: &str,
        requested_at: &str,
    ) -> Result<RespawnRequest, RespawnError> {
        let operation = "create respawn request";
        let existing = self
            .connection()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM respawn_request \
                     WHERE tmux_target = ?1 AND session_id = ?3 AND {OPEN} LIMIT 1"
                ),
                params![pane, OUTCOME_STARTED, session_id],
                from_row,
            )
            .optional()
            .map_err(db(operation))?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let request = RespawnRequest {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_owned(),
            requested_at: requested_at.to_owned(),
            outcome: OUTCOME_STARTED.to_owned(),
            tmux_target: Some(pane.to_owned()),
            new_session_id: None,
        };
        self.connection()
            .execute(
                "INSERT INTO respawn_request (id, session_id, requested_at, outcome, tmux_target) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    request.id,
                    request.session_id,
                    request.requested_at,
                    request.outcome,
                    request.tmux_target
                ],
            )
            .map_err(db(operation))?;
        Ok(request)
    }

    /// The newest request for `pane` that no `SessionStart` has answered yet.
    pub fn find_open_respawn_request(
        &self,
        pane: &str,
    ) -> Result<Option<RespawnRequest>, RespawnError> {
        self.connection()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM respawn_request WHERE tmux_target = ?1 AND {OPEN} \
                     ORDER BY requested_at DESC, rowid DESC LIMIT 1"
                ),
                params![pane, OUTCOME_STARTED],
                from_row,
            )
            .optional()
            .map_err(db("find open respawn request"))
    }

    /// The newest request for session `session_id` that no `SessionStart` has
    /// answered yet.
    pub fn find_open_respawn_request_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<RespawnRequest>, RespawnError> {
        self.connection()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM respawn_request WHERE session_id = ?1 AND {OPEN} \
                     ORDER BY requested_at DESC, rowid DESC LIMIT 1"
                ),
                params![session_id, OUTCOME_STARTED],
                from_row,
            )
            .optional()
            .map_err(db("find open respawn request for session"))
    }

    /// Answers request `id` with the session that started in its pane.
    pub fn close_respawn_request(
        &self,
        id: &str,
        new_session_id: &str,
    ) -> Result<(), RespawnError> {
        let changed = self
            .connection()
            .execute(
                "UPDATE respawn_request SET new_session_id = ?2 WHERE id = ?1",
                params![id, new_session_id],
            )
            .map_err(db("close respawn request"))?;
        if changed == 0 {
            return Err(RespawnError::Missing { id: id.into() });
        }
        Ok(())
    }
}
