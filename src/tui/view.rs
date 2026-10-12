//! Renders an [`App`] into a ratatui frame.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::app::App;
use crate::cli::age;
use crate::model::State;
use crate::store::queries::{Flag, OverviewEntry, SectionName};

const MAUVE: Color = Color::Rgb(203, 166, 247);
const OVERLAY0: Color = Color::Rgb(88, 91, 112);
const SUBTEXT0: Color = Color::Rgb(108, 112, 134);
const SURFACE0: Color = Color::Rgb(49, 50, 68);
const PEACH: Color = Color::Rgb(250, 179, 135);
const YELLOW: Color = Color::Rgb(249, 226, 175);
const GREEN: Color = Color::Rgb(166, 227, 161);
const RED: Color = Color::Rgb(243, 139, 168);

const TITLE_HEIGHT: u16 = 3;
const HELP_HEIGHT: u16 = 3;
const BORDER_ROWS: u16 = 2;
const LABEL_WIDTH: usize = 11;
const AGENT_WIDTH: usize = 8;
const AGE_WIDTH: usize = 4;
const ELLIPSIS: &str = "…";

pub fn state_label(state: State) -> &'static str {
    match state {
        State::Working => "WORKING",
        State::NeedsPermission => "PERMISSION",
        State::Idle => "IDLE",
        State::Finished => "FINISHED",
        State::Interrupted => "INTERRUPTED",
        State::Unknown => "UNKNOWN",
    }
}

fn is_degraded(entry: &OverviewEntry) -> bool {
    entry.error.is_some() || entry.flags.contains(&Flag::Degraded)
}

fn label(entry: &OverviewEntry) -> &'static str {
    if is_degraded(entry) {
        "DEGRADED"
    } else {
        state_label(entry.state)
    }
}

fn label_style(entry: &OverviewEntry) -> Style {
    let color = if is_degraded(entry) {
        RED
    } else {
        match entry.state {
            State::NeedsPermission => PEACH,
            State::Idle => YELLOW,
            State::Working => GREEN,
            State::Interrupted => RED,
            State::Finished | State::Unknown => SUBTEXT0,
        }
    };
    let style = Style::default().fg(color);
    if entry.state == State::NeedsPermission {
        style.add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        style
    }
}

fn section_title(name: SectionName) -> &'static str {
    match name {
        SectionName::Active => "Active",
        SectionName::Interrupted => "Interrupted",
        SectionName::Finished => "Finished",
        SectionName::Done => "Done",
        SectionName::Dismissed => "Dismissed",
    }
}

fn location(entry: &OverviewEntry) -> String {
    match (&entry.repo, &entry.branch) {
        (Some(repo), Some(branch)) => format!("{repo}/{branch}"),
        (Some(repo), None) => repo.clone(),
        (None, _) if entry.cwd.is_empty() => "-".to_owned(),
        (None, _) => entry.cwd.clone(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn padded(text: &str, width: usize) -> String {
    let clipped: String = text.chars().take(width).collect();
    format!("{clipped:<width$}")
}

/// Cuts `segments` to `width` characters, ending in an ellipsis when it cut.
fn fit(segments: Vec<(String, Style)>, width: usize) -> Line<'static> {
    let total: usize = segments.iter().map(|(text, _)| text.chars().count()).sum();
    if total <= width {
        return Line::from(
            segments
                .into_iter()
                .map(|(text, style)| Span::styled(text, style))
                .collect::<Vec<_>>(),
        );
    }
    let mut budget = width.saturating_sub(1);
    let mut spans = Vec::new();
    for (text, style) in segments {
        let kept: String = text.chars().take(budget).collect();
        budget = budget.saturating_sub(kept.chars().count());
        spans.push(Span::styled(kept, style));
        if budget == 0 {
            break;
        }
    }
    if width > 0 {
        spans.push(Span::raw(ELLIPSIS));
    }
    Line::from(spans)
}

fn row(entry: &OverviewEntry, now: &str, selected: bool, width: usize) -> Line<'static> {
    let marker = if selected {
        "> "
    } else if entry.state == State::NeedsPermission {
        "! "
    } else {
        "  "
    };
    let task = entry
        .error
        .as_deref()
        .or(entry.task_description.as_deref())
        .map(one_line)
        .unwrap_or_default();
    let plain = Style::default();
    let segments = vec![
        (marker.to_owned(), Style::default().fg(PEACH)),
        (padded(label(entry), LABEL_WIDTH), label_style(entry)),
        (" ".to_owned(), plain),
        (padded(&entry.agent_kind, AGENT_WIDTH), plain),
        (
            format!("{:>AGE_WIDTH$}  ", age(now, &entry.last_activity_at)),
            Style::default().fg(SUBTEXT0),
        ),
        (location(entry), plain),
        ("  ".to_owned(), plain),
        (task, Style::default().fg(SUBTEXT0)),
    ];
    let line = fit(segments, width);
    if selected {
        line.style(Style::default().bg(SURFACE0).add_modifier(Modifier::BOLD))
    } else {
        line
    }
}

fn block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(OVERLAY0))
}

fn note(text: &str) -> Vec<Line<'static>> {
    vec![Line::from(Span::styled(
        format!("  {text}"),
        Style::default().fg(SUBTEXT0),
    ))]
}

/// The body's lines and the index of the selected row among them.
fn body_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize) {
    let Some(snapshot) = app.snapshot() else {
        return (note("Loading…"), 0);
    };
    let mut lines = Vec::new();
    let mut selected_line = 0;
    for (section, entries) in app.sections() {
        if entries.is_empty() && section.collapsed_count == 0 {
            continue;
        }
        lines.push(Line::from(Span::styled(
            format!("{} ({})", section_title(section.name), entries.len()),
            Style::default().fg(MAUVE).add_modifier(Modifier::BOLD),
        )));
        for entry in entries {
            let selected = app.selected_id() == Some(entry.id.as_str());
            if selected {
                selected_line = lines.len();
            }
            lines.push(row(entry, &snapshot.now, selected, width));
        }
        if section.collapsed_count > 0 {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} older finished (list --expand-finished)",
                    section.collapsed_count
                ),
                Style::default().fg(SUBTEXT0),
            )));
        }
    }
    if lines.is_empty() {
        let text = if app.hidden_count() > 0 {
            "Every session is hidden with d; each returns when its state changes."
        } else {
            "No sessions yet. They appear here once an agent reports in."
        };
        return (note(text), 0);
    }
    (lines, selected_line)
}

fn title_line(app: &App) -> String {
    let visible = app.visible();
    let count = |state: State| visible.iter().filter(|e| e.state == state).count();
    format!(
        " claude-monitor — {} need permission · {} idle · {} working ",
        count(State::NeedsPermission),
        count(State::Idle),
        count(State::Working),
    )
}

fn help_line(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    if let Some(status) = app.status() {
        spans.push(Span::styled(status.to_owned(), Style::default().fg(RED)));
        spans.push(Span::raw("  "));
    }
    let key = Style::default().fg(MAUVE);
    spans.extend([
        Span::styled(" j/k", key),
        Span::raw(" navigate  "),
        Span::styled("Enter", key),
        Span::raw(" jump  "),
        Span::styled("d", key),
        Span::raw(" hide  "),
        Span::styled("q", key),
        Span::raw(" quit"),
    ]);
    Line::from(spans)
}

fn scroll_offset(selected_line: usize, visible_rows: u16) -> u16 {
    let rows = usize::from(visible_rows).max(1);
    let offset = selected_line.saturating_add(1).saturating_sub(rows);
    u16::try_from(offset).unwrap_or(u16::MAX)
}

pub fn view(app: &App, frame: &mut Frame) {
    let [title_area, body_area, help_area] = Layout::vertical([
        Constraint::Length(TITLE_HEIGHT),
        Constraint::Min(0),
        Constraint::Length(HELP_HEIGHT),
    ])
    .areas::<3>(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title_line(app),
            Style::default().fg(MAUVE).add_modifier(Modifier::BOLD),
        )))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(OVERLAY0)),
        ),
        title_area,
    );
    render_body(app, frame, body_area);
    frame.render_widget(
        Paragraph::new(help_line(app)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(OVERLAY0)),
        ),
        help_area,
    );
}

fn render_body(app: &App, frame: &mut Frame, area: Rect) {
    let inner_width = usize::from(area.width.saturating_sub(BORDER_ROWS));
    let (lines, selected_line) = body_lines(app, inner_width);
    let offset = scroll_offset(selected_line, area.height.saturating_sub(BORDER_ROWS));
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((offset, 0))
            .block(block(" Sessions ")),
        area,
    );
}
