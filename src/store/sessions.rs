use super::Store;
use crate::model::Session;

#[derive(Debug, thiserror::Error)]
pub enum SessionsError {
    #[error("{operation} is not implemented yet")]
    NotImplemented { operation: &'static str },
}

impl Store {
    pub fn upsert_session(&self, _session: &Session) -> Result<(), SessionsError> {
        Err(SessionsError::NotImplemented {
            operation: "upsert session",
        })
    }

    pub fn get_session(&self, _id: &str) -> Result<Option<Session>, SessionsError> {
        Err(SessionsError::NotImplemented {
            operation: "get session",
        })
    }

    pub fn get_session_by_agent_id(
        &self,
        _agent_kind: &str,
        _agent_session_id: &str,
    ) -> Result<Option<Session>, SessionsError> {
        Err(SessionsError::NotImplemented {
            operation: "get session by agent id",
        })
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, SessionsError> {
        Err(SessionsError::NotImplemented {
            operation: "list sessions",
        })
    }

    pub fn find_by_pane(&self, _pane: &str) -> Result<Option<Session>, SessionsError> {
        Err(SessionsError::NotImplemented {
            operation: "find session by pane",
        })
    }

    pub fn find_cleared_predecessor(
        &self,
        _pane: Option<&str>,
        _cwd: &str,
    ) -> Result<Option<Session>, SessionsError> {
        Err(SessionsError::NotImplemented {
            operation: "find cleared predecessor",
        })
    }
}
