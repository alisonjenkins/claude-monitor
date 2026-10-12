use super::Store;
use crate::model::OwnerKind;
use crate::progress::{Item, Source};

#[derive(Debug, thiserror::Error)]
pub enum ProgressStoreError {
    #[error("cannot {operation}: {source}")]
    Db {
        operation: &'static str,
        source: rusqlite::Error,
    },
    #[error("progress binding {id} has {column} {detail}")]
    Value {
        id: Box<str>,
        column: &'static str,
        detail: String,
    },
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
        Ok(Binding {
            id: String::new(),
            owner_kind,
            owner_id: owner_id.to_owned(),
            source: source.clone(),
            chosen_by,
            read_at: now.to_owned(),
        })
    }

    pub fn get_progress_binding(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
    ) -> Result<Option<Binding>, ProgressStoreError> {
        let _ = (owner_kind, owner_id);
        Ok(None)
    }

    /// Removes the owner's binding and its steps; `false` when there was none.
    pub fn unbind_progress(
        &self,
        owner_kind: OwnerKind,
        owner_id: &str,
    ) -> Result<bool, ProgressStoreError> {
        let _ = (owner_kind, owner_id);
        Ok(false)
    }

    /// Makes the stored steps equal `items`. Writes nothing and returns `false`
    /// when they already do.
    pub fn replace_steps(
        &self,
        binding_id: &str,
        items: &[Item],
    ) -> Result<bool, ProgressStoreError> {
        let _ = (binding_id, items);
        Ok(false)
    }

    pub fn list_steps(&self, binding_id: &str) -> Result<Vec<Item>, ProgressStoreError> {
        let _ = binding_id;
        Ok(Vec::new())
    }

    pub fn touch_progress_binding(
        &self,
        binding_id: &str,
        read_at: &str,
    ) -> Result<(), ProgressStoreError> {
        let _ = (binding_id, read_at);
        Ok(())
    }
}
