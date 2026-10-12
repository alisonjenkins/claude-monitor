//! JSON Lines export in cortexiphan's vocabulary (`contracts/cortexiphan-export.md`).
//!
//! Nothing here is specific to any one project: a project is whatever rows the
//! store holds (FR-029). Conversation text never leaves the store unless
//! `include_text` is set.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Params, Row};
use serde_json::{json, Value};

use super::schema::{CONNECTOR_NAME, SCHEMA_VERSION, SYSTEM};
use crate::model::{Author, Session};
use crate::store::events::EventsError;
use crate::store::sessions::{SessionRow, SessionsError};
use crate::store::Store;

const NO_LIMIT: u32 = u32::MAX;

/// What to export and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    /// Only this project, by name or id; `None` exports everything.
    pub project: Option<String>,
    /// Include conversation-derived text (`task_description`, `left_off`, summaries).
    pub include_text: bool,
    /// Machine name recorded in each repository clone.
    pub machine: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("cannot read {what} for the export: {source}")]
    Read {
        what: &'static str,
        source: rusqlite::Error,
    },
    #[error("cannot read sessions for the export: {source}")]
    ReadSessions { source: SessionsError },
    #[error("cannot read events for the export: {source}")]
    ReadEvents { source: EventsError },
    #[error("no project named or numbered {name:?}")]
    UnknownProject { name: String },
    #[error("{entity} has {field} {value:?}, which the export cannot map")]
    Value {
        entity: String,
        field: &'static str,
        value: String,
    },
    #[error("{entity} has {field} {value:?}, which is not an RFC 3339 timestamp")]
    Time {
        entity: String,
        field: &'static str,
        value: String,
    },
}

struct ProjectRow {
    id: String,
    name: String,
    status: String,
    note: Option<String>,
    binding_id: Option<String>,
    updated_at: String,
}

struct BindingRow {
    source_path: Option<String>,
    read_at: String,
}

struct StepRow {
    ordinal: u32,
    text: String,
    status: String,
    external_id: Option<String>,
}

struct NoteRow {
    id: String,
    owner_id: String,
    body: String,
    created_at: String,
    author: String,
}

struct SummaryRow {
    id: String,
    owner_id: String,
    body: String,
    model: Option<String>,
    created_at: String,
}

/// Who last changed an entity, and when, from the append-only events.
type LastChange = HashMap<(String, String), (String, Author)>;

struct Record {
    kind: &'static str,
    id: String,
    author: Value,
    updated_at: String,
    node: Value,
}

impl Record {
    fn to_line(&self) -> String {
        json!({
            "schema_version": SCHEMA_VERSION,
            "kind": self.kind,
            "external": { "system": SYSTEM, "id": self.id, "url": null },
            "author": self.author,
            "updated_at": self.updated_at,
            "node": self.node,
        })
        .to_string()
    }
}

fn query<T, P: Params>(
    store: &Store,
    what: &'static str,
    sql: &str,
    params: P,
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, ExportError> {
    let read = |source| ExportError::Read { what, source };
    store
        .connection()
        .prepare(sql)
        .map_err(read)?
        .query_map(params, map)
        .map_err(read)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(read)
}

fn load_projects(store: &Store) -> Result<Vec<ProjectRow>, ExportError> {
    query(
        store,
        "projects",
        "SELECT id, name, status, note, progress_binding_id, updated_at FROM projects ORDER BY id",
        [],
        |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                status: row.get(2)?,
                note: row.get(3)?,
                binding_id: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
}

fn load_binding(store: &Store, id: &str) -> Result<Option<BindingRow>, ExportError> {
    let rows = query(
        store,
        "progress bindings",
        "SELECT source_path, read_at FROM progress_binding WHERE id = ?1",
        params![id],
        |row| {
            Ok(BindingRow {
                source_path: row.get(0)?,
                read_at: row.get(1)?,
            })
        },
    )?;
    Ok(rows.into_iter().next())
}

fn load_steps(store: &Store, binding_id: &str) -> Result<Vec<StepRow>, ExportError> {
    query(
        store,
        "steps",
        "SELECT ordinal, text, status, external_id FROM steps WHERE binding_id = ?1 ORDER BY ordinal",
        params![binding_id],
        |row| {
            Ok(StepRow {
                ordinal: row.get(0)?,
                text: row.get(1)?,
                status: row.get(2)?,
                external_id: row.get(3)?,
            })
        },
    )
}

fn load_notes(store: &Store) -> Result<Vec<NoteRow>, ExportError> {
    query(
        store,
        "notes",
        "SELECT id, owner_id, body, created_at, author FROM notes ORDER BY id",
        [],
        |row| {
            Ok(NoteRow {
                id: row.get(0)?,
                owner_id: row.get(1)?,
                body: row.get(2)?,
                created_at: row.get(3)?,
                author: row.get(4)?,
            })
        },
    )
}

fn load_summaries(store: &Store) -> Result<Vec<SummaryRow>, ExportError> {
    query(
        store,
        "summaries",
        "SELECT id, owner_id, body, model, created_at FROM summaries \
         WHERE model_written = 1 ORDER BY id",
        [],
        |row| {
            Ok(SummaryRow {
                id: row.get(0)?,
                owner_id: row.get(1)?,
                body: row.get(2)?,
                model: row.get(3)?,
                created_at: row.get(4)?,
            })
        },
    )
}

fn load_last_changes(store: &Store) -> Result<LastChange, ExportError> {
    let events = store
        .events_since(0, NO_LIMIT)
        .map_err(|source| ExportError::ReadEvents { source })?;
    let mut last = LastChange::new();
    for event in events {
        last.insert(
            (event.entity_kind, event.entity_id),
            (event.at, event.author),
        );
    }
    Ok(last)
}

/// `Author` in cortexiphan's vocabulary. A system change is the connector's.
fn author_json(author: &Author) -> Value {
    match author {
        Author::User { name } => json!({ "type": "user", "name": name }),
        Author::Agent { kind, model } => {
            json!({ "type": "llm", "name": model.as_deref().unwrap_or(kind) })
        }
        Author::System { .. } => json!({ "type": "connector", "name": CONNECTOR_NAME }),
    }
}

fn connector_json() -> Value {
    json!({ "type": "connector", "name": CONNECTOR_NAME })
}

fn utc(entity: &str, field: &'static str, text: &str) -> Result<String, ExportError> {
    DateTime::parse_from_rfc3339(text)
        .map(|at| {
            at.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        })
        .map_err(|_| ExportError::Time {
            entity: entity.to_owned(),
            field,
            value: text.to_owned(),
        })
}

fn mapped(
    entity: &str,
    field: &'static str,
    value: &str,
    table: &[(&str, &'static str)],
) -> Result<&'static str, ExportError> {
    table
        .iter()
        .find(|(from, _)| *from == value)
        .map(|(_, to)| *to)
        .ok_or_else(|| ExportError::Value {
            entity: entity.to_owned(),
            field,
            value: value.to_owned(),
        })
}

const PROJECT_STATUS: &[(&str, &str)] = &[
    ("active", "active"),
    ("parked", "paused"),
    ("done", "done"),
    ("archived", "archived"),
];

const STEP_STATUS: &[(&str, &str)] = &[
    ("todo", "todo"),
    ("doing", "doing"),
    ("done", "done"),
    ("blocked", "blocked"),
];

/// Author and `updated_at` of an entity: its last event, else the connector
/// at the entity's own timestamp.
fn attribution(
    changes: &LastChange,
    entity_kind: &str,
    id: &str,
    fallback_at: &str,
    entity: &str,
) -> Result<(Value, String), ExportError> {
    match changes.get(&(entity_kind.to_owned(), id.to_owned())) {
        Some((at, author)) => Ok((author_json(author), utc(entity, "event time", at)?)),
        None => Ok((connector_json(), utc(entity, "updated_at", fallback_at)?)),
    }
}

fn repos(sessions: &[&Session], machine: &str) -> Value {
    let mut by_name: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for root in sessions.iter().filter_map(|s| s.repo_root.as_deref()) {
        let name = Path::new(root)
            .file_name()
            .map_or_else(|| root.to_owned(), |n| n.to_string_lossy().into_owned());
        by_name.entry(name).or_default().insert(root);
    }
    by_name
        .into_iter()
        .map(|(name, paths)| {
            let clones: Vec<Value> = paths
                .into_iter()
                .map(|path| json!({ "machine": machine, "path": path }))
                .collect();
            json!({ "name": name, "uri": null, "clones": clones })
        })
        .collect()
}

fn task_title(step: &StepRow) -> String {
    match &step.external_id {
        Some(id) if !step.text.starts_with(id.as_str()) => format!("{id} {}", step.text),
        _ => step.text.clone(),
    }
}

/// A plan id keys the task only when it is unique in its list; otherwise the
/// position does.
fn task_ids(project_id: &str, steps: &[StepRow]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for id in steps.iter().filter_map(|s| s.external_id.as_deref()) {
        let count = seen.entry(id).or_insert(0);
        *count = count.saturating_add(1);
    }
    steps
        .iter()
        .map(|step| match step.external_id.as_deref() {
            Some(id) if seen.get(id) == Some(&1) => format!("{project_id}:{id}"),
            _ => format!("{project_id}:step-{:04}", step.ordinal),
        })
        .collect()
}

fn session_node(session: &Session, include_text: bool) -> Result<Value, ExportError> {
    let entity = format!("session {}", session.id);
    let mut node = json!({
        "agent_kind": session.agent_kind,
        "agent_session_id": session.agent_session_id,
        "cwd": session.cwd,
        "branch": session.branch,
        "state": session.state.as_str(),
        "last_activity_at": utc(&entity, "last_activity_at", &session.last_activity_at)?,
        "done_at": session
            .done_at
            .as_deref()
            .map(|at| utc(&entity, "done_at", at))
            .transpose()?,
        "resumed_from": session.resumed_from,
        "project": session.project_id,
    });
    if let (true, Some(fields)) = (include_text, node.as_object_mut()) {
        for (name, text) in [
            ("task_description", &session.task_description),
            ("left_off", &session.left_off),
        ] {
            if let Some(text) = text {
                fields.insert(name.to_owned(), json!(text));
            }
        }
    }
    Ok(node)
}

fn edge(from: &Record, to: &str, kind: &'static str) -> Record {
    Record {
        kind: "edge",
        id: format!("{}:{kind}:{to}", from.id),
        author: from.author.clone(),
        updated_at: from.updated_at.clone(),
        node: json!({ "from": from.id, "to": to, "type": kind }),
    }
}

struct Selection {
    projects: Vec<ProjectRow>,
    sessions: Vec<Session>,
}

fn select(store: &Store, filter: Option<&str>) -> Result<Selection, ExportError> {
    let mut projects = load_projects(store)?;
    if let Some(wanted) = filter {
        projects.retain(|p| p.id == wanted || p.name == wanted);
        if projects.is_empty() {
            return Err(ExportError::UnknownProject {
                name: wanted.to_owned(),
            });
        }
    }
    // A row that cannot be read has nothing to export; the rest still do.
    let mut sessions: Vec<Session> = store
        .list_sessions_lenient()
        .map_err(|source| ExportError::ReadSessions { source })?
        .into_iter()
        .filter_map(|row| match row {
            SessionRow::Readable(session) => Some(*session),
            SessionRow::Corrupt { .. } => None,
        })
        .collect();
    if filter.is_some() {
        let ids: HashSet<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        sessions.retain(|s| s.project_id.as_deref().is_some_and(|id| ids.contains(id)));
    }
    Ok(Selection { projects, sessions })
}

/// The export as JSON Lines: one record per line, sorted by `(kind, external.id)`,
/// each key sorted, so two exports of unchanged data are byte-identical.
pub fn export_jsonl(store: &Store, options: &ExportOptions) -> Result<String, ExportError> {
    let Selection { projects, sessions } = select(store, options.project.as_deref())?;
    let changes = load_last_changes(store)?;
    let mut records: Vec<Record> = Vec::new();

    for project in &projects {
        let entity = format!("project {}", project.id);
        let (author, updated_at) = attribution(
            &changes,
            "project",
            &project.id,
            &project.updated_at,
            &entity,
        )?;
        let members: Vec<&Session> = sessions
            .iter()
            .filter(|s| s.project_id.as_deref() == Some(project.id.as_str()))
            .collect();
        let status = mapped(&entity, "status", &project.status, PROJECT_STATUS)?;
        records.push(Record {
            kind: "project",
            id: project.id.clone(),
            author: author.clone(),
            updated_at: updated_at.clone(),
            node: json!({
                "title": project.name,
                "status": status,
                "repos": repos(&members, &options.machine),
            }),
        });
        if let Some(note) = &project.note {
            records.push(Record {
                kind: "memory",
                id: format!("{}:note", project.id),
                author,
                updated_at,
                node: json!({ "body": note, "about": project.id }),
            });
        }
        if let Some(binding_id) = &project.binding_id {
            records.extend(task_records(store, project, binding_id, &changes)?);
        }
    }

    for session in &sessions {
        let entity = format!("session {}", session.id);
        let (author, updated_at) = attribution(
            &changes,
            "session",
            &session.id,
            &session.last_activity_at,
            &entity,
        )?;
        records.push(Record {
            kind: "session",
            id: session.id.clone(),
            author,
            updated_at,
            node: session_node(session, options.include_text)?,
        });
    }

    let exported: HashSet<String> = records
        .iter()
        .filter(|r| r.kind == "session" || r.kind == "project")
        .map(|r| r.id.clone())
        .collect();
    records.extend(memory_records(store, &exported, &changes, options)?);

    let mut edges = Vec::new();
    for record in &records {
        match record.kind {
            "task" => {
                if let Some(project) = record.node.get("part_of").and_then(Value::as_str) {
                    edges.push(edge(record, project, "part_of"));
                }
            }
            "session" => {
                for (field, kind) in [("project", "part_of"), ("resumed_from", "resumed_from")] {
                    if let Some(to) = record.node.get(field).and_then(Value::as_str) {
                        if exported.contains(to) {
                            edges.push(edge(record, to, kind));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    records.extend(edges);

    records.sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));
    Ok(records.iter().map(|r| r.to_line() + "\n").collect())
}

fn task_records(
    store: &Store,
    project: &ProjectRow,
    binding_id: &str,
    changes: &LastChange,
) -> Result<Vec<Record>, ExportError> {
    let Some(binding) = load_binding(store, binding_id)? else {
        return Ok(Vec::new());
    };
    let steps = load_steps(store, binding_id)?;
    let ids = task_ids(&project.id, &steps);
    let (author, updated_at) = attribution(
        changes,
        "progress_binding",
        binding_id,
        &binding.read_at,
        &format!("progress binding {binding_id}"),
    )?;
    steps
        .iter()
        .zip(ids)
        .map(|(step, id)| {
            let entity = format!("step {id}");
            Ok(Record {
                kind: "task",
                node: json!({
                    "title": task_title(step),
                    "status": mapped(&entity, "status", &step.status, STEP_STATUS)?,
                    "part_of": project.id,
                    "source": binding.source_path,
                }),
                id,
                author: author.clone(),
                updated_at: updated_at.clone(),
            })
        })
        .collect()
}

fn memory_records(
    store: &Store,
    exported: &HashSet<String>,
    changes: &LastChange,
    options: &ExportOptions,
) -> Result<Vec<Record>, ExportError> {
    let mut records = Vec::new();
    for note in load_notes(store)? {
        if !exported.contains(&note.owner_id) {
            continue;
        }
        let entity = format!("note {}", note.id);
        let key = ("note".to_owned(), note.id.clone());
        let (author, updated_at) = match changes.get(&key) {
            Some(_) => attribution(changes, "note", &note.id, &note.created_at, &entity)?,
            None => (
                json!({ "type": "user", "name": note.author }),
                utc(&entity, "created_at", &note.created_at)?,
            ),
        };
        records.push(Record {
            kind: "memory",
            id: note.id,
            author,
            updated_at,
            node: json!({ "body": note.body, "about": note.owner_id }),
        });
    }
    if options.include_text {
        for summary in load_summaries(store)? {
            if !exported.contains(&summary.owner_id) {
                continue;
            }
            let entity = format!("summary {}", summary.id);
            records.push(Record {
                kind: "memory",
                author: json!({
                    "type": "llm",
                    "name": summary.model.as_deref().unwrap_or("unknown"),
                }),
                updated_at: utc(&entity, "created_at", &summary.created_at)?,
                node: json!({
                    "body": summary.body,
                    "about": summary.owner_id,
                    "model_written": true,
                }),
                id: summary.id,
            });
        }
    }
    Ok(records)
}
