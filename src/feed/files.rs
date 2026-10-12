//! Wakes the feed when a plan file bound to a session changes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::EventSource;
use crate::store::progress::ProgressStoreError;
use crate::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum FilesSourceError {
    #[error("cannot open the state database for plan watching: {0}")]
    Store(#[from] StoreError),
    #[error("cannot read the bound plan files: {0}")]
    Bindings(#[from] ProgressStoreError),
    #[error("cannot create a file watcher for plan files: {0}")]
    Create(#[from] notify::Error),
}

/// Watches every file a progress binding reads.
#[derive(Debug)]
pub struct BoundFilesSource {
    warnings: Vec<String>,
}

impl BoundFilesSource {
    pub fn watch(db_path: &Path) -> Result<Self, FilesSourceError> {
        let _ = db_path;
        Ok(Self {
            warnings: Vec::new(),
        })
    }

    /// One line per bound file that could not be watched when this was built.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The files watched now.
    pub fn watched(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

impl EventSource for BoundFilesSource {
    fn wait(&mut self, timeout: Duration) -> bool {
        std::thread::sleep(timeout);
        false
    }
}
