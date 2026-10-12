//! JSON Lines export in cortexiphan's vocabulary (`contracts/cortexiphan-export.md`).

use crate::store::Store;

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
    #[error("export is not implemented")]
    Unimplemented,
}

/// The export as JSON Lines: one record per line, sorted by `(kind, external.id)`.
pub fn export_jsonl(_store: &Store, _options: &ExportOptions) -> Result<String, ExportError> {
    Ok(String::new())
}
