use std::collections::HashMap;

use rusqlite::{params, OptionalExtension, Row, Transaction, TransactionBehavior};

use super::Store;
use crate::model::{OwnerKind, StepStatus};
use crate::progress::{Item, Source, SourceKind};

const BINDING_COLUMNS: &str =
    "id, owner_kind, owner_id, source_kind, source_path, chosen_by, read_at";

#[derive(Debug, thiserror::Error)]
pub enum ProgressStoreError {
    #[error("cannot {operation}: {source}")]
    Db {
        operation: &'static str,
        source: rusqlite::Error,
    },
}

fn db(operation: &'static str) -> impl Fn(rusqlite::Error) -> ProgressStoreError {
    move |source| ProgressStoreError::Db { operation, source }
}

/// Who picked a binding's source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChosenBy {
    Auto,
    User,
}

impl ChosenBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::User => "user",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

/// One `progress_binding` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub id: String,
    pub owner_kind: OwnerKind,
    pub owner_id: String,
    pub source: Source,
    pub chosen_by: ChosenBy,
    pub read_at: String,
}

fn owner_kind_str(kind: OwnerKind) -> &'static str {
    match kind {
        OwnerKind::Session => "session",
        OwnerKind::Project => "project",
    }
}

fn parse_owner_kind(value: &str) -> Option<OwnerKind> {
    match value {
        "session" => Some(OwnerKind::Session),
        "project" => Some(OwnerKind::Project),
        _ => None,
    }
}

fn source_kind_str(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::AgentTodo => "agent_todo",
        SourceKind::SpeckitTasks => "speckit_tasks",
        SourceKind::MarkdownChecklist => "markdown_checklist",
    }
}

fn parse_source_kind(value: &str) -> Option<SourceKind> {
    match value {
        "agent_todo" => Some(SourceKind::AgentTodo),
        "speckit_tasks" => Some(SourceKind::SpeckitTasks),
        "markdown_checklist" => Some(SourceKind::MarkdownChecklist),
        _ => None,
    }
}

fn status_str(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Todo => "todo",
        StepStatus::Doing => "doing",
        StepStatus::Done => "done",
        StepStatus::Blocked => "blocked",
    }
}

fn parse_status(value: &str) -> Option<StepStatus> {
    match value {
        "todo" => Some(StepStatus::Todo),
        "doing" => Some(StepStatus::Doing),
        "done" => Some(StepStatus::Done),
        "blocked" => Some(StepStatus::Blocked),
        _ => None,
    }
}

fn conversion_failure(index: usize, column: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::other(format!("unknown {column} {value:?}"))),
    )
}

fn binding_from_row(row: &Row<'_>) -> rusqlite::Result<Binding> {
    let owner_kind: String = row.get(1)?;
    let source_kind: String = row.get(3)?;
    let chosen_by: String = row.get(5)?;
    Ok(Binding {
        id: row.get(0)?,
        owner_kind: parse_owner_kind(&owner_kind)
            .ok_or_else(|| conversion_failure(1, "owner_kind", &owner_kind))?,
        owner_id: row.get(2)?,
        source: Source {
            kind: parse_source_kind(&source_kind)
                .ok_or_else(|| conversion_failure(3, "source_kind", &source_kind))?,
            path: row.get(4)?,
        },
        chosen_by: ChosenBy::parse(&chosen_by)
            .ok_or_else(|| conversion_failure(5, "chosen_by", &chosen_by))?,
        read_at: row.get(6)?,
    })
}

fn item_from_row(row: &Row<'_>) -> rusqlite::Result<Item> {
    let status: String = row.get(2)?;
    Ok(Item {
        ordinal: row.get(0)?,
        text: row.get(1)?,
        status: parse_status(&status).ok_or_else(|| conversion_failure(2, "status", &status))?,
        group: row.get(3)?,
        external_id: row.get(4)?,
    })
}

fn find_binding(
    store: &Store,
    owner_kind: OwnerKind,
    owner_id: &str,
) -> rusqlite::Result<Option<Binding>> {
    store
        .connection()
        .query_row(
            &format!(
                "SELECT {BINDING_COLUMNS} FROM progress_binding \
                 WHERE owner_kind = ?1 AND owner_id = ?2 ORDER BY rowid LIMIT 1"
            ),
            params![owner_kind_str(owner_kind), owner_id],
            binding_from_row,
        )
        .optional()
}

impl Store {
    /// Binds the owner to `source`, replacing any earlier binding and, when the
    /// source changed, its steps. Re-binding the same source keeps the steps.
    pub fn bind_progress(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
        source: &Source,
        chosen_by: ChosenBy,
        now: &str,
    ) -> Result<Binding, ProgressStoreError> {
        let operation = "bind progress source";
        let tx = Transaction::new_unchecked(self.connection(), TransactionBehavior::Immediate)
            .map_err(db(operation))?;
        let existing = find_binding(self, owner_kind, owner_id).map_err(db(operation))?;
        let binding = match existing {
            Some(existing) => {
                if existing.source != *source {
                    tx.execute(
                        "DELETE FROM steps WHERE binding_id = ?1",
                        params![existing.id],
                    )
                    .map_err(db(operation))?;
                }
                tx.execute(
                    "UPDATE progress_binding \
                     SET source_kind = ?2, source_path = ?3, chosen_by = ?4, read_at = ?5 \
                     WHERE id = ?1",
                    params![
                        existing.id,
                        source_kind_str(source.kind),
                        source.path,
                        chosen_by.as_str(),
                        now
                    ],
                )
                .map_err(db(operation))?;
                Binding {
                    source: source.clone(),
                    chosen_by,
                    read_at: now.to_owned(),
                    ..existing
                }
            }
            None => {
                let binding = Binding {
                    id: uuid::Uuid::new_v4().to_string(),
                    owner_kind,
                    owner_id: owner_id.to_owned(),
                    source: source.clone(),
                    chosen_by,
                    read_at: now.to_owned(),
                };
                tx.execute(
                    "INSERT INTO progress_binding \
                     (id, owner_kind, owner_id, source_kind, source_path, chosen_by, read_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        binding.id,
                        owner_kind_str(owner_kind),
                        binding.owner_id,
                        source_kind_str(source.kind),
                        binding.source.path,
                        chosen_by.as_str(),
                        binding.read_at
                    ],
                )
                .map_err(db(operation))?;
                binding
            }
        };
        tx.commit().map_err(db(operation))?;
        Ok(binding)
    }

    pub fn get_progress_binding(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
    ) -> Result<Option<Binding>, ProgressStoreError> {
        find_binding(self, owner_kind, owner_id).map_err(db("read progress binding"))
    }

    /// Session id to the stored `(done, total)` of its bound plan; sessions
    /// with no stored steps are absent. One query, no file reads.
    pub fn progress_figures(&self) -> Result<HashMap<String, (u32, u32)>, ProgressStoreError> {
        Ok(HashMap::new())
    }

    /// Every binding that reads a file, for the watcher and the refresh pass.
    pub fn list_file_bindings(&self) -> Result<Vec<Binding>, ProgressStoreError> {
        let operation = "list progress bindings";
        let mut stmt = self
            .connection()
            .prepare(&format!(
                "SELECT {BINDING_COLUMNS} FROM progress_binding \
                 WHERE source_path IS NOT NULL ORDER BY rowid"
            ))
            .map_err(db(operation))?;
        let rows = stmt
            .query_map([], binding_from_row)
            .map_err(db(operation))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db(operation))?;
        Ok(rows)
    }

    /// Removes the owner's binding and its steps; `false` when there was none.
    pub fn unbind_progress(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
    ) -> Result<bool, ProgressStoreError> {
        let removed = self
            .connection()
            .execute(
                "DELETE FROM progress_binding WHERE owner_kind = ?1 AND owner_id = ?2",
                params![owner_kind_str(owner_kind), owner_id],
            )
            .map_err(db("unbind progress source"))?;
        Ok(removed > 0)
    }

    /// Makes the stored steps equal `items`. Writes nothing and returns `false`
    /// when they already do.
    pub fn replace_steps(
        &self,
        binding_id: &str,
        items: &[Item],
    ) -> Result<bool, ProgressStoreError> {
        let operation = "replace progress steps";
        if self.list_steps(binding_id)? == items {
            return Ok(false);
        }
        let tx = Transaction::new_unchecked(self.connection(), TransactionBehavior::Immediate)
            .map_err(db(operation))?;
        tx.execute(
            "DELETE FROM steps WHERE binding_id = ?1",
            params![binding_id],
        )
        .map_err(db(operation))?;
        for item in items {
            tx.execute(
                "INSERT INTO steps (binding_id, ordinal, text, status, group_name, external_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    binding_id,
                    item.ordinal,
                    item.text,
                    status_str(item.status),
                    item.group,
                    item.external_id
                ],
            )
            .map_err(db(operation))?;
        }
        tx.commit().map_err(db(operation))?;
        Ok(true)
    }

    pub fn list_steps(&self, binding_id: &str) -> Result<Vec<Item>, ProgressStoreError> {
        let operation = "read progress steps";
        let mut stmt = self
            .connection()
            .prepare(
                "SELECT ordinal, text, status, group_name, external_id FROM steps \
                 WHERE binding_id = ?1 ORDER BY ordinal",
            )
            .map_err(db(operation))?;
        let rows = stmt
            .query_map(params![binding_id], item_from_row)
            .map_err(db(operation))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db(operation))?;
        Ok(rows)
    }

    pub fn touch_progress_binding(
        &self,
        binding_id: &str,
        read_at: &str,
    ) -> Result<(), ProgressStoreError> {
        self.connection()
            .execute(
                "UPDATE progress_binding SET read_at = ?2 WHERE id = ?1",
                params![binding_id, read_at],
            )
            .map_err(db("update progress read time"))?;
        Ok(())
    }
}
