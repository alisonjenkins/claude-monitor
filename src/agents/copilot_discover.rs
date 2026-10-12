//! Passive discovery of Copilot CLI sessions from their session state.

use std::path::PathBuf;

pub use super::claude_discover::{MAX_PROMPT_CHARS, MAX_SESSION_ID_BYTES, READ_WINDOW_BYTES};
pub use super::discover_store::{DiscoverStoreError, DiscoverSummary};
use super::{DiscoverError, ScanReport};
use crate::env::Env;
use crate::store::Store;

const NOT_IMPLEMENTED: DiscoverError = DiscoverError::Unsupported { kind: "copilot" };

/// The directory whose subdirectories hold one session each.
pub fn session_state_dir(_env: &Env) -> Result<PathBuf, DiscoverError> {
    Err(NOT_IMPLEMENTED)
}

/// Reads every session directory under the session-state directory.
pub fn scan(_env: &Env) -> Result<ScanReport, DiscoverError> {
    Err(NOT_IMPLEMENTED)
}

/// Scans, then records each stub like Claude discovery does.
pub fn discover_and_store(
    env: &Env,
    _store: &Store,
) -> Result<DiscoverSummary, DiscoverStoreError> {
    scan(env)?;
    Ok(DiscoverSummary::default())
}
