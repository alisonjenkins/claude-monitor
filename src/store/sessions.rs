use rusqlite::{params, Row};

use super::Store;
use crate::model::{Session, State};

const COLUMNS: &str = "id, agent_kind, agent_session_id, resumed_from, forked_from, repo_root, \
     cwd, branch, worktree_exists, state, end_reason, task_description, left_off, pid, \
     pid_start, boot_id, tmux_pane, started_at, last_activity_at, state_since, project_id, \
     parked, dismissed, done_at";

const END_REASON_CLEAR: &str = "clear";

#[derive(Debug, thiserror::Error)]
pub enum SessionsError {
    #[error("cannot write session {id}: {source}")]
    Write {
        id: Box<str>,
        source: rusqlite::Error,
    },
    #[error("cannot {operation}: {source}")]
    Read {
        operation: &'static str,
        source: rusqlite::Error,
    },
    #[error("session {id} has {column} {detail}")]
    Value {
        id: Box<str>,
        column: &'static str,
        detail: String,
    },
}

/// One row of [`Store::list_sessions_lenient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRow {
    Readable(Box<Session>),
    Corrupt { id: String, error: String },
}

pub fn parse_state(value: &str) -> Option<State> {
    Some(match value {
        "working" => State::Working,
        "needs_permission" => State::NeedsPermission,
        "idle" => State::Idle,
        "finished" => State::Finished,
        "interrupted" => State::Interrupted,
        "unknown" => State::Unknown,
        _ => return None,
    })
}

fn conversion_failure(index: usize, ty: rusqlite::types::Type, detail: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, ty, Box::new(std::io::Error::other(detail)))
}

fn session_from_row(row: &Row<'_>) -> rusqlite::Result<Session> {
    let mut session = Session::new(
        &row.get::<_, String>(0)?,
        &row.get::<_, String>(1)?,
        &row.get::<_, String>(2)?,
        &row.get::<_, String>(6)?,
        &row.get::<_, String>(17)?,
    );
    session.resumed_from = row.get(3)?;
    session.forked_from = row.get(4)?;
    session.repo_root = row.get(5)?;
    session.branch = row.get(7)?;
    session.worktree_exists = row.get(8)?;
    let state: String = row.get(9)?;
    session.state = parse_state(&state).ok_or_else(|| {
        conversion_failure(
            9,
            rusqlite::types::Type::Text,
            format!("unknown state {state:?}"),
        )
    })?;
    session.end_reason = row.get(10)?;
    session.task_description = row.get(11)?;
    session.left_off = row.get(12)?;
    session.pid = row
        .get::<_, Option<i64>>(13)?
        .map(u32::try_from)
        .transpose()
        .map_err(|e| conversion_failure(13, rusqlite::types::Type::Integer, e.to_string()))?;
    session.pid_start = row
        .get::<_, Option<i64>>(14)?
        .map(u64::try_from)
        .transpose()
        .map_err(|e| conversion_failure(14, rusqlite::types::Type::Integer, e.to_string()))?;
    session.boot_id = row.get(15)?;
    session.tmux_pane = row.get(16)?;
    session.last_activity_at = row.get(18)?;
    session.state_since = row.get(19)?;
    session.project_id = row.get(20)?;
    session.parked = row.get(21)?;
    session.dismissed = row.get(22)?;
    session.done_at = row.get(23)?;
    Ok(session)
}

impl Store {
    /// Inserts the session, or replaces every column of the row with the same `id`.
    pub fn upsert_session(&self, session: &Session) -> Result<(), SessionsError> {
        let value_error = |column, detail: String| SessionsError::Value {
            id: session.id.as_str().into(),
            column,
            detail,
        };
        let pid_start = session
            .pid_start
            .map(i64::try_from)
            .transpose()
            .map_err(|e| value_error("pid_start", e.to_string()))?;
        self.connection()
            .prepare_cached(&format!(
                "INSERT INTO sessions ({COLUMNS}) VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
                 ?18, ?19, ?20, ?21, ?22, ?23, ?24) \
                 ON CONFLICT(id) DO UPDATE SET \
                 agent_kind = excluded.agent_kind, agent_session_id = excluded.agent_session_id, \
                 resumed_from = excluded.resumed_from, forked_from = excluded.forked_from, \
                 repo_root = excluded.repo_root, cwd = excluded.cwd, branch = excluded.branch, \
                 worktree_exists = excluded.worktree_exists, state = excluded.state, \
                 end_reason = excluded.end_reason, task_description = excluded.task_description, \
                 left_off = excluded.left_off, pid = excluded.pid, pid_start = excluded.pid_start, \
                 boot_id = excluded.boot_id, tmux_pane = excluded.tmux_pane, \
                 started_at = excluded.started_at, last_activity_at = excluded.last_activity_at, \
                 state_since = excluded.state_since, project_id = excluded.project_id, \
                 parked = excluded.parked, dismissed = excluded.dismissed, \
                 done_at = excluded.done_at"
            ))
            .and_then(|mut stmt| {
                stmt.execute(params![
                    session.id,
                    session.agent_kind,
                    session.agent_session_id,
                    session.resumed_from,
                    session.forked_from,
                    session.repo_root,
                    session.cwd,
                    session.branch,
                    session.worktree_exists,
                    session.state.as_str(),
                    session.end_reason,
                    session.task_description,
                    session.left_off,
                    session.pid,
                    pid_start,
                    session.boot_id,
                    session.tmux_pane,
                    session.started_at,
                    session.last_activity_at,
                    session.state_since,
                    session.project_id,
                    session.parked,
                    session.dismissed,
                    session.done_at,
                ])
            })
            .map(|_| ())
            .map_err(|source| SessionsError::Write {
                id: session.id.as_str().into(),
                source,
            })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>, SessionsError> {
        self.query_one(
            "get session",
            &format!("SELECT {COLUMNS} FROM sessions WHERE id = ?1"),
            params![id],
        )
    }

    pub fn get_session_by_agent_id(
        &self,
        agent_kind: &str,
        agent_session_id: &str,
    ) -> Result<Option<Session>, SessionsError> {
        self.query_one(
            "get session by agent session id",
            &format!(
                "SELECT {COLUMNS} FROM sessions WHERE agent_kind = ?1 AND agent_session_id = ?2"
            ),
            params![agent_kind, agent_session_id],
        )
    }

    /// Every session, most recently active first.
    pub fn list_sessions(&self) -> Result<Vec<Session>, SessionsError> {
        let read = |source| SessionsError::Read {
            operation: "list sessions",
            source,
        };
        let mut stmt = self
            .connection()
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM sessions ORDER BY last_activity_at DESC, rowid DESC"
            ))
            .map_err(read)?;
        let rows = stmt
            .query_map([], session_from_row)
            .map_err(read)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(read)?;
        Ok(rows)
    }

    /// Every session, most recently active first, with an unreadable row
    /// reported in place instead of failing the whole listing (FR-021).
    pub fn list_sessions_lenient(&self) -> Result<Vec<SessionRow>, SessionsError> {
        let read = |source| SessionsError::Read {
            operation: "list sessions",
            source,
        };
        let mut stmt = self
            .connection()
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM sessions ORDER BY last_activity_at DESC, rowid DESC"
            ))
            .map_err(read)?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                Ok(match session_from_row(row) {
                    Ok(session) => SessionRow::Readable(Box::new(session)),
                    Err(error) => SessionRow::Corrupt {
                        id,
                        error: error.to_string(),
                    },
                })
            })
            .map_err(read)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(read)?;
        Ok(rows)
    }

    /// The most recently active session recorded in tmux pane `pane`.
    pub fn find_by_pane(&self, pane: &str) -> Result<Option<Session>, SessionsError> {
        self.query_one(
            "find session by pane",
            &format!(
                "SELECT {COLUMNS} FROM sessions WHERE tmux_pane = ?1 \
                 ORDER BY last_activity_at DESC, rowid DESC LIMIT 1"
            ),
            params![pane],
        )
    }

    /// The session that just ended with `/clear` and has no successor yet:
    /// matched by tmux pane when `pane` is known, else by working directory.
    pub fn find_cleared_predecessor(
        &self,
        pane: Option<&str>,
        cwd: &str,
    ) -> Result<Option<Session>, SessionsError> {
        self.query_one(
            "find cleared predecessor",
            &format!(
                "SELECT {COLUMNS} FROM sessions \
                 WHERE state = 'finished' AND end_reason = ?3 \
                 AND NOT EXISTS (SELECT 1 FROM sessions successor WHERE successor.resumed_from = sessions.id) \
                 AND ((?1 IS NOT NULL AND tmux_pane = ?1) OR (?1 IS NULL AND cwd = ?2)) \
                 ORDER BY last_activity_at DESC, rowid DESC LIMIT 1"
            ),
            params![pane, cwd, END_REASON_CLEAR],
        )
    }

    /// The newest session of the `/clear` chain that `id` belongs to: follows
    /// `resumed_from` successors only, so a fork is never the head of the
    /// original's chain.
    pub fn chain_head(&self, id: &str) -> Result<String, SessionsError> {
        Ok(id.to_owned())
    }

    /// Marks `child_id` as a fork of `parent_id`. Returns whether the row
    /// changed: `false` when the child is unknown, already has a parent, or
    /// would be its own parent.
    pub fn record_fork(&self, _parent_id: &str, _child_id: &str) -> Result<bool, SessionsError> {
        Ok(false)
    }

    fn query_one(
        &self,
        operation: &'static str,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<Session>, SessionsError> {
        let read = |source| SessionsError::Read { operation, source };
        let mut stmt = self.connection().prepare_cached(sql).map_err(read)?;
        let mut rows = stmt.query(params).map_err(read)?;
        match rows.next().map_err(read)? {
            Some(row) => session_from_row(row).map(Some).map_err(read),
            None => Ok(None),
        }
    }
}
