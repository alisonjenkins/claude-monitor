use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;

use crate::session;

/// Events from the file watcher
pub enum WatchEvent {
    /// A status file was created or modified
    Changed,
    /// A status file was removed
    Removed,
}

/// Create a file watcher on the status directory.
/// Returns the watcher (must be kept alive) and a receiver for events.
pub fn watch_status_dir() -> Result<(RecommendedWatcher, mpsc::Receiver<WatchEvent>)> {
    let dir = session::status_dir();
    std::fs::create_dir_all(&dir)?;

    let (tx, rx) = mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            let watch_event = match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) => Some(WatchEvent::Changed),
                EventKind::Remove(_) => Some(WatchEvent::Removed),
                _ => None,
            };
            if let Some(e) = watch_event {
                let _ = tx.send(e);
            }
        }
    })?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    Ok((watcher, rx))
}
