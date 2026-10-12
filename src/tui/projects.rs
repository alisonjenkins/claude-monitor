//! The dashboard's rows laid out as sections, either flat or grouped under
//! their project (T111, FR-019b).

use crate::store::queries::{OverviewEntry, Section, SectionName};

/// Where a part of the list belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    /// Not tied to a project: the flat view's sections, and the Unseen and
    /// Dismissed sections of the grouped view.
    Global,
    Project(&'a str),
    /// Sessions in no project, in the grouped view.
    NoProject,
}

/// One titled run of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part<'a> {
    pub scope: Scope<'a>,
    pub name: SectionName,
    /// The rows to show, in display order.
    pub entries: Vec<&'a OverviewEntry>,
    /// Rows the section holds before hiding and collapsing.
    pub total: usize,
}

impl Part<'_> {
    pub fn title(&self) -> String {
        String::new()
    }
}

/// The sections as they are, each its own part.
pub fn flat<'a>(sections: Vec<(&'a Section, Vec<&'a OverviewEntry>)>) -> Vec<Part<'a>> {
    sections
        .into_iter()
        .map(|(section, entries)| Part {
            scope: Scope::Global,
            name: section.name,
            entries,
            total: section.sessions.len(),
        })
        .collect()
}

/// Rows grouped under their project, each project with its own Active,
/// Interrupted, Finished and Done parts, then the sessions in no project the
/// same way, then Unseen and Dismissed as they are.
pub fn grouped<'a>(_sections: Vec<(&'a Section, Vec<&'a OverviewEntry>)>) -> Vec<Part<'a>> {
    Vec::new()
}
