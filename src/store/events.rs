use serde_json::Value;

use super::Store;
use crate::model::Author;

#[derive(Debug, thiserror::Error)]
pub enum EventsError {
    #[error("events table is not implemented yet")]
    NotImplemented,
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

impl Store {
    pub fn append_event(&self, _event: &NewEvent<'_>) -> Result<i64, EventsError> {
        Err(EventsError::NotImplemented)
    }

    pub fn append_event_if_changed(
        &self,
        _old_payload: &Value,
        _event: &NewEvent<'_>,
    ) -> Result<Option<i64>, EventsError> {
        Err(EventsError::NotImplemented)
    }

    pub fn events_since(&self, _cursor: i64, _limit: u32) -> Result<Vec<EventRow>, EventsError> {
        Err(EventsError::NotImplemented)
    }

    pub fn last_seq(&self) -> Result<i64, EventsError> {
        Err(EventsError::NotImplemented)
    }
}
