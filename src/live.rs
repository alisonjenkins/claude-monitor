//! Keeping the store current while a viewer runs: wake sources, a throttled
//! [`refresh`](crate::refresh::refresh) per wake, and the process-exit
//! registrations that make a dead agent show up at once.
//!
//! The loop takes its sources as a list, so tests drive it with fakes and the
//! binary hands it the real watchers from [`build_sources`].

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use crate::env::Env;
use crate::feed::{EventSource, ProcExitHandle};
use crate::model::Session;
use crate::refresh::RefreshReport;
use crate::store::Store;

/// The real sources for this machine, and what could not be set up.
pub struct LiveSources {
    pub sources: Vec<Box<dyn EventSource + Send>>,
    /// Present when process-exit watching is available; the loop registers the
    /// pids of live sessions on it.
    pub proc_exit: Option<ProcExitHandle>,
    /// One line per source that was skipped; never fatal.
    pub warnings: Vec<String>,
}

/// Watchers for the Claude registry, Claude transcripts and Copilot session
/// state, plus process exits and the safety net. A directory that is missing
/// or cannot be watched is skipped with a warning.
pub fn build_sources(_env: &Env) -> LiveSources {
    LiveSources {
        sources: Vec::new(),
        proc_exit: None,
        warnings: Vec::new(),
    }
}

/// Pid to start time of every active session whose process is known.
pub fn tracked_processes(_sessions: &[Session]) -> BTreeMap<u32, u64> {
    BTreeMap::new()
}

/// Waits on `sources`; a wake, coalesced over the feed's start-anchored
/// window, runs one refresh and then `on_refresh`. Returns once `stop` is set.
pub fn run_live(
    _store: &Store,
    _env: &Env,
    _sources: Vec<Box<dyn EventSource + Send>>,
    _proc_exit: Option<&ProcExitHandle>,
    _stop: &AtomicBool,
    _on_refresh: &mut dyn FnMut(&RefreshReport),
) {
}
