//! The `summaries` table: model-written text, kept apart from the verbatim
//! `task_description` and `left_off` on the session (FR-017a).

use rusqlite::{params, Row};

use super::Store;

/// Label shown wherever a model-written summary appears.
pub const GENERATED_LABEL: &str = "generated";

#[derive(Debug, thiserror::Error)]
pub enum SummariesError {
    #[error("cannot {operation}: {source}")]
    Db {
        operation: &'static str,
        source: rusqlite::Error,
    },
}

/// One `summaries` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub body: String,
    pub model: Option<String>,
    pub model_written: bool,
    pub created_at: String,
}

impl Summary {
    /// `Some("generated")` for model text; verbatim text has no label.
    pub fn label(&self) -> Option<&'static str> {
        self.model_written.then_some(GENERATED_LABEL)
    }
}

const COLUMNS: &str = "id, owner_kind, owner_id, body, model, model_written, created_at";

fn db(operation: &'static str) -> impl Fn(rusqlite::Error) -> SummariesError {
    move |source| SummariesError::Db { operation, source }
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<Summary> {
    Ok(Summary {
        id: row.get(0)?,
        owner_kind: row.get(1)?,
        owner_id: row.get(2)?,
        body: row.get(3)?,
        model: row.get(4)?,
        model_written: row.get(5)?,
        created_at: row.get(6)?,
    })
}

impl Store {
    /// Stores model-written `body` for the owner; never touches the owner's
    /// own verbatim text. Each call adds a row.
    pub fn insert_model_summary(
        &self,
        owner_kind: &str,
        owner_id: &str,
        body: &str,
        model: &str,
        created_at: &str,
    ) -> Result<Summary, SummariesError> {
        let summary = Summary {
            id: uuid::Uuid::new_v4().to_string(),
            owner_kind: owner_kind.to_owned(),
            owner_id: owner_id.to_owned(),
            body: body.to_owned(),
            model: Some(model.to_owned()),
            model_written: true,
            created_at: created_at.to_owned(),
        };
        self.connection()
            .execute(
                &format!("INSERT INTO summaries ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)"),
                params![
                    summary.id,
                    summary.owner_kind,
                    summary.owner_id,
                    summary.body,
                    summary.model,
                    summary.created_at
                ],
            )
            .map_err(db("insert summary"))?;
        Ok(summary)
    }

    /// The owner's summaries, newest first.
    pub fn list_summaries(
        &self,
        owner_kind: &str,
        owner_id: &str,
    ) -> Result<Vec<Summary>, SummariesError> {
        let operation = "list summaries";
        let mut statement = self
            .connection()
            .prepare(&format!(
                "SELECT {COLUMNS} FROM summaries WHERE owner_kind = ?1 AND owner_id = ?2 \
                 ORDER BY created_at DESC, rowid DESC"
            ))
            .map_err(db(operation))?;
        let rows = statement
            .query_map(params![owner_kind, owner_id], from_row)
            .map_err(db(operation))?;
        rows.collect::<Result<_, _>>().map_err(db(operation))
    }
}
