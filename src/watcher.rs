use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;

use crate::session;

/// True if `path` should be ignored — atomic-write temp files matching
/// `*.tmp.*`, which never represent a stable session state.
fn is_ignored(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.contains(".tmp."))
}

/// Create a file watcher on the status directory. Returns the watcher (must
/// be kept alive) and a receiver that yields one `()` per coalesced burst of
/// relevant filesystem events — callers should drain it and treat any
/// receipt as "refresh", not count events.
pub fn watch_status_dir() -> Result<(RecommendedWatcher, mpsc::Receiver<()>)> {
    let dir = session::status_dir();
    session::ensure_status_dir(&dir)?;

    let (tx, rx) = mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        let Ok(event) = res else {
            return;
        };
        let relevant = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) && event.paths.iter().any(|p| !is_ignored(p));
        if relevant {
            let _ = tx.send(());
        }
    })?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    Ok((watcher, rx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ignores_tmp_files() {
        assert!(is_ignored(Path::new("/x/foo.json.tmp.1234")));
    }

    #[test]
    fn does_not_ignore_json_files() {
        assert!(!is_ignored(Path::new("/x/foo.json")));
    }
}
