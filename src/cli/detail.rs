//! What a session's detail shows beyond the overview row: its plan progress
//! with the source it came from, the verbatim summaries, and the model-written
//! ones kept apart under the "generated" label (FR-015a, FR-017, FR-017a).
//! Shared by `show` and the TUI detail page.

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use super::progress::{candidate_line, current_line, json_value};
use crate::env::Env;
use crate::export::links::link_json;
use crate::model::{OwnerKind, Session};
use crate::progress::binding::{self, session_root, ProgressView};
use crate::store::links::Link;
use crate::store::summaries::{Summary, GENERATED_LABEL};
use crate::store::Store;
use crate::text::{shell_word, single_line};

const OWNER_KIND: &str = "session";
const LABEL_WIDTH: usize = 14;
const UNKNOWN_MODEL: &str = "unknown model";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDetail {
    pub id: String,
    pub progress: ProgressView,
    pub task_description: Option<String>,
    pub left_off: Option<String>,
    pub generated: Vec<Summary>,
    pub links: Vec<Link>,
    pub root: PathBuf,
}

fn line(label: &str, value: &str) -> String {
    format!("{label:<LABEL_WIDTH$}{value}")
}

/// `task 42 (doc 7)`, without the doc when none is recorded.
pub fn link_text(link: &Link) -> String {
    let kind = single_line(&link.cortex_kind);
    let node = single_line(&link.cortex_node_id);
    match &link.cortex_doc_id {
        Some(doc) => format!("{kind} {node} (doc {})", single_line(doc)),
        None => format!("{kind} {node}"),
    }
}

impl SessionDetail {
    /// Reads the progress view (stat only, no writes) and the stored summaries.
    pub fn load(store: &Store, env: &Env, session: &Session) -> Result<Self, String> {
        let progress = binding::view(store, env, session).map_err(|e| e.to_string())?;
        let generated = store
            .list_summaries(OWNER_KIND, &session.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|summary| summary.model_written)
            .collect();
        let links = store
            .links_for(OwnerKind::Session, &session.id)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            id: session.id.clone(),
            progress,
            task_description: session.task_description.clone(),
            left_off: session.left_off.clone(),
            generated,
            links,
            root: session_root(session),
        })
    }

    /// The `show --json` additions: `progress_status`, `progress` (bound only),
    /// `candidates` (pick-one only) and `summaries`.
    pub fn json_fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        if let Value::Object(mut view) = json_value(&self.id, &self.progress) {
            if let Some(status) = view.remove("status") {
                fields.insert("progress_status".to_owned(), status);
            }
            for key in ["progress", "candidates"] {
                if let Some(value) = view.remove(key) {
                    fields.insert(key.to_owned(), value);
                }
            }
        }
        let generated: Vec<Value> = self
            .generated
            .iter()
            .map(|summary| {
                json!({
                    "text": summary.body,
                    "model": summary.model,
                    "created_at": summary.created_at,
                    "label": summary.label().unwrap_or(GENERATED_LABEL),
                })
            })
            .collect();
        fields.insert(
            "summaries".to_owned(),
            json!({
                "task_description": self.task_description,
                "left_off": self.left_off,
                "generated": generated,
            }),
        );
        fields.insert(
            "links".to_owned(),
            Value::Array(self.links.iter().map(link_json).collect()),
        );
        fields
    }

    /// One plain line per link: `task 42 (doc 7)`.
    pub fn link_lines(&self) -> Vec<String> {
        self.links.iter().map(link_text).collect()
    }

    /// Plain lines for the progress block; empty when there is nothing to say.
    pub fn progress_lines(&self) -> Vec<String> {
        match &self.progress {
            ProgressView::None => Vec::new(),
            ProgressView::PickOne(candidates) => {
                let mut lines = vec![
                    line("progress", "none: several plans could be this session's"),
                    line(
                        "",
                        &format!(
                            "choose one with `claude-monitor progress bind {} <path>`",
                            shell_word(&self.id)
                        ),
                    ),
                ];
                lines.extend(candidates.iter().map(|c| candidate_line(&self.root, c)));
                lines
            }
            ProgressView::Bound(bound) => {
                let Some(progress) = &bound.progress else {
                    return Vec::new();
                };
                let mut head = format!(
                    "{}  {}",
                    single_line(&progress.label()),
                    single_line(&progress.source_label())
                );
                if bound.stale {
                    head.push_str("  (stale: source unreadable, figure may be out of date)");
                }
                let mut lines = vec![line("progress", &head)];
                if let Some(current) = &progress.current {
                    lines.push(line("current", &current_line(current)));
                }
                if !progress.phases.is_empty() {
                    let phases: Vec<String> = progress
                        .phases
                        .iter()
                        .map(|p| format!("{} {}/{}", single_line(&p.name), p.done, p.total))
                        .collect();
                    lines.push(line("phases", &phases.join(", ")));
                }
                lines
            }
        }
    }

    /// Plain lines for the model-written summaries, each labelled.
    pub fn generated_lines(&self) -> Vec<String> {
        self.generated
            .iter()
            .map(|summary| {
                let model = summary.model.as_deref().unwrap_or(UNKNOWN_MODEL);
                format!(
                    "{GENERATED_LABEL} ({model}, {}): {}",
                    summary.created_at, summary.body
                )
            })
            .collect()
    }
}
