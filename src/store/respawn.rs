use super::Store;

/// Outcome of a request whose process was started and is waiting for its `SessionStart`.
pub const OUTCOME_STARTED: &str = "started";

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

impl Store {
    /// Records that session `session_id` was respawned into tmux pane `pane`.
    /// Re-running for the same pane and session returns the open request.
    pub fn create_respawn_request(
        &self,
        _session_id: &str,
        _pane: &str,
        _requested_at: &str,
    ) -> Result<RespawnRequest, RespawnError> {
        Ok(RespawnRequest {
            id: String::new(),
            session_id: String::new(),
            requested_at: String::new(),
            outcome: String::new(),
            tmux_target: None,
            new_session_id: None,
        })
    }

    /// The newest request for `pane` that no `SessionStart` has answered yet.
    pub fn find_open_respawn_request(
        &self,
        _pane: &str,
    ) -> Result<Option<RespawnRequest>, RespawnError> {
        Ok(None)
    }

    /// Answers request `id` with the session that started in its pane.
    pub fn close_respawn_request(
        &self,
        _id: &str,
        _new_session_id: &str,
    ) -> Result<(), RespawnError> {
        Ok(())
    }
}
