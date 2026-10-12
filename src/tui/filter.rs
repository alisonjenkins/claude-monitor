//! The dashboard's filters (T111, FR-019): free text, one state, one agent
//! kind, and whether dismissed sessions are listed.

use crate::model::State;
use crate::store::queries::OverviewEntry;

const STATE_CYCLE: [State; 6] = [
    State::NeedsPermission,
    State::Idle,
    State::Working,
    State::Finished,
    State::Interrupted,
    State::Unknown,
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    text: String,
    state: Option<State>,
    agent: Option<String>,
    show_dismissed: bool,
}

impl Filter {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
    }

    pub fn state(&self) -> Option<State> {
        self.state
    }

    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    pub fn show_dismissed(&self) -> bool {
        self.show_dismissed
    }

    pub fn toggle_dismissed(&mut self) {
        self.show_dismissed = !self.show_dismissed;
    }

    /// Moves to the next state, then back to none after the last.
    pub fn cycle_state(&mut self) {
        self.state = match self.state {
            None => STATE_CYCLE.first().copied(),
            Some(current) => STATE_CYCLE
                .iter()
                .position(|state| *state == current)
                .and_then(|at| STATE_CYCLE.get(at.saturating_add(1)))
                .copied(),
        };
    }

    /// Moves to the next of `kinds` (sorted, distinct), then back to none. A
    /// kind that is no longer listed counts as none.
    pub fn cycle_agent(&mut self, kinds: &[String]) {
        let next = match self.agent.as_deref() {
            None => 0,
            Some(current) => kinds
                .iter()
                .position(|kind| kind == current)
                .map_or(0, |at| at.saturating_add(1)),
        };
        self.agent = kinds.get(next).cloned();
    }

    /// True when text, state or agent narrows the list.
    pub fn narrows(&self) -> bool {
        !self.text.is_empty() || self.state.is_some() || self.agent.is_some()
    }

    /// Whether the row passes the text, state and agent filters. Dismissed
    /// rows are handled by the section they are in, not here.
    pub fn matches(&self, entry: &OverviewEntry) -> bool {
        self.state.is_none_or(|state| entry.state == state)
            && self
                .agent
                .as_deref()
                .is_none_or(|agent| entry.agent_kind == agent)
            && self.text_matches(entry)
    }

    fn text_matches(&self, entry: &OverviewEntry) -> bool {
        if self.text.is_empty() {
            return true;
        }
        let needle = self.text.to_lowercase();
        [
            entry.task_description.as_deref(),
            Some(entry.cwd.as_str()),
            entry.repo.as_deref(),
            entry.branch.as_deref(),
            entry.project.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|field| field.to_lowercase().contains(&needle))
    }

    /// `state idle · agent claude · /text`, `None` when nothing is set.
    pub fn summary(&self) -> Option<String> {
        let parts: Vec<String> = [
            self.state.map(|state| format!("state {state}")),
            self.agent.as_ref().map(|agent| format!("agent {agent}")),
            (!self.text.is_empty()).then(|| format!("/{}", self.text)),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}
