//! The restore view (`R`): pick which sessions to bring back, see what would
//! start, confirm, then watch each start. The view-model is pure; the runner
//! plans and starts through the effects it returns.

use crate::store::queries::{Overview, OverviewEntry};

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

#[derive(Debug, Clone, Default)]
pub struct RestoreView {
    rows: Vec<RestoreRow>,
    cursor: usize,
}

impl RestoreView {
    /// Rows for the interrupted, finished and unseen sections. Dismissed,
    /// done and active sessions are not offered.
    pub fn from_overview(_overview: &Overview) -> Self {
        Self::default()
    }

    pub fn rows(&self) -> &[RestoreRow] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn move_cursor(&mut self, _forward: bool) {}

    /// Flip the selection of the row under the cursor. Only the selection
    /// changes: the session stays interrupted and is not dismissed.
    pub fn toggle(&mut self) {}

    pub fn select_all_interrupted(&mut self) {}

    pub fn select_none(&mut self) {}

    pub fn selected_ids(&self) -> Vec<String> {
        Vec::new()
    }
}
