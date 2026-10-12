//! Progress derived from a real plan: spec-kit `tasks.md`, a Markdown
//! checklist, or an agent todo snapshot (US4, FR-015, FR-016).
//!
//! Parsers are pure functions over text. They return `None` when the input
//! holds no tasks, so a caller never shows a figure it cannot derive.

pub mod checklist;
pub mod speckit;
pub mod todo;

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
        String::new()
    }

    /// The named source of the figure (FR-015a): path plus kind.
    pub fn source_label(&self) -> String {
        String::new()
    }
}
