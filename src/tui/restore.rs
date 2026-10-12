//! The restore view (`R`): pick which sessions to bring back, see what would
//! start, confirm, then watch each start. The view-model is pure; the runner
//! plans and starts through the effects it returns.

use crate::store::queries::{Overview, OverviewEntry, SectionName};

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
        Self { rows, cursor: 0 }
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
