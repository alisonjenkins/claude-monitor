//! Keeping the store current while a viewer runs: wake sources, a throttled
//! [`refresh`] per wake, and the process-exit registrations that make a dead
//! agent show up at once.
//!
//! The loop takes its sources as a list, so tests drive it with fakes and the
//! binary hands it the real watchers from [`build_sources`].

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::agents::{claude_discover, claude_registry, copilot_discover};
use crate::env::Env;
use crate::feed::{
    BoundFilesSource, ClaudeProjectsSource, ClaudeRegistrySource, CopilotSessionsSource,
    EventSource, MergedSource, Monotonic, ProcExitError, ProcExitHandle, ProcExitSource,
    SafetyNetSource, SystemMonotonic, Throttle, SAFETY_NET_INTERVAL, THROTTLE_WINDOW,
};
use crate::model::Session;
use crate::refresh::{refresh, RefreshReport};
use crate::store::sessions::SessionRow;
use crate::store::Store;

/// Longest the loop blocks without rechecking `stop`.
const STOP_CHECK_SLICE: Duration = Duration::from_millis(100);

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
pub fn build_sources(env: &Env) -> LiveSources {
    let mut built = LiveSources {
        sources: Vec::new(),
        proc_exit: None,
        warnings: Vec::new(),
    };
    watch_dir(
        &mut built,
        env,
        "claude registry",
        claude_registry::sessions_dir(env).map_err(|e| e.to_string()),
        |dir| ClaudeRegistrySource::watch(dir).map(boxed),
    );
    watch_dir(
        &mut built,
        env,
        "claude projects",
        claude_discover::projects_dir(env).map_err(|e| e.to_string()),
        |dir| ClaudeProjectsSource::watch(dir).map(boxed),
    );
    watch_dir(
        &mut built,
        env,
        "copilot sessions",
        copilot_discover::session_state_dir(env).map_err(|e| e.to_string()),
        |dir| CopilotSessionsSource::watch(dir).map(boxed),
    );
    match ProcExitSource::new() {
        Ok(source) => {
            built.proc_exit = Some(source.handle());
            built.sources.push(Box::new(source));
        }
        Err(ProcExitError::Unsupported) => {}
        Err(e) => built.warnings.push(format!("process exits: {e}")),
    }
    built.sources.push(Box::new(SafetyNetSource::new(
        SAFETY_NET_INTERVAL,
        SystemMonotonic,
    )));
    built
}

/// [`build_sources`] plus a watch on the plan files `store`'s progress
/// bindings read; bindings made later are picked up while it runs. A file
/// that cannot be watched is skipped with a warning.
pub fn build_sources_with_files(env: &Env, store: &Store) -> LiveSources {
    let mut built = build_sources(env);
    match BoundFilesSource::watch(store.path()) {
        Ok(source) => {
            built.warnings.extend(source.warnings().iter().cloned());
            built.sources.push(Box::new(source));
        }
        Err(e) => built.warnings.push(format!("plan files: {e}")),
    }
    built
}

fn boxed<S: EventSource + Send + 'static>(source: S) -> Box<dyn EventSource + Send> {
    Box::new(source)
}

fn watch_dir<E: std::fmt::Display>(
    built: &mut LiveSources,
    env: &Env,
    label: &str,
    dir: Result<std::path::PathBuf, String>,
    watch: impl FnOnce(&Path) -> Result<Box<dyn EventSource + Send>, E>,
) {
    let dir = match dir {
        Ok(dir) => dir,
        Err(e) => {
            built.warnings.push(format!("{label}: {e}"));
            return;
        }
    };
    if !env.fs.exists(&dir) {
        built
            .warnings
            .push(format!("{label}: {} does not exist", dir.display()));
        return;
    }
    match watch(&dir) {
        Ok(source) => built.sources.push(source),
        Err(e) => built.warnings.push(format!("{label}: {e}")),
    }
}

/// Pid to start time of every active session whose process is known.
pub fn tracked_processes(sessions: &[Session]) -> BTreeMap<u32, u64> {
    sessions
        .iter()
        .filter(|session| session.state.is_active())
        .filter_map(|session| Some((session.pid?, session.pid_start?)))
        .collect()
}

/// Waits on `sources`; a wake, coalesced over the feed's start-anchored
/// window, runs one refresh and then `on_refresh`. Returns once `stop` is set.
pub fn run_live(
    store: &Store,
    env: &Env,
    sources: Vec<Box<dyn EventSource + Send>>,
    proc_exit: Option<&ProcExitHandle>,
    stop: &AtomicBool,
    on_refresh: &mut dyn FnMut(&RefreshReport),
) {
    let mut merged = MergedSource::new(sources);
    let mut throttle = Throttle::new(THROTTLE_WINDOW);
    let clock = SystemMonotonic;
    let mut registered = BTreeMap::new();
    if let Some(handle) = proc_exit {
        sync_registrations(store, handle, &mut registered);
    }
    while !stop.load(Ordering::Acquire) {
        let timeout = throttle.deadline().map_or(STOP_CHECK_SLICE, |deadline| {
            deadline
                .saturating_duration_since(clock.now())
                .min(STOP_CHECK_SLICE)
        });
        if merged.wait(timeout) {
            throttle.wake(clock.now());
        }
        if throttle.poll(clock.now()) {
            let report = refresh(store, env, &env.clock.now());
            if let Some(handle) = proc_exit {
                sync_registrations(store, handle, &mut registered);
            }
            on_refresh(&report);
        }
    }
}

/// A [`run_live`] loop on its own thread; dropping it stops and joins the loop.
#[derive(Debug)]
pub struct LiveThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

/// Runs [`run_live`] on a new thread that owns `store`.
pub fn spawn_live(
    store: Store,
    env: Env,
    sources: Vec<Box<dyn EventSource + Send>>,
    proc_exit: Option<ProcExitHandle>,
    mut on_refresh: impl FnMut(&RefreshReport) + Send + 'static,
) -> std::io::Result<LiveThread> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("live-refresh".to_owned())
        .spawn(move || {
            run_live(
                &store,
                &env,
                sources,
                proc_exit.as_ref(),
                &flag,
                &mut on_refresh,
            );
        })?;
    Ok(LiveThread {
        stop,
        handle: Some(handle),
    })
}

impl Drop for LiveThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            // A panicked loop has nothing left to report here.
            let _ = handle.join();
        }
    }
}

/// Makes the watched processes match the sessions that should be running. An
/// exit needs no payload: the refresh that follows the wake sees it.
fn sync_registrations(store: &Store, handle: &ProcExitHandle, registered: &mut BTreeMap<u32, u64>) {
    let Ok(rows) = store.list_sessions_lenient() else {
        return;
    };
    let sessions: Vec<Session> = rows
        .into_iter()
        .filter_map(|row| match row {
            SessionRow::Readable(session) => Some(*session),
            SessionRow::Corrupt { .. } => None,
        })
        .collect();
    let wanted = tracked_processes(&sessions);
    let gone: Vec<u32> = registered
        .keys()
        .filter(|pid| !wanted.contains_key(pid))
        .copied()
        .collect();
    for pid in gone {
        handle.unregister(pid);
        registered.remove(&pid);
    }
    for (pid, start) in wanted {
        if registered.get(&pid) != Some(&start) && handle.register(pid, start).is_ok() {
            registered.insert(pid, start);
        }
    }
    handle.drain_exited();
}
