//! Wakes the feed when a Claude Code session registry file changes.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::EventSource;

/// One queued wake is enough: it means "scan again", and more would only repeat it.
const WAKE_QUEUE_DEPTH: usize = 1;
const ENTRY_EXTENSION: &str = "json";

#[derive(Debug, thiserror::Error)]
pub enum RegistrySourceError {
    #[error("cannot create a file watcher for {}: {source}", dir.display())]
    Create { dir: PathBuf, source: notify::Error },
    #[error("cannot watch Claude session registry {}: {source}", dir.display())]
    Watch { dir: PathBuf, source: notify::Error },
}

/// Only `*.json` wakes: the `.key` files beside them are never of interest.
fn is_relevant(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) && event
        .paths
        .iter()
        .any(|p| p.extension().is_some_and(|e| e == ENTRY_EXTENSION))
}

/// Watches `<config dir>/sessions` for new, changed or removed registry files.
/// The directory must exist; the notify backend owns its event thread and
/// stops it when this is dropped.
#[derive(Debug)]
pub struct ClaudeRegistrySource {
    _watcher: RecommendedWatcher,
    wakes: Receiver<()>,
}

impl ClaudeRegistrySource {
    pub fn watch(sessions_dir: &Path) -> Result<Self, RegistrySourceError> {
        let (tx, wakes) = mpsc::sync_channel(WAKE_QUEUE_DEPTH);
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                // A watcher error (for example a queue overflow) may have lost
                // events, so it wakes the consumer like a change does.
                let relevant = res.as_ref().map_or(true, is_relevant);
                if relevant {
                    // Full means a wake is already queued; that covers this one.
                    let _ = tx.try_send(());
                }
            })
            .map_err(|source| RegistrySourceError::Create {
                dir: sessions_dir.to_owned(),
                source,
            })?;
        watcher
            .watch(sessions_dir, RecursiveMode::NonRecursive)
            .map_err(|source| RegistrySourceError::Watch {
                dir: sessions_dir.to_owned(),
                source,
            })?;
        Ok(Self {
            _watcher: watcher,
            wakes,
        })
    }
}

impl EventSource for ClaudeRegistrySource {
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

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn json_changes_are_relevant() {
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
        ] {
            assert!(is_relevant(&event(kind, "/s/42.json")), "{kind:?}");
        }
    }

    #[test]
    fn key_files_and_access_events_are_not_relevant() {
        assert!(!is_relevant(&event(
            EventKind::Create(CreateKind::File),
            "/s/42.abcd.key"
        )));
        assert!(!is_relevant(&event(
            EventKind::Access(AccessKind::Any),
            "/s/42.json"
        )));
    }
}
