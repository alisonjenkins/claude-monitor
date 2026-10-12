use serde::Serialize;

use super::Store;
use crate::model::State;

const DEFAULT_FINISHED_WINDOW_DAYS: u32 = 7;

#[derive(Debug, thiserror::Error)]
pub enum QueriesError {
    #[error("cannot {operation}: {source}")]
    Read {
        operation: &'static str,
        source: rusqlite::Error,
    },
    #[error("cannot {operation}: clock value {value:?} is not an RFC 3339 time: {source}")]
    Clock {
        operation: &'static str,
        value: String,
        source: chrono::ParseError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionName {
    Active,
    Interrupted,
    Finished,
    Done,
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    WorktreeMissing,
    Parked,
    Dismissed,
    StateUnknown,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewQuery {
    pub states: Vec<State>,
    pub agent: Option<String>,
    pub repo: Option<String>,
    pub project: Option<String>,
    pub text: Option<String>,
    pub include_dismissed: bool,
    pub expand_finished: bool,
    /// Finished sessions idle for longer than this collapse into
    /// `Section::collapsed_count` unless `expand_finished` is set.
    pub finished_window_days: u32,
}

impl Default for OverviewQuery {
    fn default() -> Self {
        Self {
            states: Vec::new(),
            agent: None,
            repo: None,
            project: None,
            text: None,
            include_dismissed: false,
            expand_finished: false,
            finished_window_days: DEFAULT_FINISHED_WINDOW_DAYS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverviewEntry {
    pub id: String,
    pub agent_kind: String,
    pub agent_session_id: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub cwd: String,
    pub state: State,
    pub running: bool,
    pub last_activity_at: String,
    pub task_description: Option<String>,
    pub left_off: Option<String>,
    pub project: Option<String>,
    pub flags: Vec<Flag>,
    pub section: SectionName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: SectionName,
    pub sessions: Vec<OverviewEntry>,
    pub collapsed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub sections: Vec<Section>,
}

impl Store {
    pub fn overview(&self, _query: &OverviewQuery, _now: &str) -> Result<Overview, QueriesError> {
        Err(QueriesError::Read {
            operation: "build overview",
            source: rusqlite::Error::InvalidQuery,
        })
    }
}
