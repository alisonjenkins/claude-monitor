//! Wakes the feed when a Claude Code transcript appears or changes.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::event::CreateKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::EventSource;

/// One queued wake is enough: it means "scan again", and more would only repeat it.
const WAKE_QUEUE_DEPTH: usize = 1;
const TRANSCRIPT_EXTENSION: &str = "jsonl";

#[derive(Debug, thiserror::Error)]
pub enum ClaudeSourceError {
    #[error("cannot create a file watcher for {}: {source}", dir.display())]
    Create { dir: PathBuf, source: notify::Error },
    #[error("cannot watch Claude projects directory {}: {source}", dir.display())]
    Watch { dir: PathBuf, source: notify::Error },
}

fn is_transcript(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == TRANSCRIPT_EXTENSION)
}

/// A new project directory wakes too: its first transcript can be written
/// before the recursive watch reaches the directory, and would be missed.
fn is_relevant(event: &notify::Event) -> bool {
    match event.kind {
        EventKind::Create(CreateKind::Folder) => true,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
            event.paths.iter().any(|p| is_transcript(p))
        }
        _ => false,
    }
}

/// Watches `<config dir>/projects` recursively for new or changed `*.jsonl`
/// transcripts. The directory must exist; the notify backend owns its event
/// thread and stops it when this is dropped.
#[derive(Debug)]
pub struct ClaudeProjectsSource {
    _watcher: RecommendedWatcher,
    wakes: Receiver<()>,
}

impl ClaudeProjectsSource {
    pub fn watch(projects_dir: &Path) -> Result<Self, ClaudeSourceError> {
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
            .map_err(|source| ClaudeSourceError::Create {
                dir: projects_dir.to_owned(),
                source,
            })?;
        watcher
            .watch(projects_dir, RecursiveMode::Recursive)
            .map_err(|source| ClaudeSourceError::Watch {
                dir: projects_dir.to_owned(),
                source,
            })?;
        Ok(Self {
            _watcher: watcher,
            wakes,
        })
    }
}

impl EventSource for ClaudeProjectsSource {
    fn wait(&mut self, timeout: Duration) -> bool {
        let woken = self.wakes.recv_timeout(timeout).is_ok();
        while self.wakes.try_recv().is_ok() {}
        woken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, ModifyKind, RemoveKind};

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn transcript_changes_are_relevant() {
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
        ] {
            assert!(is_relevant(&event(kind, "/p/-w/abc.jsonl")), "{kind:?}");
        }
    }

    #[test]
    fn other_files_and_access_events_are_not_relevant() {
        assert!(!is_relevant(&event(
            EventKind::Create(CreateKind::File),
            "/p/-w/notes.txt"
        )));
        assert!(!is_relevant(&event(
            EventKind::Access(AccessKind::Any),
            "/p/-w/abc.jsonl"
        )));
    }

    #[test]
    fn a_new_directory_is_relevant() {
        assert!(is_relevant(&event(
            EventKind::Create(CreateKind::Folder),
            "/p/-new"
        )));
    }
}
