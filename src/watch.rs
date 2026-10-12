//! `claude-monitor watch`: the live refresh loop with no screen, logging one
//! line per session state change.
//!
//! Each line is `<ISO8601 UTC> <agent> <short id> <state> (was <state>|new) <cwd>`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::env::Env;
use crate::live::{run_live, LiveSources};
use crate::model::{Session, State};
use crate::notify::{self, is_notifiable};
use crate::refresh::{refresh, RefreshReport};
use crate::store::Store;

const SHORT_ID_CHARS: usize = 8;

/// Lines for every session whose state differs from `previous`, which is then
/// brought up to date. The `first` pass reports only active sessions: the
/// finished history is not news.
pub fn state_changes(
    previous: &mut BTreeMap<String, State>,
    sessions: &[Session],
    at: &str,
    first: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    for session in sessions {
        let before = previous.insert(session.id.clone(), session.state);
        if before == Some(session.state) || (first && !session.state.is_active()) {
            continue;
        }
        let detail = before.map_or_else(|| "new".to_owned(), |state| format!("was {state}"));
        let id: String = session.id.chars().take(SHORT_ID_CHARS).collect();
        lines.push(format!(
            "{at} {} {id} {} ({detail}) {}",
            session.agent_kind, session.state, session.cwd
        ));
    }
    lines
}

/// The `(project, state)` of every session that just changed into a state
/// worth a desktop notification, by the dashboard's rules. Read-only: call it
/// before [`state_changes`] brings `previous` up to date. The `first` pass
/// notifies nothing: sessions that already exist are not news.
pub fn notifications(
    previous: &BTreeMap<String, State>,
    sessions: &[Session],
    first: bool,
) -> Vec<(String, State)> {
    if first {
        return Vec::new();
    }
    sessions
        .iter()
        .filter(|session| is_notifiable(session.state))
        .filter(|session| previous.get(&session.id) != Some(&session.state))
        .map(|session| (project_name(session), session.state))
        .collect()
}

/// The repository name, else the last part of the working directory.
fn project_name(session: &Session) -> String {
    [session.repo_root.as_deref(), Some(session.cwd.as_str())]
        .into_iter()
        .flatten()
        .find_map(|path| Path::new(path).file_name())
        .map_or_else(
            || session.id.clone(),
            |name| name.to_string_lossy().into_owned(),
        )
}

/// Refreshes on every wake of `built`'s sources and writes a line per state
/// change to `out` and per new warning to `err`, until `stop` is set or `out`
/// can no longer be written.
pub fn watch(
    store: &Store,
    env: &Env,
    built: LiveSources,
    stop: &AtomicBool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) {
    let mut previous = BTreeMap::new();
    let mut last_warning: Option<String> = None;
    let mut first = true;
    let mut report_pass = |report: &RefreshReport| {
        let warning = report.warnings.join("; ");
        if !warning.is_empty() && last_warning.as_deref() != Some(warning.as_str()) {
            let _ = writeln!(err, "warning: {warning}");
        }
        last_warning = Some(warning);
        let Ok(sessions) = store.list_sessions() else {
            return;
        };
        for (project, state) in notifications(&previous, &sessions, first) {
            if let Err(error) = env
                .notifier
                .send(notify::SUMMARY, &notify::body(&project, state))
            {
                let _ = writeln!(err, "warning: notification failed: {error}");
            }
        }
        for line in state_changes(&mut previous, &sessions, &env.clock.now(), first) {
            if writeln!(out, "{line}").is_err() {
                stop.store(true, Ordering::Release);
                return;
            }
        }
        if out.flush().is_err() {
            stop.store(true, Ordering::Release);
        }
        first = false;
    };

    let mut startup = RefreshReport {
        warnings: built.warnings.clone(),
        ..RefreshReport::default()
    };
    let initial = refresh(store, env, &env.clock.now());
    startup.warnings.extend(initial.warnings);
    report_pass(&startup);
    run_live(
        store,
        env,
        built.sources,
        built.proc_exit.as_ref(),
        stop,
        &mut |report| {
            let mut report = report.clone();
            report.warnings.splice(0..0, built.warnings.iter().cloned());
            report_pass(&report);
        },
    );
}
