//! Wakes the feed when a Copilot CLI session directory appears or changes.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::event::CreateKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::EventSource;

/// One queued wake is enough: it means "scan again", and more would only repeat it.
const WAKE_QUEUE_DEPTH: usize = 1;
const WORKSPACE_FILE: &str = "workspace.yaml";
const EVENTS_FILE: &str = "events.jsonl";

#[derive(Debug, thiserror::Error)]
pub enum CopilotSourceError {
    #[error("cannot create a file watcher for {}: {source}", dir.display())]
    Create { dir: PathBuf, source: notify::Error },
    #[error("cannot watch Copilot session-state directory {}: {source}", dir.display())]
    Watch { dir: PathBuf, source: notify::Error },
}

fn is_session_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == WORKSPACE_FILE || name == EVENTS_FILE)
}

/// A new session directory wakes too: its first files can be written before
/// the recursive watch reaches the directory, and would be missed.
fn is_relevant(event: &notify::Event) -> bool {
    match event.kind {
        EventKind::Create(CreateKind::Folder) => true,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
            event.paths.iter().any(|p| is_session_file(p))
        }
        _ => false,
    }
}

/// Watches `<copilot home>/session-state` recursively for new directories and
/// new or changed `workspace.yaml` and `events.jsonl` files. The directory must
/// exist; the notify backend owns its event thread and stops it when this is
/// dropped.
#[derive(Debug)]
pub struct CopilotSessionsSource {
    _watcher: RecommendedWatcher,
    wakes: Receiver<()>,
}

impl CopilotSessionsSource {
    pub fn watch(session_state_dir: &Path) -> Result<Self, CopilotSourceError> {
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
            .map_err(|source| CopilotSourceError::Create {
                dir: session_state_dir.to_owned(),
                source,
            })?;
        watcher
            .watch(session_state_dir, RecursiveMode::Recursive)
            .map_err(|source| CopilotSourceError::Watch {
                dir: session_state_dir.to_owned(),
                source,
            })?;
        Ok(Self {
            _watcher: watcher,
            wakes,
        })
    }
}

impl EventSource for CopilotSessionsSource {
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
    fn workspace_and_event_file_changes_are_relevant() {
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
        ] {
            for path in ["/s/abc/workspace.yaml", "/s/abc/events.jsonl"] {
                assert!(is_relevant(&event(kind, path)), "{kind:?} {path}");
            }
        }
    }

    #[test]
    fn other_files_and_access_events_are_not_relevant() {
        for path in [
            "/s/abc/.workspace-fork.lock",
            "/s/abc/checkpoints/index.md",
            "/s/abc/files/events.jsonl.bak",
        ] {
            assert!(
                !is_relevant(&event(EventKind::Create(CreateKind::File), path)),
                "{path}"
            );
        }
        assert!(!is_relevant(&event(
            EventKind::Access(AccessKind::Any),
            "/s/abc/events.jsonl"
        )));
    }

    #[test]
    fn a_new_directory_is_relevant() {
        assert!(is_relevant(&event(
            EventKind::Create(CreateKind::Folder),
            "/s/new"
        )));
    }
}
