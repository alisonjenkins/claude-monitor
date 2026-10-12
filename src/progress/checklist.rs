//! Parser for plain Markdown checklists.
//!
//! Every `- [ ]` / `- [x]` / `- [X]` line (also with `*` or `+` bullets, at
//! any nesting depth) is a step, grouped under the nearest heading. Lines in
//! fenced code blocks are ignored.

use super::{scan_markdown, Entry, Line, Progress, Source, SourceKind};
use crate::model::StepStatus;

/// `None` when the text holds no checkbox lines.
pub fn parse(text: &str, path: Option<&str>) -> Option<Progress> {
    let mut group: Option<String> = None;
    let mut entries = Vec::new();
    for line in scan_markdown(text) {
        match line {
            Line::Heading { text, .. } => group = Some(text.to_string()),
            Line::Checkbox { done, rest } => entries.push(Entry {
                text: rest.to_string(),
                status: if done {
                    StepStatus::Done
                } else {
                    StepStatus::Todo
                },
                group: group.clone(),
                external_id: None,
            }),
        }
    }
    Progress::build(
        entries,
        Source {
            kind: SourceKind::MarkdownChecklist,
            path: path.map(str::to_string),
        },
    )
}
