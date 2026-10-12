//! The interactive dashboard: terminal setup and the loop that feeds
//! [`update`] keys and store changes and carries out the effects it returns.

use std::collections::VecDeque;
use std::io::stdout;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;

use super::app::{update, App, Effect, Msg, Snapshot};
use super::view::{state_label, view};
use crate::cli::migrate::open_viewer_store;
use crate::env::Env;
use crate::feed::{ChangeFeed, EventSource, FsWatcherSource, Monotonic, SystemMonotonic};
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

    install_panic_hook();
    enable_raw_mode().context("cannot enter raw mode")?;
    stdout()
        .execute(EnterAlternateScreen)
        .context("cannot enter the alternate screen")?;
    let result = Terminal::new(CrosstermBackend::new(stdout()))
        .context("cannot start the terminal")
        .and_then(|mut terminal| run_loop(&mut terminal, env, &store, feed));
    restore_terminal();
    result
}

fn run_loop<S: EventSource>(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    env: &Env,
    store: &Store,
    mut feed: ChangeFeed<S, SystemMonotonic>,
) -> Result<()> {
    let (mut app, _) = reload(App::new(), store, &env.clock.now());
    let mut last_reload = Instant::now();

    loop {
        terminal.draw(|frame| view(&app, frame))?;

        let mut queue: VecDeque<Msg> = VecDeque::new();
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
        let (next, mut effects) = if last_reload.elapsed() >= SAFETY_NET {
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
            match perform(&effect, env) {
                Done::Quit => return Ok(()),
                Done::Failed(message) => {
                    app = update(app, Msg::Failed(message)).0;
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
}

fn perform(effect: &Effect, env: &Env) -> Done {
    match effect {
        Effect::Quit => Done::Quit,
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
