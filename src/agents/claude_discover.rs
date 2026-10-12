//! Passive discovery of Claude Code sessions from their transcripts.

use super::{DiscoverError, ScanReport};
use crate::env::Env;
use crate::store::events::EventsError;
use crate::store::sessions::SessionsError;
use crate::store::Store;

/// Longest first prompt kept, in characters.
pub const MAX_PROMPT_CHARS: usize = 500;
/// Longest working directory kept, in bytes; PATH_MAX on Linux.
pub const MAX_CWD_BYTES: usize = 4096;
/// Longest git branch name kept, in bytes; a longer one is dropped.
pub const MAX_BRANCH_BYTES: usize = 256;
/// Bytes read from the start of a transcript (cwd, first prompt) and again from
/// its end (latest timestamp), so a multi-megabyte transcript is never slurped.
pub const READ_WINDOW_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum DiscoverStoreError {
    #[error("{0}")]
    Scan(#[from] DiscoverError),
    #[error("{0}")]
    Sessions(#[from] SessionsError),
    #[error("{0}")]
    Events(#[from] EventsError),
    #[error("cannot begin discovery write transaction: {0}")]
    Begin(rusqlite::Error),
    #[error("cannot commit discovery write transaction: {0}")]
    Commit(rusqlite::Error),
}

/// What one `discover_and_store` run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscoverSummary {
    pub created: usize,
    pub updated: usize,
    pub degraded: usize,
}

pub fn scan(_env: &Env) -> Result<ScanReport, DiscoverError> {
    Ok(ScanReport::default())
}

pub fn discover_and_store(env: &Env, store: &Store) -> Result<DiscoverSummary, DiscoverStoreError> {
    let _ = store;
    let report = scan(env)?;
    Ok(DiscoverSummary {
        degraded: report.degraded,
        ..DiscoverSummary::default()
    })
}
