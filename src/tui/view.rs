//! Renders an [`App`] into a ratatui frame.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, Prompt};
use super::bind::BindChooser;
use super::detail::detail_lines;
use super::restore::{Group, PreviewRow, RestoreResult, RestoreRow, RestoreView, Stage};
use crate::cli::age;
use crate::model::State;
use crate::store::queries::{Flag, OverviewEntry, SectionName};

const MAUVE: Color = Color::Rgb(203, 166, 247);
const OVERLAY0: Color = Color::Rgb(88, 91, 112);
const SUBTEXT0: Color = Color::Rgb(108, 112, 134);
const SURFACE0: Color = Color::Rgb(49, 50, 68);
pub(super) const PEACH: Color = Color::Rgb(250, 179, 135);
const YELLOW: Color = Color::Rgb(249, 226, 175);
pub(super) const GREEN: Color = Color::Rgb(166, 227, 161);
pub(super) const RED: Color = Color::Rgb(243, 139, 168);

const TITLE_HEIGHT: u16 = 3;
const HELP_HEIGHT: u16 = 4;
const BORDER_ROWS: u16 = 2;
const LABEL_WIDTH: usize = 11;
const AGENT_WIDTH: usize = 8;
const AGE_WIDTH: usize = 4;
const ELLIPSIS: &str = "…";

pub use crate::notify::state_label;

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

pub(super) fn section_title(name: SectionName) -> &'static str {
    match name {
        SectionName::Active => "Active",
        SectionName::Interrupted => "Interrupted",
        SectionName::Finished => "Finished",
        SectionName::Done => "Done",
        SectionName::Unseen => "Unseen",
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

fn row(
    entry: &OverviewEntry,
    figure: Option<(u32, u32)>,
    now: &str,
    selected: bool,
    width: usize,
) -> Line<'static> {
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
        (
            figure.map_or_else(String::new, |(done, total)| format!("{done}/{total}  ")),
            Style::default().fg(GREEN),
        ),
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
    for part in app.parts() {
        let collapsed = part.name == SectionName::Unseen && !app.unseen_expanded();
        if part.entries.is_empty() && (!collapsed || part.total == 0) {
            continue;
        }
        let (count, hint) = if collapsed {
            (part.total, " — u to show")
        } else {
            (part.entries.len(), "")
        };
        lines.push(Line::from(Span::styled(
            format!("{} ({count}){hint}", part.title()),
            Style::default().fg(MAUVE).add_modifier(Modifier::BOLD),
        )));
        for entry in part.entries {
            let selected = app.selected_id() == Some(entry.id.as_str());
            if selected {
                selected_line = lines.len();
            }
            let figure = snapshot.figures.get(&entry.id).copied();
            lines.push(row(entry, figure, &snapshot.now, selected, width));
        }
    }
    if lines.is_empty() {
        let text = if app.filter().narrows() {
            "No sessions match the filter."
        } else if app.hidden_count() > 0 {
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

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let mut spans = Vec::new();
    let notice = app
        .status()
        .or_else(|| app.restore().and_then(RestoreView::notice));
    if let Some(status) = notice {
        spans.push(Span::styled(status.to_owned(), Style::default().fg(RED)));
        spans.push(Span::raw("  "));
    }
    let key = Style::default().fg(MAUVE);
    if let Some(restore) = app.restore() {
        spans.extend(restore_help(restore.stage(), key));
        return vec![Line::from(spans)];
    }
    if app.chooser().is_some() {
        spans.extend([
            Span::styled(" j/k", key),
            Span::raw(" move  "),
            Span::styled("Enter", key),
            Span::raw(" bind  "),
            Span::styled("Esc", key),
            Span::raw(" cancel"),
        ]);
        return vec![Line::from(spans)];
    }
    if app.detail().is_some() {
        spans.extend([
            Span::styled(" L", key),
            Span::raw(" check links  "),
            Span::styled("Esc", key),
            Span::raw(" close"),
        ]);
        return vec![Line::from(spans)];
    }
    if let Some(prompt) = app.prompt() {
        spans.extend(prompt_help(prompt, app.filter().text(), key));
        return vec![Line::from(spans)];
    }
    spans.extend(keys_help(
        key,
        &[
            ("j/k", "navigate"),
            ("Enter", "jump"),
            ("i", "detail"),
            ("L", "links"),
            ("p", "projects"),
            ("/", "filter"),
            ("f", "state"),
            ("a", "agent"),
            ("S", "dismissed"),
        ],
    ));
    let second = keys_help(
        key,
        &[
            ("D", "done"),
            ("U", "undone"),
            ("x", "dismiss"),
            ("n", "note"),
            ("d", "hide"),
            ("u", "unseen"),
            ("R", "restore"),
            ("q", "quit"),
        ],
    );
    vec![Line::from(spans), Line::from(second)]
}

/// ` j/k navigate  Enter jump`: each key styled, each label plain.
fn keys_help(key: Style, bindings: &[(&'static str, &'static str)]) -> Vec<Span<'static>> {
    bindings
        .iter()
        .enumerate()
        .flat_map(|(at, (name, label))| {
            let lead = if at == 0 { " " } else { "  " };
            [
                Span::raw(lead),
                Span::styled(*name, key),
                Span::raw(format!(" {label}")),
            ]
        })
        .collect()
}

fn prompt_help(prompt: &Prompt, filter_text: &str, key: Style) -> Vec<Span<'static>> {
    let (shown, enter) = match prompt {
        Prompt::Filter => (format!(" /{filter_text}"), "keep"),
        Prompt::Note { text, .. } => (format!(" note: {text}"), "save"),
        Prompt::Group { text, .. } => (format!(" project (- for none): {text}"), "save"),
        Prompt::Link { text, .. } => (format!(" link to node [doc]: {text}"), "save"),
    };
    let leave = match prompt {
        Prompt::Filter => "clear",
        Prompt::Note { .. } | Prompt::Group { .. } | Prompt::Link { .. } => "cancel",
    };
    vec![
        Span::styled(
            format!("{shown}▏"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("Enter", key),
        Span::raw(format!(" {enter}  ")),
        Span::styled("Esc", key),
        Span::raw(format!(" {leave}")),
    ]
}

/// The chooser's rows and the line of the highlighted one.
fn chooser_lines(chooser: &BindChooser) -> (Vec<Line<'static>>, usize) {
    let mut lines = vec![
        heading(format!("Progress source for {}", chooser.id())),
        Line::default(),
    ];
    let first = lines.len();
    lines.extend(chooser.options().iter().enumerate().map(|(at, option)| {
        let chosen = at == chooser.cursor();
        let style = if chosen {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        Line::from(Span::styled(
            format!("{} {}", if chosen { ">" } else { " " }, option.label),
            style,
        ))
    }));
    (lines, first.saturating_add(chooser.cursor()))
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
        Paragraph::new(help_lines(app)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(OVERLAY0)),
        ),
        help_area,
    );
}

/// `Sessions`, then the view and filters in force: ` · by project · state idle`.
fn sessions_title(app: &App) -> String {
    let notes: Vec<String> = [
        app.grouped().then(|| "by project".to_owned()),
        app.filter().summary(),
        app.filter()
            .show_dismissed()
            .then(|| "with dismissed".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();
    if notes.is_empty() {
        " Sessions ".to_owned()
    } else {
        format!(" Sessions · {} ", notes.join(" · "))
    }
}

fn render_body(app: &App, frame: &mut Frame, area: Rect) {
    let inner_width = usize::from(area.width.saturating_sub(BORDER_ROWS));
    let (lines, selected_line, title, wrap) = match (app.detail(), app.restore(), app.snapshot()) {
        _ if app.chooser().is_some() => {
            let (lines, selected) = app.chooser().map(chooser_lines).unwrap_or_default();
            (lines, selected, " Progress source ".to_owned(), false)
        }
        (Some(open), _, _) => {
            let entry = app.visible().into_iter().find(|e| e.id == open.id());
            (detail_lines(open, entry), 0, " Detail ".to_owned(), true)
        }
        (None, Some(restore), Some(snapshot)) => {
            let (lines, selected) = restore_lines(restore, &snapshot.now, inner_width);
            // Refusal messages are long; the sessions list truncates instead.
            let wrap = !matches!(restore.stage(), Stage::Select);
            (lines, selected, " Restore ".to_owned(), wrap)
        }
        _ => {
            let (lines, selected) = body_lines(app, inner_width);
            (lines, selected, sessions_title(app), false)
        }
    };
    let offset = scroll_offset(selected_line, area.height.saturating_sub(BORDER_ROWS));
    let paragraph = Paragraph::new(lines)
        .scroll((offset, 0))
        .block(block(&title));
    frame.render_widget(
        if wrap {
            paragraph.wrap(Wrap { trim: false })
        } else {
            paragraph
        },
        area,
    );
}

fn group_title(group: Group) -> &'static str {
    match group {
        Group::Interrupted => "Interrupted",
        Group::Finished => "Finished",
        Group::Unseen => "Unseen",
    }
}

fn restore_help(stage: &Stage, key: Style) -> Vec<Span<'static>> {
    match stage {
        Stage::Select => vec![
            Span::styled(" j/k", key),
            Span::raw(" move  "),
            Span::styled("Space", key),
            Span::raw(" toggle  "),
            Span::styled("a", key),
            Span::raw(" all interrupted  "),
            Span::styled("n", key),
            Span::raw(" none  "),
            Span::styled("Enter", key),
            Span::raw(" review  "),
            Span::styled("Esc", key),
            Span::raw(" cancel"),
        ],
        Stage::Previewing => vec![Span::styled(" Esc", key), Span::raw(" cancel")],
        Stage::Confirm(_) => vec![
            Span::styled(" Enter", key),
            Span::raw(" start  "),
            Span::styled("Esc", key),
            Span::raw(" back"),
        ],
        Stage::Running { .. } => vec![Span::raw(" starting…")],
        Stage::Finished(_) => vec![Span::styled(" Esc", key), Span::raw(" close")],
    }
}

pub(super) fn heading(text: String) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default().fg(MAUVE).add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn dim(text: String) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().fg(SUBTEXT0)))
}

fn restore_row(row: &RestoreRow, now: &str, at_cursor: bool, width: usize) -> Line<'static> {
    let entry = &row.entry;
    let plain = Style::default();
    let segments = vec![
        (
            if at_cursor { "> " } else { "  " }.to_owned(),
            Style::default().fg(PEACH),
        ),
        (if row.selected { "[x] " } else { "[ ] " }.to_owned(), plain),
        (padded(&entry.agent_kind, AGENT_WIDTH), plain),
        (
            format!("{:>AGE_WIDTH$}  ", age(now, &entry.last_activity_at)),
            Style::default().fg(SUBTEXT0),
        ),
        (location(entry), plain),
        ("  ".to_owned(), plain),
        (
            entry
                .task_description
                .as_deref()
                .map(one_line)
                .unwrap_or_default(),
            Style::default().fg(SUBTEXT0),
        ),
    ];
    let line = fit(segments, width);
    if at_cursor {
        line.style(Style::default().bg(SURFACE0).add_modifier(Modifier::BOLD))
    } else {
        line
    }
}

fn restore_lines(view: &RestoreView, now: &str, width: usize) -> (Vec<Line<'static>>, usize) {
    match view.stage() {
        Stage::Select => select_lines(view, now, width),
        Stage::Previewing => (note("Planning…"), 0),
        Stage::Confirm(rows) => (confirm_lines(rows), 0),
        Stage::Running { total, results } => (result_lines(results, Some(*total)), 0),
        Stage::Finished(results) => (result_lines(results, None), 0),
    }
}

fn select_lines(view: &RestoreView, now: &str, width: usize) -> (Vec<Line<'static>>, usize) {
    if view.rows().is_empty() {
        return (note("Nothing to restore."), 0);
    }
    let mut lines = Vec::new();
    let mut cursor_line = 0;
    let mut group = None;
    for (index, row) in view.rows().iter().enumerate() {
        if group != Some(row.group) {
            group = Some(row.group);
            let count = view.rows().iter().filter(|r| r.group == row.group).count();
            lines.push(heading(format!("{} ({count})", group_title(row.group))));
        }
        let at_cursor = index == view.cursor();
        if at_cursor {
            cursor_line = lines.len();
        }
        lines.push(restore_row(row, now, at_cursor, width));
    }
    (lines, cursor_line)
}

fn confirm_lines(rows: &[PreviewRow]) -> Vec<Line<'static>> {
    let starts: Vec<&PreviewRow> = rows
        .iter()
        .filter(|r| matches!(r, PreviewRow::Start { .. }))
        .collect();
    let mut lines = vec![heading(format!("Will start ({})", starts.len()))];
    for row in &starts {
        if let PreviewRow::Start {
            session_id,
            mode,
            cwd,
            notices,
        } = row
        {
            lines.push(Line::from(format!("  {session_id}  {mode}  {cwd}")));
            lines.extend(notices.iter().map(|n| dim(format!("      note: {n}"))));
        }
    }
    let refused = rows.len().saturating_sub(starts.len());
    if refused > 0 {
        lines.push(heading(format!("Refused ({refused})")));
    }
    for row in rows {
        if let PreviewRow::Refuse {
            session_id,
            code,
            message,
            alternatives,
        } = row
        {
            lines.push(Line::from(Span::styled(
                format!("  {session_id}  {code}: {message}"),
                Style::default().fg(RED),
            )));
            for (description, command) in alternatives {
                lines.push(dim(format!("      instead: {description}")));
                lines.push(dim(format!("        {command}")));
            }
        }
    }
    lines
}

fn result_lines(results: &[RestoreResult], pending_of: Option<usize>) -> Vec<Line<'static>> {
    let title = match pending_of {
        Some(total) => format!("Starting {} of {total}", results.len()),
        None => "Done".to_owned(),
    };
    let mut lines = vec![heading(title)];
    for result in results {
        lines.push(match result {
            RestoreResult::Started { session_id, pane } => Line::from(Span::styled(
                format!("  {session_id}  started in pane {pane}"),
                Style::default().fg(GREEN),
            )),
            RestoreResult::Refused {
                session_id,
                code,
                message,
            } => Line::from(Span::styled(
                format!("  {session_id}  refused {code}: {message}"),
                Style::default().fg(RED),
            )),
        });
    }
    lines
}
