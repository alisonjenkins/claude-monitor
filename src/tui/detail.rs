//! The read-only detail page for one session: plan progress with its source,
//! the verbatim summaries and the model-written ones, apart (T093).

use ratatui::text::Line;

use crate::cli::detail::SessionDetail;
use crate::store::queries::OverviewEntry;

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
pub fn detail_lines(
    view: &DetailView,
    entry: Option<&OverviewEntry>,
    width: usize,
) -> Vec<Line<'static>> {
    let _ = (view, entry, width);
    Vec::new()
}
