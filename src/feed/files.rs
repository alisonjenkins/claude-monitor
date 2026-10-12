//! Wakes the feed when a plan file bound to a session changes.
//!
//! The source watches the directories of the bound files, not the files: an
//! editor that saves by renaming a new file over the old one would otherwise
//! drop a file watch. Events for other files in those directories are
//! ignored. Bindings are re-read from the store every [`RESYNC_INTERVAL`], so
//! a plan bound while a viewer runs is watched without a restart. A wake says
//! only "re-read": the refresh that follows parses just the files modified
//! since they were last read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::EventSource;
use crate::store::progress::ProgressStoreError;
use crate::store::{Store, StoreError, StoreOptions};

/// How often the set of bound files is re-read from the store.
const RESYNC_INTERVAL: Duration = Duration::from_millis(500);
/// One queued wake is enough: it means "re-read", and more would only repeat it.
const WAKE_QUEUE_DEPTH: usize = 1;

#[derive(Debug, thiserror::Error)]
pub enum FilesSourceError {
    #[error("cannot open the state database for plan watching: {0}")]
    Store(#[from] StoreError),
    #[error("cannot read the bound plan files: {0}")]
    Bindings(#[from] ProgressStoreError),
    #[error("cannot create a file watcher for plan files: {0}")]
    Create(#[from] notify::Error),
}

fn is_content_change(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

type SharedPaths = Arc<Mutex<BTreeSet<PathBuf>>>;

fn lock(paths: &SharedPaths) -> std::sync::MutexGuard<'_, BTreeSet<PathBuf>> {
    paths.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Watches every file a progress binding reads. The notify backend owns its
/// event thread and stops it when this is dropped.
#[derive(Debug)]
pub struct BoundFilesSource {
    store: Store,
    watcher: RecommendedWatcher,
    wakes: Receiver<()>,
    files: SharedPaths,
    dirs: BTreeSet<PathBuf>,
    last_resync: Instant,
    warnings: Vec<String>,
}

impl BoundFilesSource {
    pub fn watch(db_path: &Path) -> Result<Self, FilesSourceError> {
        let store = Store::open(db_path, &StoreOptions::default())?;
        let (tx, wakes) = mpsc::sync_channel(WAKE_QUEUE_DEPTH);
        let files: SharedPaths = Arc::default();
        let interesting = Arc::clone(&files);
        let watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                // A watcher error (for example a queue overflow) may have lost
                // events, so it wakes the consumer like a change does.
                let relevant = match &res {
                    Ok(event) => {
                        is_content_change(&event.kind) && {
                            let files = lock(&interesting);
                            event.paths.iter().any(|p| files.contains(p))
                        }
                    }
                    Err(_) => true,
                };
                if relevant {
                    // Full means a wake is already queued; that covers this one.
                    let _ = tx.try_send(());
                }
            })?;
        let mut source = Self {
            store,
            watcher,
            wakes,
            files,
            dirs: BTreeSet::new(),
            last_resync: Instant::now(),
            warnings: Vec::new(),
        };
        source.resync(true)?;
        Ok(source)
    }

    /// One line per bound file that could not be watched when this was built.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The files watched now, sorted.
    pub fn watched(&self) -> Vec<PathBuf> {
        lock(&self.files).iter().cloned().collect()
    }

    /// Brings the watched directories in line with the store's bindings. A
    /// directory that cannot be watched is skipped and tried again next time.
    fn resync(&mut self, report: bool) -> Result<(), ProgressStoreError> {
        let paths: BTreeSet<PathBuf> = self
            .store
            .list_file_bindings()?
            .into_iter()
            .filter_map(|binding| binding.source.path.map(PathBuf::from))
            .collect();
        let wanted: BTreeSet<PathBuf> = paths
            .iter()
            .filter_map(|p| p.parent())
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_owned)
            .collect();
        for dir in self.dirs.difference(&wanted).cloned().collect::<Vec<_>>() {
            let _ = self.watcher.unwatch(&dir);
            self.dirs.remove(&dir);
        }
        for dir in wanted.difference(&self.dirs.clone()) {
            match self.watcher.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.dirs.insert(dir.clone());
                }
                Err(e) if report => self
                    .warnings
                    .push(format!("plan files: cannot watch {}: {e}", dir.display())),
                Err(_) => {}
            }
        }
        *lock(&self.files) = paths
            .into_iter()
            .filter(|p| p.parent().is_some_and(|dir| self.dirs.contains(dir)))
            .collect();
        Ok(())
    }
}

impl EventSource for BoundFilesSource {
    fn wait(&mut self, timeout: Duration) -> bool {
        if self.last_resync.elapsed() >= RESYNC_INTERVAL {
            self.last_resync = Instant::now();
            // A failed read keeps the current watches; the next pass retries.
            let _ = self.resync(false);
        }
        let woken = self.wakes.recv_timeout(timeout).is_ok();
        while self.wakes.try_recv().is_ok() {}
        woken
    }
}
