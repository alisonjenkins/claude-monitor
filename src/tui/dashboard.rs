//! The interactive dashboard: terminal setup and the loop that feeds
//! [`update`] keys and store changes and carries out the effects it returns.

use std::collections::VecDeque;
use std::io::stdout;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;

use super::app::{update, App, Effect, Msg, Snapshot};
use super::restore::{preview_restore, run_restore, RestoreResult};
use super::view::{state_label, view};
use crate::cli::detail::SessionDetail;
use crate::cli::migrate::open_viewer_store;
use crate::env::Env;
use crate::feed::{ChangeFeed, EventSource, FsWatcherSource, Monotonic, SystemMonotonic};
use crate::live::{build_sources_with_files, spawn_live, LiveThread};
use crate::respawn::SPAWN_FAILED;
use crate::store::{Store, StoreOptions};
use crate::tmux;

/// How often the loop looks for a key and a feed wake. The feed itself is
/// event-driven; this only bounds how long a wake waits to be noticed.
const TICK: Duration = Duration::from_millis(100);

/// Reload even without a wake, in case the file watcher lost one.
const SAFETY_NET: Duration = Duration::from_secs(5);

const NOTIFICATION_SUMMARY: &str = "Claude Code";

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);
    let _ = stdout().execute(crossterm::cursor::Show);
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
}

/// Reads the store and runs the result through `update`.
pub fn reload(app: App, store: &Store, now: &str) -> (App, Vec<Effect>) {
    match Snapshot::load(store, now) {
        Ok(snapshot) => update(app, Msg::Refreshed(snapshot)),
        Err(error) => update(app, Msg::Failed(format!("cannot read sessions: {error}"))),
    }
}

/// Polls the feed once; when it fired, reloads the overview. Never blocks.
pub fn pump<S: EventSource, C: Monotonic>(
    app: App,
    feed: &mut ChangeFeed<S, C>,
    store: &Store,
    now: &str,
) -> (App, Vec<Effect>) {
    match feed.poll() {
        Ok(rows) if rows.is_empty() => (app, Vec::new()),
        Ok(_) => reload(app, store, now),
        Err(error) => update(app, Msg::Failed(format!("change feed: {error}"))),
    }
}

pub fn run(env: &Env, state_dir: Option<&Path>) -> Result<()> {
    let store = open_viewer_store(env, state_dir).map_err(|e| anyhow!("{e}"))?;
    let dir = store
        .path()
        .parent()
        .ok_or_else(|| anyhow!("database path {} has no directory", store.path().display()))?
        .to_owned();
    let feed_store = Store::open(store.path(), &StoreOptions::viewer())
        .map_err(|e| anyhow!("cannot open the change feed: {e}"))?;
    let source = FsWatcherSource::watch(&dir).map_err(|e| anyhow!("{e}"))?;
    let feed = ChangeFeed::new(feed_store, source, SystemMonotonic)
        .map_err(|e| anyhow!("cannot start the change feed: {e}"))?;

    let (signals, live_rx) = mpsc::channel();
    let _live = start_live(env, &store, signals)?;

    install_panic_hook();
    enable_raw_mode().context("cannot enter raw mode")?;
    stdout()
        .execute(EnterAlternateScreen)
        .context("cannot enter the alternate screen")?;
    let result = Terminal::new(CrosstermBackend::new(stdout()))
        .context("cannot start the terminal")
        .and_then(|mut terminal| run_loop(&mut terminal, env, &store, feed, &live_rx));
    restore_terminal();
    result
}

/// What the live refresh thread tells the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveSignal {
    /// A refresh changed the store: read it again and redraw.
    Changed,
    /// A source is missing or failing; one line for the footer.
    Warning(String),
}

/// One footer line for the warnings of a refresh or of setting up the sources.
pub fn warning_line(warnings: &[String]) -> Option<String> {
    let (first, rest) = warnings.split_first()?;
    Some(if rest.is_empty() {
        format!("warning: {first}")
    } else {
        format!("warning: {first} (+{} more)", rest.len())
    })
}

fn start_live(env: &Env, store: &Store, signals: mpsc::Sender<LiveSignal>) -> Result<LiveThread> {
    let built = build_sources_with_files(env, store);
    if let Some(line) = warning_line(&built.warnings) {
        let _ = signals.send(LiveSignal::Warning(line));
    }
    let live_store = Store::open(store.path(), &StoreOptions::viewer())
        .map_err(|e| anyhow!("cannot open the live refresh store: {e}"))?;
    spawn_live(
        live_store,
        env.clone(),
        built.sources,
        built.proc_exit,
        move |report| {
            if let Some(line) = warning_line(&report.warnings) {
                let _ = signals.send(LiveSignal::Warning(line));
            }
            if report.changed() {
                let _ = signals.send(LiveSignal::Changed);
            }
        },
    )
    .context("cannot start the live refresh thread")
}

fn run_loop<S: EventSource>(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    env: &Env,
    store: &Store,
    mut feed: ChangeFeed<S, SystemMonotonic>,
    live_rx: &mpsc::Receiver<LiveSignal>,
) -> Result<()> {
    let (mut app, _) = reload(App::new(), store, &env.clock.now());
    let mut last_reload = Instant::now();
    let (restore_tx, restore_rx) = mpsc::channel::<Msg>();

    loop {
        terminal.draw(|frame| view(&app, frame))?;

        let mut queue: VecDeque<Msg> = VecDeque::new();
        let mut live_changed = false;
        while let Ok(signal) = live_rx.try_recv() {
            match signal {
                LiveSignal::Changed => live_changed = true,
                LiveSignal::Warning(line) => queue.push_back(Msg::Failed(line)),
            }
        }
        while let Ok(msg) = restore_rx.try_recv() {
            queue.push_back(msg);
        }
        match event::poll(TICK) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    queue.push_back(Msg::Key(key.code, key.modifiers));
                }
                Ok(_) => {}
                Err(e) => queue.push_back(Msg::Failed(format!("input error: {e}"))),
            },
            Ok(false) => {}
            Err(e) => queue.push_back(Msg::Failed(format!("input poll error: {e}"))),
        }

        let now = env.clock.now();
        let (next, mut effects) = if live_changed || last_reload.elapsed() >= SAFETY_NET {
            last_reload = Instant::now();
            reload(app, store, &now)
        } else {
            pump(app, &mut feed, store, &now)
        };
        app = next;
        while let Some(msg) = queue.pop_front() {
            let (next, more) = update(app, msg);
            app = next;
            effects.extend(more);
        }

        for effect in effects {
            match perform(&effect, env, store, &restore_tx) {
                Done::Quit => return Ok(()),
                Done::Failed(message) => {
                    app = update(app, Msg::Failed(message)).0;
                }
                Done::Answer(msg) => {
                    app = update(app, msg).0;
                }
                Done::Ok => {}
            }
        }
    }
}

enum Done {
    Ok,
    Quit,
    Failed(String),
    Answer(Msg),
}

/// Starts run off the UI thread so a slow `git worktree add` or tmux call
/// never freezes the redraw; each result comes back as it finishes.
fn start_restore(env: &Env, store: &Store, ids: Vec<String>, results: &mpsc::Sender<Msg>) -> Done {
    let path = store.path().to_owned();
    let env = env.clone();
    let tx = results.clone();
    let worker_ids = ids.clone();
    let spawned = std::thread::Builder::new()
        .name("restore".to_owned())
        .spawn(move || match Store::open(&path, &StoreOptions::viewer()) {
            Ok(store) => run_restore(&store, &env, &worker_ids, &mut |result| {
                let _ = tx.send(Msg::RestoreResult(result));
            }),
            Err(error) => refuse_all(&worker_ids, &format!("cannot open the store: {error}"), &tx),
        });
    match spawned {
        Ok(_) => Done::Ok,
        Err(error) => {
            let detail = format!("cannot start a worker: {error}");
            refuse_all(&ids, &detail, results);
            Done::Failed(detail)
        }
    }
}

fn refuse_all(ids: &[String], detail: &str, results: &mpsc::Sender<Msg>) {
    for id in ids {
        let _ = results.send(Msg::RestoreResult(RestoreResult::Refused {
            session_id: id.clone(),
            code: SPAWN_FAILED.to_owned(),
            message: detail.to_owned(),
        }));
    }
}

fn load_detail(env: &Env, store: &Store, id: &str) -> Done {
    let session = match store.get_session(id) {
        Ok(Some(session)) => session,
        Ok(None) => return Done::Failed(format!("session {id} no longer exists")),
        Err(e) => return Done::Failed(format!("cannot read session {id}: {e}")),
    };
    match SessionDetail::load(store, env, &session) {
        Ok(detail) => Done::Answer(Msg::Detail(detail)),
        Err(e) => Done::Failed(format!("cannot read the detail of {id}: {e}")),
    }
}

fn perform(effect: &Effect, env: &Env, store: &Store, restore_tx: &mpsc::Sender<Msg>) -> Done {
    match effect {
        Effect::Quit => Done::Quit,
        Effect::PreviewRestore { ids } => {
            Done::Answer(Msg::RestorePreview(preview_restore(store, env, ids)))
        }
        Effect::StartRestore { ids } => start_restore(env, store, ids.clone(), restore_tx),
        Effect::LoadDetail { id } => load_detail(env, store, id),
        Effect::CheckLinks { .. } => Done::Ok,
        Effect::JumpToPane { pane } => match tmux::switch_to_pane(pane) {
            Ok(()) => Done::Ok,
            Err(e) => Done::Failed(format!("switch failed: {e:#}")),
        },
        Effect::Notify { project, state } => {
            let body = format!("{project}: {}", state_label(*state));
            match env.notifier.send(NOTIFICATION_SUMMARY, &body) {
                Ok(()) => Done::Ok,
                Err(e) => Done::Failed(format!("notification failed: {e}")),
            }
        }
    }
}
