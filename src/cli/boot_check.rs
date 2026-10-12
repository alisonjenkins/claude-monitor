//! `claude-monitor boot-check [--banner]`: run at login, count the sessions a
//! reboot or crash interrupted, tell the user once and start nothing
//! (FR-019a, FR-032, FR-034).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::migrate::{open_viewer_store, store_error, store_error_message};
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::State;
use crate::store::sessions::SessionRow;
use crate::store::{resolve_state_dir, StateDirEnv};

/// Holds the banner line until a tmux client shows it; lives in the state dir.
const MARKER_FILE: &str = "boot-check-pending";
/// Ids of the interrupted sessions already announced, one per line.
const NOTIFIED_FILE: &str = "boot-check-notified";
const NOTIFICATION_SUMMARY: &str = "AI sessions interrupted";

pub fn run(env: &Env, state_dir: Option<&Path>, banner: bool) -> Result<Output, Error> {
    let dir =
        resolve_state_dir(state_dir, &StateDirEnv::from_process()).map_err(|e| store_error(&e))?;
    if banner {
        return show_banner(env, &dir);
    }
    let store = open_viewer_store(env, Some(&dir))?;
    let report = crate::refresh::refresh(&store, env, &env.clock.now());
    for warning in &report.warnings {
        eprintln!("claude-monitor: refresh: {warning}");
    }
    let rows = store
        .list_sessions_lenient()
        .map_err(|e| store_error_message("list sessions", Some(store.path()), &e))?;
    let interrupted_ids: BTreeSet<String> = rows
        .iter()
        .filter_map(|row| match row {
            SessionRow::Readable(s) if s.state == State::Interrupted && !s.dismissed => {
                Some(s.id.clone())
            }
            _ => None,
        })
        .collect();
    let interrupted = interrupted_ids.len();

    let marker = marker_path(&dir);
    let notified = notified_path(&dir);
    if interrupted == 0 {
        remove_marker(&marker)?;
        remove_marker(&notified)?;
    } else {
        // Only sessions not announced before are news; a login that finds the
        // same interrupted sessions must not repeat the notification.
        let previous = read_notified(&notified);
        if !interrupted_ids.is_subset(&previous) {
            let message = banner_message(interrupted);
            write_marker(&marker, &message)?;
            if let Err(error) = env.notifier.send(NOTIFICATION_SUMMARY, &message) {
                eprintln!("claude-monitor: boot-check: {error}");
            }
        }
        if interrupted_ids != previous {
            write_notified(&notified, &interrupted_ids)?;
        }
    }
    Ok(Output::Dual {
        text: if interrupted == 0 {
            String::new()
        } else {
            banner_message(interrupted)
        },
        json: json!({ "ok": true, "interrupted": interrupted }),
    })
}

/// Shows the pending line through tmux and clears it, so only the first
/// attaching client sees it. A failed display keeps the marker for the next one.
fn show_banner(env: &Env, dir: &Path) -> Result<Output, Error> {
    let marker = marker_path(dir);
    let message = match std::fs::read_to_string(&marker) {
        Ok(message) => message,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(banner_output(false)),
        Err(source) => {
            return Err(Error::Io {
                operation: "read boot-check marker".to_owned(),
                path: Some(marker),
                source,
            })
        }
    };
    let message = message.trim();
    if message.is_empty() {
        remove_marker(&marker)?;
        return Ok(banner_output(false));
    }
    env.tmux.display_message(message).map_err(|e| Error::Io {
        operation: "show boot-check banner".to_owned(),
        path: None,
        source: std::io::Error::other(e.to_string()),
    })?;
    remove_marker(&marker)?;
    Ok(banner_output(true))
}

fn banner_output(shown: bool) -> Output {
    Output::Dual {
        text: String::new(),
        json: json!({ "ok": true, "shown": shown }),
    }
}

fn banner_message(count: usize) -> String {
    let noun = if count == 1 { "session" } else { "sessions" };
    format!("{count} AI {noun} interrupted, prefix+A to review")
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MARKER_FILE)
}

fn notified_path(dir: &Path) -> PathBuf {
    dir.join(NOTIFIED_FILE)
}

/// The ids last announced; empty when never written or unreadable, which at
/// worst repeats one notification.
fn read_notified(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn write_notified(path: &Path, ids: &BTreeSet<String>) -> Result<(), Error> {
    let text: String = ids.iter().map(|id| format!("{id}\n")).collect();
    std::fs::write(path, text).map_err(|source| Error::Io {
        operation: "write boot-check notified set".to_owned(),
        path: Some(path.to_owned()),
        source,
    })
}

fn write_marker(path: &Path, message: &str) -> Result<(), Error> {
    std::fs::write(path, message).map_err(|source| Error::Io {
        operation: "write boot-check marker".to_owned(),
        path: Some(path.to_owned()),
        source,
    })
}

fn remove_marker(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io {
            operation: "remove boot-check marker".to_owned(),
            path: Some(path.to_owned()),
            source,
        }),
    }
}
