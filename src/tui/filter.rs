//! The dashboard's filters (T111, FR-019): free text, one state, one agent
//! kind, and whether dismissed sessions are listed.

use crate::model::State;
use crate::store::queries::OverviewEntry;

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
    pub fn cycle_state(&mut self) {}

    /// Moves to the next of `kinds` (sorted, distinct), then back to none.
    pub fn cycle_agent(&mut self, _kinds: &[String]) {}

    /// True when text, state or agent narrows the list.
    pub fn narrows(&self) -> bool {
        false
    }

    pub fn matches(&self, _entry: &OverviewEntry) -> bool {
        true
    }

    /// `state idle · agent claude · /text`, `None` when nothing is set.
    pub fn summary(&self) -> Option<String> {
        None
    }
}
