use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;
use std::time::Duration;

use crate::desktop_notify;
use crate::session;
use crate::tmux;
use crate::ui::{self, App};
use crate::watcher;

const CLEANUP_INTERVAL_SECS: u64 = 30;

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

pub fn run() -> Result<()> {
    install_panic_hook();

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = run_app(&mut terminal);

    restore_terminal();

    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    let mut app = App::new();

    let (_watcher, watch_rx) = watcher::watch_status_dir()?;
    let status_dir = session::status_dir();

    let sessions = session::read_all_sessions(&status_dir);
    app.pane_locations = tmux::list_panes().ok().flatten().unwrap_or_default();
    app.update_sessions(sessions);

    let mut last_cleanup = std::time::Instant::now();
    let mut last_notified_count: usize = 0;

    loop {
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        let mut changed = false;
        while watch_rx.try_recv().is_ok() {
            changed = true;
        }

        if changed {
            let prev_count = app.sessions.len();
            let sessions = session::read_all_sessions(&status_dir);
            refresh_pane_locations(&mut app);
            app.update_sessions(sessions);

            let new_count = app.sessions.len();
            if prev_count == 0 && new_count > 0 && new_count != last_notified_count {
                if let Err(e) = desktop_notify::notify_attention(new_count) {
                    app.last_error = Some(format!("notification failed: {e:#}"));
                }
                last_notified_count = new_count;
            }
            if new_count == 0 {
                last_notified_count = 0;
            }
        }

        if last_cleanup.elapsed() > Duration::from_secs(CLEANUP_INTERVAL_SECS) {
            cleanup_dead_panes(&mut app, &status_dir);
            let sessions = session::read_all_sessions(&status_dir);
            app.update_sessions(sessions);
            last_cleanup = std::time::Instant::now();
        }

        match event::poll(Duration::from_millis(250)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if handle_key(key.code, key.modifiers, &mut app) {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => app.last_error = Some(format!("input error: {e}")),
            },
            Ok(false) => {}
            Err(e) => app.last_error = Some(format!("input poll error: {e}")),
        }
    }

    Ok(())
}

fn refresh_pane_locations(app: &mut App) {
    match tmux::list_panes() {
        Ok(Some(map)) => app.pane_locations = map,
        Ok(None) => {}
        Err(e) => app.last_error = Some(format!("tmux error: {e:#}")),
    }
}

/// Handle one key press. Returns `true` if the app should quit.
fn handle_key(code: KeyCode, modifiers: KeyModifiers, app: &mut App) -> bool {
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Enter => {
            if let Some(s) = app.selected_session().cloned() {
                if let Some(pane_id) = &s.tmux_pane {
                    match app.pane_location(pane_id).cloned() {
                        Some(loc) => {
                            if let Err(e) = tmux::switch_to_pane(&loc) {
                                app.last_error = Some(format!("switch failed: {e:#}"));
                            } else {
                                app.last_error = None;
                            }
                        }
                        None => app.last_error = Some("pane no longer exists".to_string()),
                    }
                }
            }
        }
        KeyCode::Char('d') => app.hide_selected(),
        _ => {}
    }
    false
}

/// Remove status files whose recorded tmux pane no longer exists. A no-op
/// if the tmux listing itself failed, so a transient tmux hiccup never
/// deletes live sessions' state. Failures removing a file are recorded in
/// the footer rather than propagated, so the dashboard keeps running.
fn cleanup_dead_panes(app: &mut App, status_dir: &std::path::Path) {
    let live_panes = match tmux::list_panes() {
        Ok(Some(map)) => map,
        Ok(None) => return,
        Err(e) => {
            app.last_error = Some(format!("tmux error: {e:#}"));
            return;
        }
    };

    for s in session::read_all_sessions(status_dir) {
        if let Some(pane) = &s.tmux_pane {
            if !live_panes.contains_key(pane) {
                if let Err(e) = session::remove_session(status_dir, &s.session_id) {
                    app.last_error = Some(format!("cleanup failed: {e:#}"));
                }
            }
        }
    }
}
