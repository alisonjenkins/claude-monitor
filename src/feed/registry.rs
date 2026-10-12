//! Wakes the feed when a Claude Code session registry file changes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::EventSource;

#[derive(Debug, thiserror::Error)]
pub enum RegistrySourceError {
    #[error("cannot create a file watcher for {}: {source}", dir.display())]
    Create { dir: PathBuf, source: notify::Error },
    #[error("cannot watch Claude session registry {}: {source}", dir.display())]
    Watch { dir: PathBuf, source: notify::Error },
}

#[derive(Debug)]
pub struct ClaudeRegistrySource;

impl ClaudeRegistrySource {
    pub fn watch(_sessions_dir: &Path) -> Result<Self, RegistrySourceError> {
        Ok(Self)
    }
}

impl EventSource for ClaudeRegistrySource {
    fn wait(&mut self, _timeout: Duration) -> bool {
        false
    }
}
