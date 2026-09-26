use chrono::Utc;
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

pub struct App {
    pub sessions: Vec<SessionStatus>,
    pub table_state: TableState,
    pub pane_locations: HashMap<String, PaneLocation>,
}

impl App {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            table_state: TableState::default(),
            pane_locations: HashMap::new(),
        }
    }

    pub fn update_sessions(&mut self, sessions: Vec<SessionStatus>) {
        let was_empty = self.sessions.is_empty();
        self.sessions = sessions;

        // Preserve selection if valid, otherwise reset
        if let Some(selected) = self.table_state.selected() {
            if selected >= self.sessions.len() {
                if self.sessions.is_empty() {
                    self.table_state.select(None);
                } else {
                    self.table_state.select(Some(self.sessions.len() - 1));
                }
            }
        } else if !self.sessions.is_empty() && was_empty {
            self.table_state.select(Some(0));
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
    }

    pub fn previous(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.sessions.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn selected_session(&self) -> Option<&SessionStatus> {
        self.table_state
            .selected()
            .and_then(|i| self.sessions.get(i))
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // Title
        Constraint::Min(5),    // Table
        Constraint::Length(3), // Help
    ])
    .split(frame.area());

    render_title(frame, chunks[0]);
    render_table(frame, app, chunks[1]);
    render_help(frame, chunks[2]);
}

fn render_title(frame: &mut Frame, area: Rect) {
    let title = Paragraph::new(Line::from(vec![Span::styled(
        " Claude Monitor ",
        Style::default()
            .fg(Color::Rgb(203, 166, 247)) // catppuccin mauve
            .add_modifier(Modifier::BOLD),
    )]))
    .block(Block::default().borders(Borders::ALL).border_style(
        Style::default().fg(Color::Rgb(88, 91, 112)), // catppuccin overlay0
    ));
    frame.render_widget(title, area);
}

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.sessions.is_empty() {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  No sessions need attention",
            Style::default().fg(Color::Rgb(108, 112, 134)), // catppuccin subtext0
        )]))
        .block(
            Block::default()
                .title(" Sessions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(88, 91, 112))),
        );
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Project"),
        Cell::from("Status"),
        Cell::from("Location"),
        Cell::from("Waiting"),
    ])
    .style(
        Style::default()
            .fg(Color::Rgb(166, 227, 161)) // catppuccin green
            .add_modifier(Modifier::BOLD),
    );

    let now = Utc::now();
    let rows: Vec<Row> = app
        .sessions
        .iter()
        .map(|s| {
            let status_color = match s.state {
                SessionState::NeedsPermission => Color::Rgb(250, 179, 135), // catppuccin peach
                SessionState::Idle => Color::Rgb(249, 226, 175),            // catppuccin yellow
                SessionState::Working => Color::Rgb(205, 214, 244),         // catppuccin text
            };

            let elapsed = now.signed_duration_since(s.since).num_seconds().max(0);
            let waiting = if elapsed < 60 {
                format!("{}s", elapsed)
            } else if elapsed < 3600 {
                format!("{}m", elapsed / 60)
            } else {
                format!("{}h {}m", elapsed / 3600, (elapsed % 3600) / 60)
            };

            let location = s
                .tmux_pane
                .as_deref()
                .and_then(|pane| app.pane_locations.get(pane))
                .map(PaneLocation::format)
                .unwrap_or_else(|| "unknown".to_string());

            Row::new(vec![
                Cell::from(s.project_name().to_string()),
                Cell::from(s.state_label().to_string()).style(Style::default().fg(status_color)),
                Cell::from(location),
                Cell::from(waiting),
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
                .border_style(Style::default().fg(Color::Rgb(88, 91, 112))),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(49, 50, 68)) // catppuccin surface0
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help = Paragraph::new(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(Color::Rgb(203, 166, 247))),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Rgb(203, 166, 247))),
        Span::raw(" jump  "),
        Span::styled("d", Style::default().fg(Color::Rgb(203, 166, 247))),
        Span::raw(" dismiss  "),
        Span::styled("q", Style::default().fg(Color::Rgb(203, 166, 247))),
        Span::raw(" quit"),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(88, 91, 112))),
    );
    frame.render_widget(help, area);
}
