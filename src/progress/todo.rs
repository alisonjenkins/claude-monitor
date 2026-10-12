//! Parser for an agent todo-list snapshot.
//!
//! Assumption: T004 found no todo tool in this environment's Claude Code, so
//! the payload shape is unverified. The parser accepts the shape Claude Code's
//! TodoWrite tool is known to use and a few near variants: a JSON array of
//! `{content|text|title, status}` objects, either bare or under a `todos`,
//! `items` or `tasks` key, optionally wrapped in `tool_input`. `status` is
//! `pending`, `in_progress` or `completed` (also `done`, `blocked`); any other
//! value counts as pending. Entries without text are skipped. Absent or
//! unrecognised input yields `None`, never an error.

use serde_json::Value;

use super::{Entry, Progress, Source, SourceKind};
use crate::model::StepStatus;

const LIST_KEYS: [&str; 3] = ["todos", "items", "tasks"];
const TEXT_KEYS: [&str; 3] = ["content", "text", "title"];

/// `None` for absent or unrecognised payloads.
pub fn parse(json: &str) -> Option<Progress> {
    let value: Value = serde_json::from_str(json).ok()?;
    let list = find_list(&value)?;
    let entries: Vec<Entry> = list.iter().filter_map(entry).collect();
    Progress::build(
        entries,
        Source {
            kind: SourceKind::AgentTodo,
            path: None,
        },
    )
}

fn find_list(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Array(list) => Some(list),
        Value::Object(map) => LIST_KEYS
            .iter()
            .find_map(|k| map.get(*k).and_then(Value::as_array))
            .or_else(|| map.get("tool_input").and_then(find_list)),
        _ => None,
    }
}

fn entry(item: &Value) -> Option<Entry> {
    let map = item.as_object()?;
    let text = TEXT_KEYS
        .iter()
        .find_map(|k| map.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    let status = match map.get("status").and_then(Value::as_str) {
        Some("completed" | "done") => StepStatus::Done,
        Some("in_progress") => StepStatus::Doing,
        Some("blocked") => StepStatus::Blocked,
        _ => StepStatus::Todo,
    };
    Some(Entry {
        text: text.to_string(),
        status,
        group: None,
        external_id: None,
    })
}
