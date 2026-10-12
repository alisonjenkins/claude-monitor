use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::EventSource;
use crate::store::DB_FILE_NAME;

/// The database file and the two SQLite sidecar files WAL mode writes to.
const WATCHED_FILE_NAMES: [&str; 3] = [DB_FILE_NAME, "state.db-wal", "state.db-shm"];

/// One queued wake is enough: it means "re-read", and more would only repeat it.
const WAKE_QUEUE_DEPTH: usize = 1;

#[derive(Debug, thiserror::Error)]
pub enum FsSourceError {
    #[error("cannot create a file watcher for {dir}: {source}")]
    Create { dir: PathBuf, source: notify::Error },
    #[error("cannot watch state directory {dir}: {source}")]
    Watch { dir: PathBuf, source: notify::Error },
}

fn is_state_file(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| WATCHED_FILE_NAMES.contains(&name))
}

fn is_content_change(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Watches the state directory, not the database file: the `-wal` file is
/// created, truncated and deleted by SQLite, which would drop a file watch.
/// The notify backend owns its event thread and stops it when this is dropped.
#[derive(Debug)]
pub struct FsWatcherSource {
    _watcher: RecommendedWatcher,
    wakes: Receiver<()>,
}

impl FsWatcherSource {
    pub fn watch(dir: &Path) -> Result<Self, FsSourceError> {
        let (tx, wakes) = mpsc::sync_channel(WAKE_QUEUE_DEPTH);
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                // A watcher error (for example a queue overflow) may have lost
                // events, so it wakes the consumer like a change does.
                let relevant = match &res {
                    Ok(event) => {
                        is_content_change(&event.kind)
                            && event.paths.iter().any(|p| is_state_file(p))
                    }
                    Err(_) => true,
                };
                if relevant {
                    // Full means a wake is already queued; that covers this one.
                    let _ = tx.try_send(());
                }
            })
            .map_err(|source| FsSourceError::Create {
                dir: dir.to_owned(),
                source,
            })?;
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|source| FsSourceError::Watch {
                dir: dir.to_owned(),
                source,
            })?;
        Ok(Self {
            _watcher: watcher,
            wakes,
        })
    }
}

impl EventSource for FsWatcherSource {
    fn wait(&mut self, timeout: Duration) -> bool {
        let woken = self.wakes.recv_timeout(timeout).is_ok();
        while self.wakes.try_recv().is_ok() {}
        woken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};

    #[test]
    fn state_db_and_its_sidecars_are_state_files() {
        for name in ["state.db", "state.db-wal", "state.db-shm"] {
            assert!(
                is_state_file(Path::new("/s").join(name).as_path()),
                "{name}"
            );
        }
    }

    #[test]
    fn other_files_are_not_state_files() {
        for name in ["state.db-journal", "state.db.bak", "notes.txt", "wal"] {
            assert!(
                !is_state_file(Path::new("/s").join(name).as_path()),
                "{name}"
            );
        }
    }

    #[test]
    fn only_create_modify_and_remove_are_content_changes() {
        assert!(is_content_change(&EventKind::Create(CreateKind::File)));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Any)));
        assert!(is_content_change(&EventKind::Remove(RemoveKind::File)));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Any)));
        assert!(!is_content_change(&EventKind::Other));
    }
}
