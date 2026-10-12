//! The restore view (`R`): pick which sessions to bring back, see what would
//! start, confirm, then watch each start. The view-model is pure; the runner
//! plans and starts through the effects it returns.

use crossterm::event::KeyCode;

use crate::env::Env;
use crate::store::queries::{Overview, OverviewEntry, SectionName};
use crate::store::Store;

/// Plan `ids` for the confirm screen. Reads the filesystem and process table;
/// starts and writes nothing.
pub fn preview_restore(_store: &Store, _env: &Env, _ids: &[String]) -> Vec<PreviewRow> {
    Vec::new()
}

/// Start `ids` one after another, calling `emit` as each finishes. A failure
/// to start one does not stop the others.
pub fn run_restore(
    _store: &Store,
    _env: &Env,
    _ids: &[String],
    _emit: &mut dyn FnMut(RestoreResult),
) {
}

/// Where a row came from; decides whether it starts selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Interrupted,
    Finished,
    Unseen,
}

#[derive(Debug, Clone)]
pub struct RestoreRow {
    pub entry: OverviewEntry,
    pub group: Group,
    pub selected: bool,
}

/// What the confirm screen says will happen to one selected session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewRow {
    Start {
        session_id: String,
        mode: String,
        cwd: String,
        notices: Vec<String>,
    },
    Refuse {
        session_id: String,
        code: String,
        message: String,
        /// `(description, command)` pairs, never taken automatically.
        alternatives: Vec<(String, String)>,
    },
}

/// What became of one session once confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreResult {
    Started {
        session_id: String,
        pane: String,
    },
    Refused {
        session_id: String,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Stage {
    /// Choosing sessions.
    #[default]
    Select,
    /// Waiting for the plans of the selection.
    Previewing,
    /// One screen: what will start and what is refused. Enter starts.
    Confirm(Vec<PreviewRow>),
    /// Starting; results arrive one by one.
    Running {
        total: usize,
        results: Vec<RestoreResult>,
    },
    Finished(Vec<RestoreResult>),
}

/// What a key did to the view beyond changing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Stay,
    Close,
    /// Plan these sessions for the confirm screen.
    Preview(Vec<String>),
    /// Start these sessions now.
    Start(Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub struct RestoreView {
    rows: Vec<RestoreRow>,
    cursor: usize,
    stage: Stage,
    notice: Option<String>,
}

impl RestoreView {
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// A one-line reminder for the footer, such as "nothing selected".
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn on_key(&mut self, _code: KeyCode) -> Step {
        Step::Stay
    }

    pub fn on_preview(&mut self, _rows: Vec<PreviewRow>) {}

    pub fn on_result(&mut self, _result: RestoreResult) {}

    /// Rows for the interrupted, finished and unseen sections. Dismissed,
    /// done and active sessions are not offered.
    pub fn from_overview(overview: &Overview) -> Self {
        let rows = overview
            .sections
            .iter()
            .filter_map(|section| {
                let group = match section.name {
                    SectionName::Interrupted => Group::Interrupted,
                    SectionName::Finished => Group::Finished,
                    SectionName::Unseen => Group::Unseen,
                    _ => return None,
                };
                Some((group, &section.sessions))
            })
            .flat_map(|(group, sessions)| {
                sessions.iter().map(move |entry| RestoreRow {
                    entry: entry.clone(),
                    group,
                    selected: group == Group::Interrupted,
                })
            })
            .collect();
        Self {
            rows,
            cursor: 0,
            ..Self::default()
        }
    }

    pub fn rows(&self) -> &[RestoreRow] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn move_cursor(&mut self, forward: bool) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = if forward {
            self.cursor.saturating_add(1).min(last)
        } else {
            self.cursor.saturating_sub(1)
        };
    }

    /// Flip the selection of the row under the cursor. Only the selection
    /// changes: the session stays interrupted and is not dismissed.
    pub fn toggle(&mut self) {
        if let Some(row) = self.rows.get_mut(self.cursor) {
            row.selected = !row.selected;
        }
    }

    pub fn select_all_interrupted(&mut self) {
        for row in &mut self.rows {
            if row.group == Group::Interrupted {
                row.selected = true;
            }
        }
    }

    pub fn select_none(&mut self) {
        for row in &mut self.rows {
            row.selected = false;
        }
    }

    pub fn selected_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.selected)
            .map(|row| row.entry.id.clone())
            .collect()
    }
}
