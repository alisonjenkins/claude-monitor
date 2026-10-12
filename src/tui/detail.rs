//! The read-only detail page for one session: plan progress with its source,
//! the verbatim summaries and the model-written ones, apart (T093).

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::view::{dim, heading, state_label, GREEN};
use crate::cli::detail::SessionDetail;
use crate::progress::binding::ProgressView;
use crate::store::queries::OverviewEntry;

const BAR_CELLS: usize = 20;

/// The open detail page. `detail` is `None` until the effect that reads it
/// answers.
#[derive(Debug, Clone)]
pub struct DetailView {
    id: String,
    detail: Option<SessionDetail>,
}

impl DetailView {
    pub fn loading(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            detail: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn detail(&self) -> Option<&SessionDetail> {
        self.detail.as_ref()
    }

    pub fn set(&mut self, detail: SessionDetail) {
        self.detail = Some(detail);
    }
}

/// The page's lines; `entry` is the session's overview row when it is still
/// listed.
pub fn detail_lines(view: &DetailView, entry: Option<&OverviewEntry>) -> Vec<Line<'static>> {
    let Some(detail) = view.detail() else {
        return vec![dim("  Loading…".to_owned())];
    };
    let mut lines = vec![heading(header(view.id(), entry))];

    let progress = detail.progress_lines();
    if !progress.is_empty() {
        lines.push(Line::default());
        lines.push(heading("Progress".to_owned()));
        if let ProgressView::Bound(bound) = &detail.progress {
            if let Some(figure) = &bound.progress {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(bar(figure.done, figure.total), Style::default().fg(GREEN)),
                    Span::raw(format!("  {}", figure.label())),
                ]));
            }
        }
        lines.extend(progress.into_iter().map(indented));
    }

    lines.push(Line::default());
    lines.push(heading("Task (verbatim)".to_owned()));
    lines.push(text_or_none(detail.task_description.as_deref()));
    lines.push(heading("Left off (verbatim)".to_owned()));
    lines.push(text_or_none(detail.left_off.as_deref()));

    let generated = detail.generated_lines();
    if !generated.is_empty() {
        lines.push(Line::default());
        lines.push(heading("Model-written summaries".to_owned()));
        lines.extend(generated.into_iter().map(indented));
    }
    lines
}

fn header(id: &str, entry: Option<&OverviewEntry>) -> String {
    match entry {
        Some(entry) => format!(
            "{id}  {}  {}",
            state_label(entry.state),
            entry.repo.as_deref().unwrap_or(&entry.cwd)
        ),
        None => id.to_owned(),
    }
}

fn bar(done: u32, total: u32) -> String {
    let cells = u64::try_from(BAR_CELLS).unwrap_or(u64::MAX);
    let filled = u64::from(done)
        .saturating_mul(cells)
        .checked_div(u64::from(total))
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
        .min(BAR_CELLS);
    format!(
        "[{}{}]",
        "#".repeat(filled),
        "-".repeat(BAR_CELLS.saturating_sub(filled))
    )
}

fn text_or_none(text: Option<&str>) -> Line<'static> {
    match text.filter(|t| !t.trim().is_empty()) {
        Some(text) => Line::from(format!("  {text}")),
        None => dim("  none recorded".to_owned()),
    }
}

fn indented(text: String) -> Line<'static> {
    Line::from(format!("  {text}"))
}
