use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::collections::HashMap;
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
    app.pane_locations = tmux::list_panes().unwrap_or_default().unwrap_or_default();
    let mut prev_states = desktop_notify::state_snapshot(&sessions);
    app.update_sessions(sessions);

    let mut last_cleanup = std::time::Instant::now();

    loop {
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        let mut changed = false;
        while watch_rx.try_recv().is_ok() {
            changed = true;
        }

        if changed {
            refresh(&mut app, &status_dir, &mut prev_states);
        }

        if last_cleanup.elapsed() > Duration::from_secs(CLEANUP_INTERVAL_SECS) {
            prune_stale_panes(&mut app, &status_dir);
            refresh(&mut app, &status_dir, &mut prev_states);
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

/// Reload sessions, resolve pane locations, fire notifications for new
/// transitions into an attention state, and update `app`. Returns via
/// `app.last_error` on failure rather than propagating, so the loop keeps
/// running.
fn refresh(
    app: &mut App,
    status_dir: &std::path::Path,
    prev_states: &mut HashMap<String, crate::session::SessionState>,
) {
    let sessions = session::read_all_sessions(status_dir);

    match tmux::list_panes() {
        Ok(Some(map)) => app.pane_locations = map,
        Ok(None) => {}
        Err(e) => app.last_error = Some(format!("tmux error: {e:#}")),
    }

    for s in desktop_notify::transitions_into_attention(prev_states, &sessions) {
        if let Err(e) = desktop_notify::notify_transition(s.project_name(), s.state) {
            app.last_error = Some(format!("notification failed: {e:#}"));
        }
    }
    *prev_states = desktop_notify::state_snapshot(&sessions);

    app.update_sessions(sessions);
}

/// Handle one key press. Returns `true` if the app should quit.
fn handle_key(code: KeyCode, modifiers: KeyModifiers, app: &mut App) -> bool {
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }
    // Ctrl-D reaching the pane (e.g. an EOF on attach) must not act as `d`.
    if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return false;
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Enter => {
            if let Some(s) = app.selected_session().cloned() {
                if let Some(pane_id) = &s.tmux_pane {
                    match app.pane_location(pane_id).cloned() {
                        Some(_) => {
                            if let Err(e) = tmux::switch_to_pane(pane_id) {
                                app.last_error = Some(format!("switch failed: {e:#}"));
                            } else {
                                app.last_error = None;
                            }
                        }
                        None => {
                            app.last_error = Some("pane no longer exists".to_string());
                        }
                    }
                }
            }
        }
        KeyCode::Char('d') => {
            app.hide_selected();
        }
        _ => {}
    }
    false
}

/// Remove status files whose recorded tmux pane no longer exists. A no-op
/// if the tmux listing itself failed (tmux not running), so a transient
/// tmux hiccup never deletes live sessions' state.
fn prune_stale_panes(app: &mut App, status_dir: &std::path::Path) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionState, SessionStatus};

    fn app_with_one_session() -> App {
        let mut app = App::new();
        app.update_sessions(vec![SessionStatus {
            session_id: "s1".to_string(),
            state: SessionState::Idle,
            cwd: "/tmp/s1".to_string(),
            tmux_pane: None,
            since: chrono::Utc::now(),
        }]);
        app
    }

    #[test]
    fn ctrl_letter_does_not_trigger_plain_binding() {
        let mut app = app_with_one_session();
        assert!(!handle_key(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            &mut app
        ));
        assert!(!handle_key(KeyCode::Char('q'), KeyModifiers::ALT, &mut app));
        assert!(app.hidden.is_empty());
    }

    #[test]
    fn plain_d_hides_and_ctrl_c_quits() {
        let mut app = app_with_one_session();
        assert!(!handle_key(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
            &mut app
        ));
        assert_eq!(app.hidden.len(), 1);
        assert!(handle_key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            &mut app
        ));
    }
}
