//! The dashboard's rows laid out as sections, either flat or grouped under
//! their project (T111, FR-019b).

use std::collections::BTreeSet;

use super::view::section_title;
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

const NO_PROJECT_TITLE: &str = "No project";

/// The sections each project (and the no-project group) is split into.
const PROJECT_SECTIONS: [SectionName; 4] = [
    SectionName::Active,
    SectionName::Interrupted,
    SectionName::Finished,
    SectionName::Done,
];

impl Part<'_> {
    pub fn title(&self) -> String {
        let section = section_title(self.name);
        match self.scope {
            Scope::Global => section.to_owned(),
            Scope::Project(name) => format!("{name} · {section}"),
            Scope::NoProject => format!("{NO_PROJECT_TITLE} · {section}"),
        }
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
pub fn grouped<'a>(sections: Vec<(&'a Section, Vec<&'a OverviewEntry>)>) -> Vec<Part<'a>> {
    let in_project_sections = || {
        sections
            .iter()
            .filter(|(section, _)| PROJECT_SECTIONS.contains(&section.name))
    };
    let names: BTreeSet<&str> = in_project_sections()
        .flat_map(|(_, entries)| entries.iter().filter_map(|entry| entry.project.as_deref()))
        .collect();
    let scopes = names
        .into_iter()
        .map(Scope::Project)
        .chain(std::iter::once(Scope::NoProject));

    let mut parts = Vec::new();
    for scope in scopes {
        for name in PROJECT_SECTIONS {
            let entries: Vec<&OverviewEntry> = in_project_sections()
                .filter(|(section, _)| section.name == name)
                .flat_map(|(_, entries)| entries.iter().copied())
                .filter(|entry| belongs(scope, entry))
                .collect();
            if !entries.is_empty() {
                let total = entries.len();
                parts.push(Part {
                    scope,
                    name,
                    entries,
                    total,
                });
            }
        }
    }
    parts.extend(
        flat(sections)
            .into_iter()
            .filter(|part| !PROJECT_SECTIONS.contains(&part.name) && part.total > 0),
    );
    parts
}

fn belongs(scope: Scope<'_>, entry: &OverviewEntry) -> bool {
    match scope {
        Scope::Project(name) => entry.project.as_deref() == Some(name),
        Scope::NoProject => entry.project.is_none(),
        Scope::Global => true,
    }
}
