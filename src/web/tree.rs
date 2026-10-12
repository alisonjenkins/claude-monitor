//! The overview as a tree: every project with its sessions, then the sessions
//! that have no project. Pure functions over the JSON `list --json` prints, so
//! the page and the JSON form of `/` share one grouping.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use super::pages::RowProgress;

/// The order states are named in a summary: what needs a person first.
const STATE_ORDER: [&str; 6] = [
    "working",
    "needs_permission",
    "interrupted",
    "idle",
    "finished",
    "unknown",
];

/// Separates the parts of a summary.
const SUMMARY_SEPARATOR: &str = " · ";

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// `2 working · 1 interrupted · 3 finished`, or `no sessions`.
pub fn summary(sessions: &[Value]) -> String {
    let parts: Vec<String> = STATE_ORDER
        .iter()
        .filter_map(|state| {
            let count = sessions
                .iter()
                .filter(|s| text(s, "state") == *state)
                .count();
            (count > 0).then(|| format!("{count} {}", state.replace('_', " ")))
        })
        .collect();
    if parts.is_empty() {
        "no sessions".to_owned()
    } else {
        parts.join(SUMMARY_SEPARATOR)
    }
}

fn counts(sessions: &[Value]) -> Value {
    let mut out = Map::new();
    for state in STATE_ORDER {
        let count = sessions
            .iter()
            .filter(|s| text(s, "state") == state)
            .count();
        if count > 0 {
            out.insert(state.to_owned(), json!(count));
        }
    }
    Value::Object(out)
}

fn node(sessions: Vec<Value>) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("summary".to_owned(), json!(summary(&sessions)));
    out.insert("counts".to_owned(), counts(&sessions));
    out.insert("sessions".to_owned(), Value::Array(sessions));
    out
}

/// `{"projects": [...], "no_project": {...}}`: projects alphabetical, empty
/// ones kept, each session under the project it names. A session naming a
/// project that is not in `projects` is treated as having none.
pub fn build(
    projects: &[Value],
    entries: &[Value],
    progress: &HashMap<String, RowProgress>,
) -> Value {
    let mut sorted: Vec<&Value> = projects.iter().collect();
    sorted.sort_by_key(|p| (text(p, "name").to_lowercase(), text(p, "name").to_owned()));
    let known = |name: &str| sorted.iter().any(|p| text(p, "name") == name);
    let nodes: Vec<Value> = sorted
        .iter()
        .map(|project| {
            let name = text(project, "name");
            let own = entries
                .iter()
                .filter(|e| text(e, "project") == name)
                .cloned()
                .collect();
            let mut out = node(own);
            out.insert("id".to_owned(), json!(text(project, "id")));
            out.insert("name".to_owned(), json!(name));
            out.insert("status".to_owned(), json!(text(project, "status")));
            if let Some(figure) = progress.get(text(project, "id")) {
                out.insert(
                    "progress".to_owned(),
                    json!({
                        "done": figure.done,
                        "total": figure.total,
                        "source": figure.source,
                    }),
                );
            }
            Value::Object(out)
        })
        .collect();
    let loose = entries
        .iter()
        .filter(|e| !known(text(e, "project")))
        .cloned()
        .collect();
    json!({ "projects": nodes, "no_project": Value::Object(node(loose)) })
}
