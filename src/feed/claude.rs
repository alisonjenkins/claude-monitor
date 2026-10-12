//! Wakes the feed when a Claude Code transcript appears or changes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::EventSource;

#[derive(Debug, thiserror::Error)]
pub enum ClaudeSourceError {
    #[error("cannot watch Claude projects directory {}: {source}", dir.display())]
    Watch { dir: PathBuf, source: notify::Error },
}

#[derive(Debug)]
pub struct ClaudeProjectsSource;

impl ClaudeProjectsSource {
    pub fn watch(_projects_dir: &Path) -> Result<Self, ClaudeSourceError> {
        Ok(Self)
    }
}

impl EventSource for ClaudeProjectsSource {
    fn wait(&mut self, _timeout: Duration) -> bool {
        false
    }
}
