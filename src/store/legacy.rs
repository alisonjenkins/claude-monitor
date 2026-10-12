use std::path::Path;

use super::Store;
use crate::env::Clock;

pub const MARKER_ENTITY_KIND: &str = "store";
pub const MARKER_ENTITY_ID: &str = "legacy-runtime";
pub const MARKER_EVENT_KIND: &str = "store.legacy_imported";

#[derive(Debug, thiserror::Error)]
pub enum LegacyError {
    #[error("legacy runtime import is not implemented yet")]
    NotImplemented,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrateReport {
    pub imported: u32,
    pub skipped_existing: u32,
    pub skipped_corrupt: u32,
    /// File names only, never paths or contents.
    pub corrupt_files: Vec<String>,
}

pub fn is_migrated(_store: &Store) -> Result<bool, LegacyError> {
    Err(LegacyError::NotImplemented)
}

pub fn migrate(
    _store: &Store,
    _runtime_dir: &Path,
    _clock: &dyn Clock,
) -> Result<MigrateReport, LegacyError> {
    Err(LegacyError::NotImplemented)
}

pub fn migrate_if_needed(
    _store: &Store,
    _runtime_dir: &Path,
    _clock: &dyn Clock,
) -> Result<Option<MigrateReport>, LegacyError> {
    Err(LegacyError::NotImplemented)
}
