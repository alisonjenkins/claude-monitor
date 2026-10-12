//! Projects and the grouping of sessions under them (FR-018). Every change
//! appends a user-authored event in the same transaction.

use rusqlite::{params, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::{json, Value};

use super::events::{EventsError, NewEvent};
use super::Store;
use crate::model::{Author, Project, ProjectStatus};

const ENTITY_PROJECT: &str = "project";
const ENTITY_SESSION: &str = "session";

const PROJECT_COLUMNS: &str = "id, name, status, note, progress_binding_id, created_at, updated_at";

#[derive(Debug, thiserror::Error)]
pub enum ProjectsError {
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
    #[error("project {name:?} already exists")]
    Duplicate { name: Box<str> },
    #[error("no project is named {name:?}")]
    UnknownProject { name: Box<str> },
    #[error("no session has the id {id:?}")]
    UnknownSession { id: Box<str> },
    #[error("project {name:?} has an invalid status {status:?}")]
    BadStatus { name: Box<str>, status: Box<str> },
}

/// What a `group` call did, session by session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupReport {
    /// The target project; `None` when the sessions were unassigned.
    pub project: Option<Project>,
    pub changed: Vec<String>,
    pub unchanged: Vec<String>,
}

fn db(operation: &'static str) -> impl Fn(rusqlite::Error) -> ProjectsError {
    move |source| ProjectsError::Db { operation, source }
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<(Project, String)> {
    let status: String = row.get(2)?;
    let project = Project {
        id: row.get(0)?,
        name: row.get(1)?,
        status: ProjectStatus::Active,
        note: row.get(3)?,
        progress_binding_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    };
    Ok((project, status))
}

fn decode(raw: (Project, String)) -> Result<Project, ProjectsError> {
    let (mut project, status) = raw;
    project.status = ProjectStatus::parse(&status).ok_or_else(|| ProjectsError::BadStatus {
        name: project.name.as_str().into(),
        status: status.as_str().into(),
    })?;
    Ok(project)
}

fn begin(store: &Store) -> Result<Transaction<'_>, ProjectsError> {
    Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(db("begin write transaction"))
}

#[allow(clippy::too_many_arguments)]
fn emit(
    store: &Store,
    operation: &'static str,
    at: &str,
    entity_kind: &str,
    entity_id: &str,
    kind: &str,
    payload: &Value,
    author: &Author,
) -> Result<(), ProjectsError> {
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
        .map_err(|source| ProjectsError::Event { operation, source })
}

impl Store {
    pub fn project_by_name(&self, name: &str) -> Result<Option<Project>, ProjectsError> {
        self.connection()
            .query_row(
                &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE name = ?1"),
                [name],
                project_from_row,
            )
            .optional()
            .map_err(db("read project"))?
            .map(decode)
            .transpose()
    }

    /// Every project, by name.
    pub fn list_projects(&self) -> Result<Vec<Project>, ProjectsError> {
        let mut stmt = self
            .connection()
            .prepare_cached(&format!(
                "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY name"
            ))
            .map_err(db("list projects"))?;
        let rows = stmt
            .query_map([], project_from_row)
            .map_err(db("list projects"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db("list projects"))?;
        rows.into_iter().map(decode).collect()
    }

    /// A new active project; the name must be unused.
    pub fn create_project(
        &self,
        name: &str,
        at: &str,
        author: &Author,
    ) -> Result<Project, ProjectsError> {
        let tx = begin(self)?;
        let project = self.insert_project(name, at, author)?;
        tx.commit().map_err(db("commit project"))?;
        Ok(project)
    }

    /// Renames the project; the new name must be unused.
    pub fn rename_project(
        &self,
        name: &str,
        new_name: &str,
        at: &str,
        author: &Author,
    ) -> Result<Project, ProjectsError> {
        let tx = begin(self)?;
        let mut project = self.require_project(name)?;
        if self.project_by_name(new_name)?.is_some() {
            return Err(ProjectsError::Duplicate {
                name: new_name.into(),
            });
        }
        self.connection()
            .execute(
                "UPDATE projects SET name = ?2, updated_at = ?3 WHERE id = ?1",
                params![project.id, new_name, at],
            )
            .map_err(db("rename project"))?;
        emit(
            self,
            "rename project",
            at,
            ENTITY_PROJECT,
            &project.id,
            "project.renamed",
            &json!({ "from": name, "to": new_name }),
            author,
        )?;
        tx.commit().map_err(db("commit project"))?;
        new_name.clone_into(&mut project.name);
        at.clone_into(&mut project.updated_at);
        Ok(project)
    }

    /// Sets the status; a project already in that status is left alone and
    /// records nothing.
    pub fn set_project_status(
        &self,
        name: &str,
        status: ProjectStatus,
        at: &str,
        author: &Author,
    ) -> Result<Project, ProjectsError> {
        let tx = begin(self)?;
        let mut project = self.require_project(name)?;
        if project.status != status {
            self.connection()
                .execute(
                    "UPDATE projects SET status = ?2, updated_at = ?3 WHERE id = ?1",
                    params![project.id, status.as_str(), at],
                )
                .map_err(db("set project status"))?;
            emit(
                self,
                "set project status",
                at,
                ENTITY_PROJECT,
                &project.id,
                "project.status_set",
                &json!({ "from": project.status.as_str(), "to": status.as_str() }),
                author,
            )?;
            project.status = status;
            at.clone_into(&mut project.updated_at);
        }
        tx.commit().map_err(db("commit project"))?;
        Ok(project)
    }

    /// Puts the sessions in `project` (created when it does not exist yet),
    /// or in no project when `project` is `None`. All sessions must exist or
    /// nothing changes; one already in place records nothing.
    pub fn group_sessions(
        &self,
        project: Option<&str>,
        session_ids: &[String],
        at: &str,
        author: &Author,
    ) -> Result<GroupReport, ProjectsError> {
        let tx = begin(self)?;
        let mut previous = Vec::with_capacity(session_ids.len());
        for id in session_ids {
            let row: Option<(Option<String>, Option<String>)> = self
                .connection()
                .query_row(
                    "SELECT s.project_id, p.name FROM sessions s \
                     LEFT JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(db("read session project"))?;
            previous.push(row.ok_or_else(|| ProjectsError::UnknownSession {
                id: id.as_str().into(),
            })?);
        }
        let target = match project {
            Some(name) => Some(match self.project_by_name(name)? {
                Some(existing) => existing,
                None => self.insert_project(name, at, author)?,
            }),
            None => None,
        };
        let target_id = target.as_ref().map(|p| p.id.as_str());
        let mut report = GroupReport {
            project: target.clone(),
            changed: Vec::new(),
            unchanged: Vec::new(),
        };
        for (id, (old_id, old_name)) in session_ids.iter().zip(previous) {
            if old_id.as_deref() == target_id {
                report.unchanged.push(id.clone());
                continue;
            }
            self.connection()
                .execute(
                    "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
                    params![id, target_id],
                )
                .map_err(db("assign session to project"))?;
            emit(
                self,
                "group session",
                at,
                ENTITY_SESSION,
                id,
                "session.grouped",
                &json!({ "project": project, "from": old_name }),
                author,
            )?;
            report.changed.push(id.clone());
        }
        tx.commit().map_err(db("commit grouping"))?;
        Ok(report)
    }

    fn require_project(&self, name: &str) -> Result<Project, ProjectsError> {
        self.project_by_name(name)?
            .ok_or_else(|| ProjectsError::UnknownProject { name: name.into() })
    }

    fn insert_project(
        &self,
        name: &str,
        at: &str,
        author: &Author,
    ) -> Result<Project, ProjectsError> {
        if self.project_by_name(name)?.is_some() {
            return Err(ProjectsError::Duplicate { name: name.into() });
        }
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_owned(),
            status: ProjectStatus::Active,
            note: None,
            progress_binding_id: None,
            created_at: at.to_owned(),
            updated_at: at.to_owned(),
        };
        self.connection()
            .execute(
                "INSERT INTO projects (id, name, status, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![project.id, project.name, project.status.as_str(), at],
            )
            .map_err(db("create project"))?;
        emit(
            self,
            "create project",
            at,
            ENTITY_PROJECT,
            &project.id,
            "project.created",
            &json!({ "name": name }),
            author,
        )?;
        Ok(project)
    }
}
