use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Row};
use serde::Serialize;

use super::sessions::parse_state;
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

impl SectionName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Interrupted => "interrupted",
            Self::Finished => "finished",
            Self::Done => "done",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "active" => Self::Active,
            "interrupted" => Self::Interrupted,
            "finished" => Self::Finished,
            "done" => Self::Done,
            "dismissed" => Self::Dismissed,
            _ => return None,
        })
    }
}

impl Flag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorktreeMissing => "worktree_missing",
            Self::Parked => "parked",
            Self::Dismissed => "dismissed",
            Self::StateUnknown => "state_unknown",
            Self::Degraded => "degraded",
        }
    }
}

const SELECT: &str = "SELECT s.id, s.agent_kind, s.agent_session_id, s.repo_root, s.cwd, \
     s.branch, s.worktree_exists, s.state, s.task_description, s.left_off, \
     s.last_activity_at, s.parked, s.dismissed, s.done_at, p.name \
     FROM sessions s LEFT JOIN projects p ON p.id = s.project_id";

const NO_PROJECT: &str = "-";

fn read_error(operation: &'static str) -> impl Fn(rusqlite::Error) -> QueriesError {
    move |source| QueriesError::Read { operation, source }
}

fn classify(state: State, dismissed: bool, done: bool) -> SectionName {
    if dismissed {
        SectionName::Dismissed
    } else if done {
        SectionName::Done
    } else {
        match state {
            State::Interrupted => SectionName::Interrupted,
            State::Finished => SectionName::Finished,
            State::Working | State::NeedsPermission | State::Idle | State::Unknown => {
                SectionName::Active
            }
        }
    }
}

fn repo_name(repo_root: Option<&str>) -> Option<String> {
    repo_root
        .and_then(|root| Path::new(root).file_name())
        .map(|name| name.to_string_lossy().into_owned())
}

fn entry_from_row(row: &Row<'_>) -> rusqlite::Result<OverviewEntry> {
    let state_text: String = row.get(7)?;
    let (state, error) = match parse_state(&state_text) {
        Some(state) => (state, None),
        None => (
            State::Unknown,
            Some(format!("unknown state {state_text:?}")),
        ),
    };
    let repo_root: Option<String> = row.get(3)?;
    let worktree_exists: bool = row.get(6)?;
    let parked: bool = row.get(11)?;
    let dismissed: bool = row.get(12)?;
    let done_at: Option<String> = row.get(13)?;

    let mut flags = Vec::new();
    if !worktree_exists {
        flags.push(Flag::WorktreeMissing);
    }
    if parked {
        flags.push(Flag::Parked);
    }
    if dismissed {
        flags.push(Flag::Dismissed);
    }
    if state == State::Unknown && error.is_none() {
        flags.push(Flag::StateUnknown);
    }
    if error.is_some() {
        flags.push(Flag::Degraded);
    }
    Ok(OverviewEntry {
        id: row.get(0)?,
        agent_kind: row.get(1)?,
        agent_session_id: row.get(2)?,
        repo: repo_name(repo_root.as_deref()),
        branch: row.get(5)?,
        cwd: row.get(4)?,
        state,
        running: state.is_active(),
        last_activity_at: row.get(10)?,
        task_description: row.get(8)?,
        left_off: row.get(9)?,
        project: row.get(14)?,
        flags,
        section: classify(state, dismissed, done_at.is_some()),
        error,
    })
}

fn degraded_entry(id: String, error: &rusqlite::Error) -> OverviewEntry {
    OverviewEntry {
        id,
        agent_kind: String::new(),
        agent_session_id: String::new(),
        repo: None,
        branch: None,
        cwd: String::new(),
        state: State::Unknown,
        running: false,
        last_activity_at: String::new(),
        task_description: None,
        left_off: None,
        project: None,
        flags: vec![Flag::Degraded],
        section: SectionName::Active,
        error: Some(error.to_string()),
    }
}

/// The WHERE clauses and bound values for `query`, with `?N` placeholders.
fn filters(query: &OverviewQuery) -> (Vec<String>, Vec<Value>) {
    let mut clauses = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    let mut bind = |value: String| {
        values.push(Value::Text(value));
        format!("?{}", values.len())
    };
    if !query.include_dismissed {
        clauses.push("s.dismissed = 0".to_owned());
    }
    if !query.states.is_empty() {
        let marks: Vec<String> = query
            .states
            .iter()
            .map(|state| bind(state.as_str().to_owned()))
            .collect();
        clauses.push(format!("s.state IN ({})", marks.join(", ")));
    }
    if let Some(agent) = &query.agent {
        clauses.push(format!("s.agent_kind = {}", bind(agent.clone())));
    }
    if let Some(repo) = &query.repo {
        let mark = bind(repo.clone());
        clauses.push(format!(
            "(s.repo_root = {mark} OR substr(s.repo_root, -(length({mark}) + 1)) = '/' || {mark})"
        ));
    }
    if let Some(project) = &query.project {
        if project == NO_PROJECT {
            clauses.push("s.project_id IS NULL".to_owned());
        } else {
            clauses.push(format!("p.name = {}", bind(project.clone())));
        }
    }
    if let Some(text) = &query.text {
        let mark = bind(text.clone());
        clauses.push(format!(
            "(instr(lower(s.task_description), lower({mark})) > 0 \
             OR instr(lower(s.cwd), lower({mark})) > 0)"
        ));
    }
    (clauses, values)
}

/// Finished sessions last active before this instant are "older".
fn finished_cutoff(now: &str, window_days: u32) -> Result<Option<String>, QueriesError> {
    let parsed = DateTime::parse_from_rfc3339(now).map_err(|source| QueriesError::Clock {
        operation: "build overview",
        value: now.to_owned(),
        source,
    })?;
    Ok(parsed
        .with_timezone(&Utc)
        .checked_sub_signed(chrono::Duration::days(i64::from(window_days)))
        .map(|cutoff| cutoff.to_rfc3339_opts(SecondsFormat::Secs, true)))
}

impl Store {
    /// Sessions grouped into view sections, derived on every call. `now` is the
    /// clock reading used to collapse old finished sessions.
    pub fn overview(&self, query: &OverviewQuery, now: &str) -> Result<Overview, QueriesError> {
        let (clauses, values) = filters(query);
        let entries = self.fetch_entries(&clauses, values)?;
        let cutoff = if query.expand_finished {
            None
        } else {
            finished_cutoff(now, query.finished_window_days)?
        };

        let mut names = vec![
            SectionName::Active,
            SectionName::Interrupted,
            SectionName::Finished,
            SectionName::Done,
        ];
        if query.include_dismissed {
            names.push(SectionName::Dismissed);
        }
        let mut sections: Vec<Section> = names
            .into_iter()
            .map(|name| Section {
                name,
                sessions: Vec::new(),
                collapsed_count: 0,
            })
            .collect();
        for entry in entries {
            let collapse = entry.section == SectionName::Finished
                && cutoff
                    .as_deref()
                    .is_some_and(|cutoff| entry.last_activity_at.as_str() < cutoff);
            if let Some(section) = sections.iter_mut().find(|s| s.name == entry.section) {
                if collapse {
                    section.collapsed_count = section.collapsed_count.saturating_add(1);
                } else {
                    section.sessions.push(entry);
                }
            }
        }
        for section in &mut sections {
            if section.name == SectionName::Active {
                section
                    .sessions
                    .sort_by_key(|entry| entry.state != State::NeedsPermission);
            }
        }
        Ok(Overview { sections })
    }

    /// One session as an overview entry (dismissed or not), `None` when `id` is unknown.
    pub fn overview_entry(&self, id: &str) -> Result<Option<OverviewEntry>, QueriesError> {
        let clauses = vec!["s.id = ?1".to_owned()];
        let entries = self.fetch_entries(&clauses, vec![Value::Text(id.to_owned())])?;
        Ok(entries.into_iter().next())
    }

    /// Ids of the sessions whose `resumed_from` is `id`, oldest first.
    pub fn resumed_by(&self, id: &str) -> Result<Vec<String>, QueriesError> {
        let read = read_error("list successor sessions");
        let mut stmt = self
            .connection()
            .prepare_cached("SELECT id FROM sessions WHERE resumed_from = ?1 ORDER BY rowid")
            .map_err(&read)?;
        let rows = stmt
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(&read)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(&read)?;
        Ok(rows)
    }

    fn fetch_entries(
        &self,
        clauses: &[String],
        values: Vec<Value>,
    ) -> Result<Vec<OverviewEntry>, QueriesError> {
        let read = read_error("query sessions");
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let sql = format!("{SELECT}{where_clause} ORDER BY s.last_activity_at DESC, s.rowid DESC");
        let mut stmt = self.connection().prepare(&sql).map_err(&read)?;
        let rows = stmt
            .query_map(params_from_iter(values), |row| {
                let id: String = row.get(0)?;
                Ok(entry_from_row(row).unwrap_or_else(|error| degraded_entry(id, &error)))
            })
            .map_err(&read)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(&read)?;
        Ok(rows)
    }
}
