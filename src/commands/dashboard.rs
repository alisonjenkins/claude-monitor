use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
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

pub fn run() -> Result<()> {
    // Set up terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = run_app(&mut terminal);

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    let mut app = App::new();

    // Start file watcher
    let (_watcher, watch_rx) = watcher::watch_status_dir()?;

    let status_dir = session::status_dir();

    // Initial load
    let sessions = session::read_all_sessions(&status_dir);
    app.pane_locations = tmux::list_panes().ok().flatten().unwrap_or_default();
    app.update_sessions(sessions);

    let mut last_cleanup = std::time::Instant::now();
    let mut last_notified_count: usize = 0;

    loop {
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        // Check for file watcher events (non-blocking)
        let mut changed = false;
        while watch_rx.try_recv().is_ok() {
            changed = true;
        }

        if changed {
            let prev_count = app.sessions.len();
            let sessions = session::read_all_sessions(&status_dir);
            app.pane_locations = tmux::list_panes().ok().flatten().unwrap_or_default();
            app.update_sessions(sessions);

            // Send desktop notification when sessions go from 0 to >0
            let new_count = app.sessions.len();
            if prev_count == 0 && new_count > 0 && new_count != last_notified_count {
                let _ = desktop_notify::notify_attention(new_count);
                last_notified_count = new_count;
            }
            if new_count == 0 {
                last_notified_count = 0;
            }
        }

        // Periodic cleanup: remove sessions whose tmux panes no longer exist
        if last_cleanup.elapsed() > Duration::from_secs(CLEANUP_INTERVAL_SECS) {
            cleanup_dead_panes(&status_dir)?;
            let sessions = session::read_all_sessions(&status_dir);
            app.update_sessions(sessions);
            last_cleanup = std::time::Instant::now();
        }

        // Handle keyboard input with timeout
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => app.next(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous(),
                    KeyCode::Enter => {
                        if let Some(s) = app.selected_session().cloned() {
                            if let Some(pane_id) = &s.tmux_pane {
                                if let Some(loc) = app.pane_location(pane_id).cloned() {
                                    let _ = tmux::switch_to_pane(&loc);
                                }
                            }
                        }
                    }
                    KeyCode::Char('d') => app.hide_selected(),
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

/// Remove status files for tmux panes that no longer exist. A no-op if the
/// tmux listing itself failed, so a transient tmux hiccup never deletes
/// live sessions' state.
fn cleanup_dead_panes(status_dir: &std::path::Path) -> Result<()> {
    let Some(live_panes) = tmux::list_panes()? else {
        return Ok(());
    };

    for s in session::read_all_sessions(status_dir) {
        if let Some(pane) = &s.tmux_pane {
            if !live_panes.contains_key(pane) {
                session::remove_session(status_dir, &s.session_id)?;
            }
        }
    }

    Ok(())
}
