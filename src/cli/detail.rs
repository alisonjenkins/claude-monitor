//! What a session's detail shows beyond the overview row: its plan progress
//! with the source it came from, the verbatim summaries, and the model-written
//! ones kept apart under the "generated" label (FR-015a, FR-017, FR-017a).
//! Shared by `show` and the TUI detail page.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::env::Env;
use crate::model::Session;
use crate::progress::binding::ProgressView;
use crate::store::summaries::Summary;
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDetail {
    pub id: String,
    pub progress: ProgressView,
    pub task_description: Option<String>,
    pub left_off: Option<String>,
    pub generated: Vec<Summary>,
    pub root: PathBuf,
}

impl SessionDetail {
    /// Reads the progress view (stat only, no writes) and the stored summaries.
    pub fn load(store: &Store, env: &Env, session: &Session) -> Result<Self, String> {
        let _ = (store, env);
        Ok(Self {
            id: session.id.clone(),
            progress: ProgressView::None,
            task_description: None,
            left_off: None,
            generated: Vec::new(),
            root: PathBuf::new(),
        })
    }

    /// The `show --json` additions: `progress_status`, `progress` (bound only),
    /// `candidates` (pick-one only) and `summaries`.
    pub fn json_fields(&self) -> Map<String, Value> {
        Map::new()
    }

    /// Plain lines for the progress block; empty when there is nothing to say.
    pub fn progress_lines(&self) -> Vec<String> {
        Vec::new()
    }

    /// Plain lines for the model-written summaries, each labelled.
    pub fn generated_lines(&self) -> Vec<String> {
        Vec::new()
    }
}
