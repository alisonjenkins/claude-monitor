//! `claude-monitor watch`: the live refresh loop with no screen, logging one
//! line per session state change.
//!
//! Each line is `<ISO8601 UTC> <agent> <short id> <state> (was <state>|new) <cwd>`.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::AtomicBool;

use crate::env::Env;
use crate::live::LiveSources;
use crate::model::{Session, State};
use crate::store::Store;

/// Lines for every session whose state differs from `previous`, which is then
/// brought up to date. The `first` pass reports only active sessions: the
/// finished history is not news.
pub fn state_changes(
    _previous: &mut BTreeMap<String, State>,
    _sessions: &[Session],
    _at: &str,
    _first: bool,
) -> Vec<String> {
    Vec::new()
}

/// Refreshes on every wake of `built`'s sources and writes a line per state
/// change to `out` and per new warning to `err`, until `stop` is set or `out`
/// can no longer be written.
pub fn watch(
    _store: &Store,
    _env: &Env,
    _built: LiveSources,
    _stop: &AtomicBool,
    _out: &mut dyn Write,
    _err: &mut dyn Write,
) {
}
