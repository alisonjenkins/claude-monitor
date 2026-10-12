//! Progress derived from a real plan: spec-kit `tasks.md`, a Markdown
//! checklist, or an agent todo snapshot (US4, FR-015, FR-016).
//!
//! Parsers are pure functions over text. They return `None` when the input
//! holds no tasks, so a caller never shows a figure it cannot derive.

pub mod binding;
pub mod checklist;
pub mod speckit;
pub mod todo;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::StepStatus;

/// Mirrors `progress_binding.source_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    AgentTodo,
    SpeckitTasks,
    MarkdownChecklist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub ordinal: u32,
    pub text: String,
    pub status: StepStatus,
    pub group: Option<String>,
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseCount {
    pub name: String,
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    pub current: Option<Item>,
    pub phases: Vec<PhaseCount>,
    pub items: Vec<Item>,
    pub source: Source,
}

impl Progress {
    /// `"4 of 10"`.
    pub fn label(&self) -> String {
        format!("{} of {}", self.done, self.total)
    }

    /// The named source of the figure (FR-015a): path plus kind.
    pub fn source_label(&self) -> String {
        let kind = match self.source.kind {
            SourceKind::AgentTodo => "agent todo list",
            SourceKind::SpeckitTasks => "spec-kit tasks",
            SourceKind::MarkdownChecklist => "Markdown checklist",
        };
        match (self.source.kind, self.source.path.as_deref()) {
            (SourceKind::AgentTodo, _) | (_, None) => kind.to_string(),
            (_, Some(path)) => format!("{path} ({kind})"),
        }
    }

    /// `None` when there are no items: no plan, no figure (FR-016).
    pub(crate) fn build(entries: Vec<Entry>, source: Source) -> Option<Self> {
        if entries.is_empty() {
            return None;
        }
        let mut items = Vec::with_capacity(entries.len());
        let mut phases: Vec<PhaseCount> = Vec::new();
        // Phase name to its place in `phases`; a scan per entry would be
        // quadratic on a file with one phase per task.
        let mut phase_index: HashMap<String, usize> = HashMap::new();
        let mut done = 0_u32;
        for (idx, entry) in entries.into_iter().enumerate() {
            let is_done = entry.status == StepStatus::Done;
            if is_done {
                done = done.saturating_add(1);
            }
            if let Some(name) = &entry.group {
                let pos = phase_index.get(name.as_str()).copied();
                let phase = match pos.and_then(|i| phases.get_mut(i)) {
                    Some(phase) => phase,
                    None => {
                        phase_index.insert(name.clone(), phases.len());
                        phases.push(PhaseCount {
                            name: name.clone(),
                            done: 0,
                            total: 0,
                        });
                        match phases.last_mut() {
                            Some(phase) => phase,
                            None => continue,
                        }
                    }
                };
                phase.total = phase.total.saturating_add(1);
                if is_done {
                    phase.done = phase.done.saturating_add(1);
                }
            }
            items.push(Item {
                ordinal: u32::try_from(idx).unwrap_or(u32::MAX),
                text: entry.text,
                status: entry.status,
                group: entry.group,
                external_id: entry.external_id,
            });
        }
        let current = items.iter().find(|i| i.status != StepStatus::Done).cloned();
        Some(Self {
            done,
            total: u32::try_from(items.len()).unwrap_or(u32::MAX),
            current,
            phases,
            items,
            source,
        })
    }
}

/// A parsed task before ordinals and counts are assigned.
pub(crate) struct Entry {
    pub text: String,
    pub status: StepStatus,
    pub group: Option<String>,
    pub external_id: Option<String>,
}

/// One meaningful line of a Markdown document, outside code fences.
pub(crate) enum Line<'a> {
    Heading { level: usize, text: &'a str },
    Checkbox { done: bool, rest: &'a str },
}

/// Headings and checkbox lines in file order. Lines inside fenced code
/// blocks (``` or ~~~) are skipped, so examples never count as tasks.
pub(crate) fn scan_markdown(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        match (fence, marker) {
            (None, Some(m)) => {
                fence = Some(m);
                continue;
            }
            (Some(open), Some(m)) if open == m => {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            (None, None) => {}
        }
        if let Some(parsed) = parse_checkbox(trimmed) {
            out.push(parsed);
        } else if let Some(heading) = parse_heading(line) {
            out.push(heading);
        }
    }
    out
}

fn parse_checkbox(trimmed: &str) -> Option<Line<'_>> {
    let after_bullet = ["- ", "* ", "+ "]
        .iter()
        .find_map(|b| trimmed.strip_prefix(b))?;
    let inner = after_bullet.strip_prefix('[')?;
    let mut chars = inner.chars();
    let mark = chars.next()?;
    let rest = chars.as_str().strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let done = match mark {
        ' ' => false,
        'x' | 'X' => true,
        _ => return None,
    };
    Some(Line::Checkbox {
        done,
        rest: rest.trim(),
    })
}

fn parse_heading(line: &str) -> Option<Line<'_>> {
    let indent = line.len().saturating_sub(line.trim_start().len());
    if indent > 3 {
        return None;
    }
    let body = line.trim_start();
    let hashes = body.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let text = body.get(hashes..)?.strip_prefix(' ')?.trim();
    if text.is_empty() {
        return None;
    }
    Some(Line::Heading {
        level: hashes,
        text,
    })
}
