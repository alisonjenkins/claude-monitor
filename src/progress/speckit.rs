//! Parser for spec-kit `tasks.md` files.
//!
//! A task is a checkbox line whose text starts with an id (`T083`, `T006a`),
//! then optional `[P]` and `[US#]` markers. Phases are the `## ` headings;
//! deeper headings do not start a new phase. Checkboxes without a task id are
//! not tasks.

use super::{scan_markdown, Entry, Line, Progress, Source, SourceKind};
use crate::model::StepStatus;

/// `None` when the text holds no `T###` task lines.
pub fn parse(text: &str, path: Option<&str>) -> Option<Progress> {
    let mut phase: Option<String> = None;
    let mut entries = Vec::new();
    for line in scan_markdown(text) {
        match line {
            Line::Heading { level: 2, text } => phase = Some(text.to_string()),
            Line::Heading { .. } => {}
            Line::Checkbox { done, rest } => {
                let Some((id, task)) = split_task(rest) else {
                    continue;
                };
                entries.push(Entry {
                    text: task.to_string(),
                    status: if done {
                        StepStatus::Done
                    } else {
                        StepStatus::Todo
                    },
                    group: phase.clone(),
                    external_id: Some(id.to_string()),
                });
            }
        }
    }
    Progress::build(
        entries,
        Source {
            kind: SourceKind::SpeckitTasks,
            path: path.map(str::to_string),
        },
    )
}

fn split_task(rest: &str) -> Option<(&str, &str)> {
    let (id, mut tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if !is_task_id(id) {
        return None;
    }
    loop {
        tail = tail.trim_start();
        if let Some(t) = tail.strip_prefix("[P]") {
            tail = t;
        } else if let Some(t) = strip_story(tail) {
            tail = t;
        } else {
            break;
        }
    }
    let text = tail.trim();
    Some((id, if text.is_empty() { id } else { text }))
}

fn strip_story(s: &str) -> Option<&str> {
    let after = s.strip_prefix("[US")?;
    let end = after.find(']')?;
    let digits = after.get(..end)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    after.get(end.saturating_add(1)..)
}

fn is_task_id(id: &str) -> bool {
    let Some(body) = id.strip_prefix('T') else {
        return false;
    };
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    let suffix = body.get(digits..).unwrap_or("");
    digits > 0 && suffix.bytes().all(|b| b.is_ascii_lowercase())
}
