use chrono::{DateTime, Utc};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};
use std::collections::HashMap;

use crate::session::{SessionState, SessionStatus};
use crate::tmux::PaneLocation;

const MAUVE: Color = Color::Rgb(203, 166, 247);
const OVERLAY0: Color = Color::Rgb(88, 91, 112);
const SUBTEXT0: Color = Color::Rgb(108, 112, 134);
const SURFACE0: Color = Color::Rgb(49, 50, 68);
const PEACH: Color = Color::Rgb(250, 179, 135);
const YELLOW: Color = Color::Rgb(249, 226, 175);
const GREEN: Color = Color::Rgb(166, 227, 161);
const BLUE: Color = Color::Rgb(137, 180, 250);
const RED: Color = Color::Rgb(243, 139, 168);

fn state_rank(state: SessionState) -> u8 {
    match state {
        SessionState::NeedsPermission => 0,
        SessionState::Idle => 1,
        SessionState::Working => 2,
    }
}

fn state_color(state: SessionState) -> Color {
    match state {
        SessionState::NeedsPermission => PEACH,
        SessionState::Idle => YELLOW,
        SessionState::Working => GREEN,
    }
}

/// Sort sessions by priority (permission-needed first, then idle, then
/// working), oldest `since` first within a group.
pub fn sort_sessions(sessions: &mut [SessionStatus]) {
    sessions.sort_by(|a, b| {
        state_rank(a.state)
            .cmp(&state_rank(b.state))
            .then(a.since.cmp(&b.since))
    });
}

/// Format an elapsed duration between `since` and `now` as e.g. "45s",
/// "12m", "3h". Negative durations (clock skew, a `since` in the future)
/// clamp to zero.
pub fn format_elapsed(since: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let elapsed = now.signed_duration_since(since).num_seconds().max(0);
    if elapsed < 60 {
        format!("{elapsed}s")
    } else if elapsed < 3600 {
        format!("{}m", elapsed / 60)
    } else {
        format!("{}h", elapsed / 3600)
    }
}

pub struct App {
    pub sessions: Vec<SessionStatus>,
    pub table_state: TableState,
    pub selected_id: Option<String>,
    pub pane_locations: HashMap<String, PaneLocation>,
    pub hidden: std::collections::HashSet<(String, SessionState, DateTime<Utc>)>,
    pub last_error: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            table_state: TableState::default(),
            selected_id: None,
            pane_locations: HashMap::new(),
            hidden: std::collections::HashSet::new(),
            last_error: None,
        }
    }

    /// Replace the visible session list, keeping the selection on the same
    /// `session_id` if it still has a visible row, and clamping otherwise.
    pub fn update_sessions(&mut self, mut sessions: Vec<SessionStatus>) {
        self.hidden.retain(|(id, state, since)| {
            sessions
                .iter()
                .any(|s| &s.session_id == id && s.state == *state && s.since == *since)
        });
        sessions.retain(|s| {
            !self
                .hidden
                .contains(&(s.session_id.clone(), s.state, s.since))
        });
        sort_sessions(&mut sessions);
        self.sessions = sessions;

        let new_index = self
            .selected_id
            .as_ref()
            .and_then(|id| self.sessions.iter().position(|s| &s.session_id == id));

        match new_index {
            Some(i) => self.table_state.select(Some(i)),
            None if self.sessions.is_empty() => {
                self.table_state.select(None);
                self.selected_id = None;
            }
            None => {
                let clamped = self
                    .table_state
                    .selected()
                    .unwrap_or(0)
                    .min(self.sessions.len() - 1);
                self.table_state.select(Some(clamped));
            }
        }
        self.selected_id = self
            .table_state
            .selected()
            .and_then(|i| self.sessions.get(i))
            .map(|s| s.session_id.clone());
    }

    pub fn hide_selected(&mut self) {
        if let Some(s) = self.selected_session() {
            self.hidden.insert((s.session_id.clone(), s.state, s.since));
            let sessions = std::mem::take(&mut self.sessions);
            self.update_sessions(sessions);
        }
    }

    pub fn next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => (i + 1) % self.sessions.len(),
            None => 0,
        };
        self.table_state.select(Some(i));
        self.selected_id = self.sessions.get(i).map(|s| s.session_id.clone());
    }

    pub fn previous(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(0) | None => self.sessions.len() - 1,
            Some(i) => i - 1,
        };
        self.table_state.select(Some(i));
        self.selected_id = self.sessions.get(i).map(|s| s.session_id.clone());
    }

    pub fn selected_session(&self) -> Option<&SessionStatus> {
        self.table_state
            .selected()
            .and_then(|i| self.sessions.get(i))
    }

    pub fn pane_location(&self, pane_id: &str) -> Option<&PaneLocation> {
        self.pane_locations.get(pane_id)
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(frame.area());

    render_title(frame, app, chunks[0]);
    render_table(frame, app, chunks[1]);
    render_help(frame, app, chunks[2]);
}

fn render_title(frame: &mut Frame, app: &App, area: Rect) {
    let needs_permission = app
        .sessions
        .iter()
        .filter(|s| s.state == SessionState::NeedsPermission)
        .count();
    let idle = app
        .sessions
        .iter()
        .filter(|s| s.state == SessionState::Idle)
        .count();
    let working = app
        .sessions
        .iter()
        .filter(|s| s.state == SessionState::Working)
        .count();

    let title = format!(
        "claude-monitor — {needs_permission} need permission · {idle} idle · {working} working"
    );

    let title = Paragraph::new(Line::from(vec![Span::styled(
        format!(" {title} "),
        Style::default().fg(MAUVE).add_modifier(Modifier::BOLD),
    )]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(OVERLAY0)),
    );
    frame.render_widget(title, area);
}

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.sessions.is_empty() {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  No sessions need attention",
            Style::default().fg(SUBTEXT0),
        )]))
        .block(
            Block::default()
                .title(" Sessions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(OVERLAY0)),
        );
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Project"),
        Cell::from("State"),
        Cell::from("Location"),
        Cell::from("Since"),
    ])
    .style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD));

    let now = Utc::now();
    let rows: Vec<Row> = app
        .sessions
        .iter()
        .map(|s| {
            let color = state_color(s.state);
            let location = s
                .tmux_pane
                .as_deref()
                .and_then(|pane| app.pane_locations.get(pane))
                .map(PaneLocation::format)
                .unwrap_or_else(|| "-".to_string());
            let since = format_elapsed(s.since, now);

            Row::new(vec![
                Cell::from(s.project_name().to_string()),
                Cell::from(s.state_label().to_string()).style(Style::default().fg(color)),
                Cell::from(location),
                Cell::from(since),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(20),
        Constraint::Percentage(30),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" Sessions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(OVERLAY0)),
        )
        .row_highlight_style(
            Style::default()
                .bg(SURFACE0)
                .fg(BLUE)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_help(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(" j/k", Style::default().fg(MAUVE)),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(MAUVE)),
        Span::raw(" jump  "),
        Span::styled("d", Style::default().fg(MAUVE)),
        Span::raw(" hide  "),
        Span::styled("q", Style::default().fg(MAUVE)),
        Span::raw(" quit"),
    ];

    if let Some(err) = &app.last_error {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(err.clone(), Style::default().fg(RED)));
    }

    let help = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(OVERLAY0)),
    );
    frame.render_widget(help, area);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::Duration;

    fn session(id: &str, state: SessionState, since: DateTime<Utc>) -> SessionStatus {
        SessionStatus {
            session_id: id.to_string(),
            state,
            cwd: format!("/tmp/{id}"),
            tmux_pane: None,
            since,
        }
    }

    #[test]
    fn sorts_by_state_priority_then_oldest_first() {
        let now = Utc::now();
        let mut sessions = vec![
            session("working-new", SessionState::Working, now),
            session("perm-new", SessionState::NeedsPermission, now),
            session("idle-old", SessionState::Idle, now - Duration::seconds(100)),
            session(
                "perm-old",
                SessionState::NeedsPermission,
                now - Duration::seconds(100),
            ),
        ];
        sort_sessions(&mut sessions);
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["perm-old", "perm-new", "idle-old", "working-new"]);
    }

    #[test]
    fn selection_follows_session_id_across_refresh() {
        let now = Utc::now();
        let mut app = App::new();
        app.update_sessions(vec![
            session("a", SessionState::Working, now),
            session("b", SessionState::NeedsPermission, now),
        ]);
        // "b" sorts first (permission), select it.
        app.table_state.select(Some(0));
        app.selected_id = Some("b".to_string());

        // Reorder: "a" becomes permission too, but older, so it sorts first now.
        app.update_sessions(vec![
            session(
                "a",
                SessionState::NeedsPermission,
                now - Duration::seconds(10),
            ),
            session("b", SessionState::NeedsPermission, now),
        ]);

        assert_eq!(app.selected_id, Some("b".to_string()));
        let idx = app.table_state.selected().unwrap();
        assert_eq!(app.sessions[idx].session_id, "b");
    }

    #[test]
    fn selection_clamps_when_list_shrinks() {
        let now = Utc::now();
        let mut app = App::new();
        app.update_sessions(vec![
            session("a", SessionState::Working, now),
            session("b", SessionState::Working, now - Duration::seconds(1)),
        ]);
        app.table_state.select(Some(1));
        app.selected_id = Some(app.sessions[1].session_id.clone());

        app.update_sessions(vec![session(
            "c",
            SessionState::Working,
            now - Duration::seconds(2),
        )]);

        assert_eq!(app.table_state.selected(), Some(0));
        assert_eq!(app.selected_id, Some("c".to_string()));
    }

    #[test]
    fn selection_none_when_list_empty() {
        let mut app = App::new();
        app.update_sessions(vec![session("a", SessionState::Working, Utc::now())]);
        app.update_sessions(vec![]);
        assert_eq!(app.table_state.selected(), None);
        assert_eq!(app.selected_id, None);
    }

    #[test]
    fn elapsed_formats_seconds_minutes_hours() {
        let now = Utc::now();
        assert_eq!(format_elapsed(now - Duration::seconds(45), now), "45s");
        assert_eq!(format_elapsed(now - Duration::seconds(720), now), "12m");
        assert_eq!(format_elapsed(now - Duration::seconds(10800), now), "3h");
    }

    #[test]
    fn elapsed_clamps_negative_to_zero() {
        let now = Utc::now();
        let future = now + Duration::seconds(30);
        assert_eq!(format_elapsed(future, now), "0s");
    }

    #[test]
    fn hidden_entry_is_filtered_and_reappears_on_state_change() {
        let now = Utc::now();
        let mut app = App::new();
        app.update_sessions(vec![session("a", SessionState::Idle, now)]);
        app.table_state.select(Some(0));
        app.selected_id = Some("a".to_string());
        app.hide_selected();
        assert!(app.sessions.is_empty());

        // Same state/since: stays hidden.
        app.update_sessions(vec![session("a", SessionState::Idle, now)]);
        assert!(app.sessions.is_empty());

        // State changes: reappears.
        app.update_sessions(vec![session("a", SessionState::Working, now)]);
        assert_eq!(app.sessions.len(), 1);
    }
}
